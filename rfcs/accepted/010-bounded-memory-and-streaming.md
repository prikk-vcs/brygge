# RFC 010 — Bounded memory and streaming (OQ-F)

**Status.** **Accepted (2026-09-12)** — design settled by the architect; the theme is owner-directed (the
owner asked to proceed on large-repo streaming, 2026-09-12). Picks up **OQ-F**, deferred across RFC 003 and
every decoder RFC (004–007) as "correctness and determinism first." Cross-cutting (touches the decoders and,
at its limit, the artifact writer), settled as a design before implementation. **No IR contract change**
(D-1); the frozen 1.0.0 artifact bytes are unaffected.

**Implementation status. Increment 1 built and green** (`brygge-decode-svn`): snapshot retention is now
bounded to the revisions a `copyfrom` names (plus the rolling previous tree), turning
O(revisions × tree) scratch into O(copy-targets × tree). Local change to `decode.rs`/`tree.rs`, no format
or determinism change; the atoms produced and their order are unchanged (verified: existing copy/branch/
determinism tests pass, plus a new test that a `copyfrom` to a *distant* revision resolves across a gap of
non-retained revisions). **The measurement harness is built** (`tools/bench`) and confirms the win with a
same-corpus A/B: peak drops from ~2.8 GiB to ~62 MiB at 20k revisions over a 500-file tree, IR identical
(OQ-A). **Queued:** increments 2–4 (SVN dumpstream iterator, CVS reconstruction bound, and — gated on
measurement, OQ-A/D-3 — a streaming artifact writer), each measured with the harness before/after.

**Tracks.** A cross-cutting engineering theme, not a new source. Touches `brygge-decode-svn` (the sharpest
target), `brygge-decode-cvs`, and potentially `brygge-ir`'s artifact writer. Revisits the threat model's
**T-8** (resource exhaustion) — bounds are a security control, not just performance.

## Summary

A "large repository" is a memory question, and the first job is to say *where the memory actually goes* —
measured or structurally-clear, never guessed (the project's standing honesty rule, and RFC 133's).

**The clarifying finding: streaming cannot make the IR smaller, because the IR *is* the content.** The IR
holds every atom and every blob of the imported history; that is O(repository content) by definition, and
no streaming trick shrinks it. So "streaming for a large repo" does **not** mean "hold less than the IR." It
means:

> **Bound the decoder's *scratch* space to ~O(IR), instead of O(IR × history-depth).**

The over-allocation worth removing is the intermediate state a decoder holds *beyond* the IR it is
building. Ranked by how far each exceeds O(IR):

1. **SVN snapshot history (worst).** `brygge-decode-svn` keeps a `Vec<Tree>` — a tree snapshot **per
   revision** — so directory copies can resolve `copyfrom` from any past revision. That is
   **O(revisions × live-tree)**, which for a long-lived repo dwarfs the IR. This is the sharpest target and
   is fixable **locally, with no format or determinism change** (D-2 / increment 1).
2. **SVN parsed dump.** The decoder parses the whole dumpstream into an in-memory `Dump` before processing —
   a second O(content) copy alongside the IR. Removing it means parsing the dumpstream as a **record
   iterator** (increment 2).
3. **CVS per-revision reconstruction.** `brygge-decode-cvs` reconstructs every revision's content by
   walking the delta chain from `head` each time — O(revisions²) work, and it holds all reconstructed
   `FileRev`s at once. Bounding this is increment 3.
4. **The builder / artifact writer (~O(IR), mostly unavoidable).** `IrBuilder` accumulates all atoms and the
   content store and `to_bytes` serializes the whole `Ir`; peak is ~2×IR (the built IR plus its serialized
   bytes). This is close to the floor — the IR is the content — so a streaming *writer* (produce artifact
   bytes as atoms are finished, dropping them from memory) is worth building **only if measured to matter**
   after 1–3, and only if it stays byte-identical (D-3). It is deliberately the last increment.

## Decisions

- **D-1 — No IR contract change; artifacts stay byte-identical.** Streaming changes *how* the bytes are
  produced and consumed, never *what* they are. Any streaming writer must emit the exact bytes `to_bytes`
  would (the frozen 1.0.0 format, RFC 003 D-3). A decode with a bounded-scratch decoder produces the same
  `Ir` — and the same artifact — as today; this is testable by comparison and is the acceptance bar for
  every increment.

- **D-2 — Determinism is preserved by construction, and it is why 1–3 are safe.** The canonical order
  (`IrBuilder`'s topological sort with a total tie-break, RFC 003 D-5) is unchanged; bounding *scratch* does
  not touch which atoms are produced or in what order they are added. Determinism is a security property
  (T-9), so no increment may make output depend on memory pressure, retention choices, or iteration order —
  each increment ships a "decode twice → byte-identical" test, and increment 1 additionally asserts
  identical output to the pre-bound decoder on a copy-bearing fixture.

- **D-3 — A streaming *writer* is gated on measurement and byte-identity.** The builder/writer refactor
  (increment 4) is the only part that touches `brygge-ir`, and it is deferred until 1–3 are done and the
  remaining ~2×IR peak is *measured* to be the binding constraint on a real large repository. It must
  produce byte-identical artifacts (the count-prefixed framing means either a two-pass or a
  count-patched-in-place write; the integrity digest becomes a running hash over the fixed byte order). If
  measurement shows the decoder scratch (1–3) was the real problem, increment 4 may prove unnecessary —
  which is the preferred outcome (the IR floor is the honest limit).

- **D-4 — Bounds refuse rather than exhaust (T-8), and that already ships.** Each decoder already caps its
  untrusted input (`MAX_DUMP_BYTES`, `MAX_RCS_BYTES`, `MAX_FILES`) and refuses past the ceiling (`FA-3`).
  Streaming lowers the *steady-state* footprint; the *ceiling* refusals stay. A repository that is large
  because it is hostile is still refused; a repository that is large because it is real now costs less to
  import. The threat model's T-8 control (C-2d/C-8) is strengthened, not replaced.

## Increments

1. **SVN snapshot retention bound (this RFC's first build).** Keep only the tree snapshots a `copyfrom`
   actually references (plus the rolling previous tree), instead of one per revision. A first pass over the
   dump collects the set of referenced `copyfrom` revisions; the main pass retains a snapshot only for those.
   Local to `brygge-decode-svn` (`decode.rs` + `tree.rs`); no format, determinism, or dependency change.
   **O(revisions × tree) → O(copy-targets × tree)** — copy targets are branch/tag creation points, typically
   ≪ revisions. Acceptance: existing copy/branch tests still pass; byte-identical output; a
   many-revisions-few-copies fixture retains few snapshots.
2. **SVN dumpstream record iterator.** Parse the dumpstream as a streamed sequence of records rather than a
   whole `Dump`, so the parsed input is not a second O(content) copy. Larger change to `dumpstream.rs`;
   byte-identical output.
3. **CVS reconstruction bound.** Reconstruct per-file content along the delta chain incrementally (reuse the
   running content instead of re-walking from `head`), and avoid holding all `FileRev`s where the clustering
   pass allows a bounded window. Local to `brygge-decode-cvs`.
4. **Streaming artifact writer (gated, D-3).** Only if 1–3 leave a measured ~2×IR peak that binds on a real
   large repository. Touches `brygge-ir`; byte-identical; the one increment that would carry an architect
   note against RFC 003 D-3 (the write path, not the format).

## Open questions

- **OQ-A — The measurement** — **harness built** (`tools/bench`, dev-only, zero-dependency, `/proc`-based
  peak RSS with one decode per subprocess). It records decoder peak memory + time + IR size on synthetic
  corpora at scale, so increments 2–4 are gated on evidence. **Increment 1 measured (A/B, same corpus,
  pre-bound `251ece1` vs post-bound `c7ba7bf`, `svn-revs` = a 500-file tree + n single-file edits):** the IR
  is identical in both, while peak drops **~145 MiB → ~6 MiB at 1k revisions, ~708 MiB → ~19 MiB at 5k, and
  ~2.8 GiB → ~62 MiB at 20k** — pre-bound grew O(revisions × tree), post-bound tracks the IR (the reduction
  widens with scale, as predicted). The remaining targets (2–3) are to be measured with this harness before
  they are built; increment 4's gate (D-3) is decided on its numbers.
- **OQ-B — CVS incremental reconstruction vs branches.** Reusing running content down the trunk is
  straightforward; branch revisions (forward deltas off a branch point) complicate a single running buffer.
  *Leaning:* bound the trunk case first, keep branches on the current per-revision reconstruction, measure.

## Consequences

- The largest concrete over-allocation brygge has (SVN's per-revision snapshots) is removed immediately, with
  no risk to the frozen contract or to determinism (increment 1).
- The project keeps its honesty discipline about performance: the deep refactor (increment 4) is gated on
  measurement, and may be shown unnecessary — the IR floor (O(content)) is stated plainly as the real limit,
  so "streaming" never over-promises a footprint below the content it must carry.
- T-8 is strengthened: steady-state memory drops while the refuse-past-ceiling controls stay.

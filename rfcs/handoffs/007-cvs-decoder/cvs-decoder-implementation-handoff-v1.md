# Handoff — `brygge-decode-cvs` implementation (RFC 007)

**Realizes:** accepted **RFC 007** (CVS decoder), under **RFC 009** (dependency policy) and against the
**brygge-ir** core (RFCs 001/002/003), frozen at IR **1.0.0**. Read RFC 007's decisions D-1…D-9 first; this
turns them into a build. Follows the RFC 004/005/006 decoder pattern closely — read the `brygge-decode-svn`
decoder and its handoff for the shape; this document states only what differs for CVS.
**Status.** Inherits Accepted; read tier ruled **Tier R (pure-Rust RCS `,v` reader)**, confidence floor
ruled **per-changeset** (import the confident majority, loudly flag/refuse under-floor). **D-9 confirmed CVS
fits IR 1.0.0 with zero contract changes** — build against the frozen types as-is; add no IR field or
variant.
**Scope (increment 1 = ROADMAP M4):** `brygge decode cvs` as a library — read a CVS repository (a directory
of RCS `,v` files, deleted files under `Attic/`) and produce a `brygge_ir::Ir`: a **`Derived`** changeset
spine reconstructed from per-file revisions (each atom `Derived(ReconstructedChangeset)`), the per-file
content and history carried faithfully, and an opt-in `Derived` tag/branch layer. Plus the RFC 009 D-7
isolation test.
**Out (queued):** the CLI `decode cvs` dispatch and `verify --against-source` (small — the CLI dispatches on
source kind, and for CVS checks *per-file* correspondence + determinism, **not** changeset correspondence,
D-7); rename **inference** (OQ-C, ships **off** / absent); large-repo streaming (OQ-F).

## 1. Crate & layout (RFC 009 D-1)

`crates/brygge-decode-cvs/` — the only crate that reads CVS. `#![forbid(unsafe_code)]`,
`#![warn(missing_docs)]`, workspace lints, 2018 module style, tests as siblings. **Dependencies:
`brygge-ir` only.** RCS `,v` files are uncompressed text, so — like SVN — there is **no decompression codec,
no C, no FFI, no new `workspace.dependencies`, no `deny.toml` addition, and no subprocess** (the reader
reads the repository's own files directly; there is no `cvs`-tool step). `brygge-ir` and `verify --internal`
link none of it.

Modules:
- `lib.rs` — crate docs, re-exports, `Error`, `decode(source: &Source, &Options) -> Result<Ir>`, `DECODER`.
- `source.rs` — resolve the input: a **local CVS repository path** (a module directory of `,v` files).
  **Refuse a `:pserver:`/remote/URL source** with a named reason (INV-3): brygge reads local files only.
- `rcs.rs` (+ tests) — the RCS `,v` reader (§5a): the new untrusted-input parser.
- `scan.rs` (+ tests) — walk the repository tree, enumerate `,v` files, map store paths → repo-relative
  paths (`foo/bar.c,v` → `foo/bar.c`; `foo/Attic/bar.c,v` → `foo/bar.c`, a mainline-deleted file).
- `cluster.rs` (+ tests) — the changeset reconstructor (§5b): the research core (D-3).
- `symbols.rs` (+ tests) — per-file symbolic names → reconstructed tag/branch refs (D-5, opt-in).
- `decode.rs` — orchestrate → IR (mirrors the svn/hg/git `decode.rs`).
- `options.rs` — `Options`: the clustering `window` and confidence `floor`, `reconstruct_refs` **off by
  default**, recorded into provenance (PR-5).

## 2. The settled decisions (build to these)

- **The changeset atom is `Derived`; per-file content/history are faithful (SRC-C1/C2/C3).** Every
  reconstructed changeset → `ChangeAtom { status: Derived(Derivation { kind: ReconstructedChangeset, by:
  "brygge-decode-cvs", params: {"window": "<seconds>", "cluster_keys": "author,log"}, confidence: Some(_) }) }`.
  This is the M4 defining property — unlike git/hg/svn, the atom itself is a judgment.
- **CVS revision numbers are never identity** — the changeset's `SourceIdentity.atom_id` packs the canonical,
  sorted set of source-native `(path@rev)` pairs the changeset groups (D-9/OQ-D); the `AtomId` is
  brygge-ir's own SHA-256. `repo_id` = the CVSROOT/module root (a stable marker).
- **Refuse, never approximate**, below the confidence floor (D-8) and below the format line (an RCS file the
  reader cannot parse is refused, not guessed).
- **No network, no `cvs` tool, no source code executes** (D-7/INV-2/INV-3): read `,v` bytes; a `:pserver:`
  source is refused.
- **Byte-deterministic** for the same repository + brygge version + options: the clustering is a pure
  function of the per-file revisions and the recorded `window`, with a total, stable tie-break (§5b).
- **Never claim changeset-level VF-2 (D-7/SRC-C3).** The against-source surface (queued) re-runs the
  reconstruction (VF-1) and checks *per-file content* correspondence; it does not assert changeset
  correspondence. The CVS faithfulness statement (VF-5) says so before the run.

## 3. The mapping spec (CVS → brygge-ir)

Build with `brygge_ir::builder::IrBuilder`. Per reconstructed changeset (in reconstructed time order):

- **Changeset → `ChangeAtom`** (`status = Derived(ReconstructedChangeset)`, §2): `parents` = the prior
  changeset(s) it descends from (the per-file revision ancestry threads the DAG; a branch's changesets are
  parented at the branch point, PR-2); `metadata` = author / a representative date (e.g. the latest per-file
  date, recorded as chosen) / log, as claims (PR-3); `source.atom_id` = the canonical `(path@rev)` set.
- **Per-file revision in the changeset → `PathOp`** (all `Stated` — the *content* is faithful even though the
  *grouping* is derived): first revision of a path → `Add`; a later revision → `Modify`; a `dead`-state
  revision → `Delete`; in canonical path order; content → `builder.add_blob`. File mode from RCS/CVS flags:
  `-kb` (binary) and text default → the regular IR mode; the executable bit if the RCS `mode` records it.
- **No rename (OQ-C):** CVS records none. A move is a `Delete` + `Add` with no relation; emit **no
  `RenameHint`**. (A Git-style opt-in, always-`Derived` inference is a later increment, off by default —
  not M4.)
- **Tags/branches (opt-in, `reconstruct_refs` on) → `Derived` `RefRecord`s (D-5, §5c):** aggregate each `,v`'s
  symbolic names; a tag name present across files → a `Derived` `RefRecord { kind: Tag,
  status: Derived(ReconstructedBranch, params: {"straddle": "an CVS tag is per-file and may name revisions
  from different reconstructed changesets"}) }` pointing at the best-corresponding changeset; a branch
  (magic number) → `Derived { kind: Branch }`. Off by default; the `Derived` changeset spine always imports.
- **Provenance:** `decoder = "brygge-decode-cvs"`, `params` = the `Options` (window, floor, reconstruct_refs),
  `import_time = None`.

## 4. Floor & loss (D-6/D-8, owner-ruled)

- **Floor — per-changeset confidence (OQ-B):** a changeset whose reconstruction confidence is below the bar
  is **refused/loudly flagged with a named reason** (FA-3, CL-08 exit class **30 convention/confidence**),
  the confident changesets still importing; an **entirely-under-floor** import is a whole-import refusal.
  The bar's numeric value + definition (a time-spread threshold and an ambiguity measure) are a configurable
  default read as policy (CF-03) — see §5b for the confidence computation. Also refuse: a `:pserver:`/remote
  source (§1); an RCS file the reader cannot parse.
- **Loss, with reconstruction uncertainty prominent (SRC-C2, PR-9):** the fidelity report leads with the
  reconstruction — every atom `derived:reconstructed-changeset`, the window used, and a count of
  under-/near-floor changesets. Dropped-with-record (Representation): RCS physical layout & delta encoding;
  keyword expansion / `-kb` text translation (**stored bytes carried**, NG-5); `CVSROOT` admin files
  (modules, notify, …), locks, working-copy state. There is **no `svn:mergeinfo` analogue** — CVS records no
  merge tracking. Nothing in the never-silently-omit class is dropped without a record.

## 5. The genuinely new pieces

All bounds-checked and panic-free on malformed input (untrusted, T-2/INV-2 — §6).

**5a. The RCS `,v` reader (`rcs.rs`).** Parse the RCS file grammar (rcsfile(5)): the **admin** section
(`head`, `branch`, `symbols` = tag/branch names → revisions, `locks`, `comment`, `expand`); the **delta**
section (per revision: `date`, `author`, `state`, `branches`, `next`); `desc`; and the **deltatext** section
(per revision: `log` and `text`). Strings are **`@`-delimited with `@@` escaping a literal `@`** — parse them
by scanning for a lone `@`, never by fixed length; bound string and file size. Reconstruct a revision's
content by walking the **delta chain**: the `head` revision stores full text; trunk ancestors are recovered
by applying **reverse** RCS diffs (ed-style `a`/`d` commands) down the `next` chain, branch revisions by
**forward** diffs along `branches`. Bound the chain length and reconstructed size (a malformed `,v` must not
OOM or loop). `Attic/` files are read the same way (they are ordinary `,v` files for deleted paths).

**5b. The changeset reconstructor (`cluster.rs`) — the research core (SRC-C1, D-3).** Gather every per-file
revision `(path, rev, date, author, log, state)` across all `,v` files. **Cluster** into changesets: group
revisions sharing `(author, log)` whose timestamps fall within the `window`, with **at most one revision
per path** per changeset (a file cannot appear twice). Order changesets by their representative time; thread
`parents` from the shared per-file revision ancestry (trunk linear; branch changesets parented at the branch
point). **Confidence** (D-8): a function of cluster tightness — the internal time-spread against the window,
and whether a revision could plausibly belong to an adjacent cluster (ambiguity); tighter and less ambiguous
→ higher. **Determinism is a security property (VF-1):** sort revisions by `(date, path, rev)` with a total
tie-break, and make every clustering choice deterministic — the same repository + window yields byte-identical
changesets. Record the `window` in provenance (PR-5).

**5c. Symbol aggregation (`symbols.rs`).** Per-file symbolic names are collected across all `,v` files; a
repo-wide tag/branch is the set of per-file revisions it names. Reconstruct (opt-in) → `Derived` refs (§3),
carrying the straddle caveat in params.

## 6. Security posture (tees up the RFC 009 D-6 / GOVERNANCE review against `brygge-03`)

Tier R adds no dependency and **no subprocess** (simpler than SVN Tier D), but it adds a **new
untrusted-input parser** (RCS), so the review will check, and the build must satisfy:
- **Untrusted parser:** all of §5a/§5b bounds-checked, no `unwrap`/`expect`/indexing (workspace lints warn;
  keep clean), typed errors not panics; `@`-string lengths, delta-chain length, reconstructed size, revision
  and file counts all bounded; a malformed `,v` is a typed refusal.
- **No network, no tool execution:** local `,v` files only; a `:pserver:`/remote source refused (INV-3); no
  `cvs` binary is run (INV-2).
- **Isolation (RFC 009 D-7):** `brygge-ir` and `verify --internal` build and run with no `brygge-decode-cvs`
  present.

## 7. Build order

1. `rcs.rs` on a fixture `,v` (hand-written, or produced by `rcs`/`cvs` at test time): parse admin/delta/
   deltatext, reconstruct the head and one older trunk revision, check the content. 2. `scan.rs`: enumerate
   `,v` files incl. `Attic/`, map paths. 3. `cluster.rs`: per-file revisions → changesets on a small fixture
   with a known grouping; determinism. 4. `decode.rs`: assemble the `Derived` changeset spine → `Ir`. 5.
   confidence + floor; loss. 6. `symbols.rs` + the opt-in `Derived` ref layer. 7. determinism + isolation.

## 8. Tests & gates (acceptance checklist)

Fixtures can be **hand-written `,v` files** (RCS is plain text — no `cvs` tool needed to build them), which
is the primary path; a `cvs`/`rcs`-driven fixture may be added, skipping when the tool is absent. Cover:
- **Faithful per-file spine:** a repo with two files and a few revisions decodes; per-file `Add`/`Modify`/
  `Delete` (incl. an `Attic` deletion) and content match; **every atom is `Derived(reconstructed-changeset)`**;
  the fidelity report shows `derived.reconstructed-changeset=<all atoms>` (SRC-C2).
- **Reconstruction:** revisions across files sharing (author, log) within the window cluster into one
  changeset; a different author/log or a gap beyond the window splits them; ordering and parents are
  deterministic.
- **Confidence floor (FA-3):** an ambiguous/skewed fixture drops below the bar → the under-floor changeset is
  flagged/refused with a named reason (exit class 30), the confident ones importing; an all-ambiguous repo is
  a whole refusal.
- **Determinism (VF-1):** decode twice → identical `to_bytes`.
- **Refs (opt-in):** a tag symbol across files → a `Derived` `Tag` ref with the straddle caveat; off by
  default yields no refs.
- **Loss / NG-5:** keyword/`-kb` recorded, stored (unexpanded) bytes carried; CVSROOT/locks recorded; nothing
  silently omitted.
- **RCS parser safety:** a truncated/overlong-`@`-string/cyclic-delta `,v` is a typed refusal, not a panic.
- **Isolation (RFC 009 D-7):** the IR + `verify --internal` build/run with no `brygge-decode-cvs` present.

**Gates (all, `--locked`):** `cargo fmt --check`; `cargo clippy --workspace --all-targets --all-features
--locked -D warnings`; `cargo test --workspace --locked`; `cargo deny check`; `cargo audit`.

## 9. Acceptance criteria

- `decode()` produces a byte-deterministic IR from a CVS repository: a **`Derived` changeset spine** with a
  faithful per-file content/history, and (opt-in) `Derived` tag/branch refs.
- Every atom is `Derived(ReconstructedChangeset)` carrying its clustering params and confidence; the fidelity
  report makes the reconstruction prominent (SRC-C2).
- Under-floor changesets are flagged/refused with named reasons (per-changeset floor, OQ-B); an
  all-under-floor import is refused; a `:pserver:` source and an unparsable `,v` are refused.
- No rename hints by default (OQ-C); keyword/`-kb` dropped-with-record, stored bytes carried; CVS revision
  numbers preserved (packed in `atom_id`); nothing reads as target-verified; **no changeset-level VF-2 is
  claimed** (D-7).
- The RCS reader and reconstructor are panic-free on malformed input; no new crate dependency; no subprocess;
  the isolation test passes; the `brygge-03` security review (untrusted RCS parser) is satisfied.
- **D-9 holds in the build:** no IR field or variant added (confirmed zero-change; `(path@rev)` packs into
  `atom_id`). If the build somehow needs one, stop — it is an *additive* minor contract event for the owner,
  not a silent change.
- All gates green.

## 10. Queued next (not this increment)

- CLI: extend `decode` to accept `cvs`, and `verify --against-source` to dispatch by source kind and, for
  CVS, check **per-file** correspondence + determinism (never changeset correspondence, D-7).
- Rename **inference** for CVS (OQ-C), off by default, always `Derived` if ever added.
- Adaptive clustering windows (OQ-E); large-repo/streaming (OQ-F); a structured per-op source-revision field
  **only if** a consumer needs it (OQ-D — an additive minor contract event, owner-gated).
- With CVS delivered, the **difficulty gradient is complete** (Git, hg, SVN, CVS); the IR held all four
  sources with no contract change.

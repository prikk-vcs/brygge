# Architect security review — the `brygge-decode-svn` Tier D decoder (RFC 006)

**Required by** the GOVERNANCE security gate (a change that adds a source decoder and an untrusted-input
path must get an architect review against `brygge-03` and revisit it) and RFC 009 D-6. **Owner approval:**
not separately required — Tier D adds **no** heavy dependency, so the owner's OQ-A tier ruling (2026-09-08)
stands as the dependency decision; this is the architect review the gate requires, not a new
owner-approval event. **Verdict:** **proceed to implementation**, with the controls below bound as tests.
**Scope:** the trust surface Tier D introduces — a new untrusted-input parser, a running tree model, and an
optional `svnadmin` subprocess — and how brygge-03's invariants survive it. Not the decoder's mapping logic
(that is the program-design handoff, `svn-decoder-implementation-handoff-v1.md`). **Basis:** RFC 006 D-1…D-9
and that handoff.

## What enters the trust surface

| Metric | Value |
|---|---|
| New crate dependencies | **zero** — `brygge-ir` only; the dumpstream is uncompressed, so no `flate2`/`ruzstd`, no C, no FFI, no network crate |
| Crates in the `brygge-decode-svn` subtree | brygge's own + `brygge-ir`'s core; **no** third-party decoder library |
| Crates in the `brygge-ir` core subtree | **unchanged**; links none of this (RFC 009 D-1) |
| C toolchain / `*-sys` / FFI in the tree | **none** |
| New process executed at decode time | **`svnadmin dump`**, optional — only when brygge produces the dump; **not run** when the operator supplies a dumpfile |
| Network backend | **none**; a URL/remote source is refused, `svnrdump` is excluded (INV-3) |
| New untrusted-input parsers | **two** — `dumpstream.rs` and `tree.rs`, pure-Rust, `forbid(unsafe)` |

## Findings and dispositions

- **F-1 — The supply-chain surface SHRINKS; T-4 barely applies (INV-4, contrast the gix review).** Where
  `brygge-decode-git` added 141 crates (gix), Tier D adds **zero**. The audit surface for this source is
  brygge's own pure-Rust code plus, optionally at decode time, the `svnadmin` binary already on the
  operator's host. The decoder that brygge exists to isolate (PU-5) turns out to need nothing isolated.
  **Disposition:** accepted; the strongest possible T-4/A-SUPPLY posture.

- **F-2 — SVN's C-format complexity is isolated by *process*, not linked — a containment advantage over
  Tier L (T-2/TB-2, INV-4, C-4b).** FSFS/BDB parsing is real C territory. Tier D confines it to `svnadmin`
  as a **subprocess**, so a memory-safety bug in svnadmin processing a hostile repository is contained in a
  **separate address space** — it cannot corrupt brygge's process or defeat `forbid(unsafe_code)` — and
  when the operator supplies a dumpfile, svnadmin is not run at all. Linking `libsvn` (Tier L) would have
  pulled that C surface *into* brygge's address space. **Disposition:** accepted; the subprocess posture is
  a security advantage, not merely a dependency-weight one — this is a reason Tier D beat Tier L, and it
  refines C-4b (see the threat-model delta).

- **F-3 — brygge's real defense is the pure-Rust parser, and it must be bounds-checked (T-2/T-8, C-2a/C-2d,
  INV-2).** Whether the dumpstream arrives as a user file or from svnadmin, the attacker-controllable bytes
  ultimately reach `dumpstream.rs`/`tree.rs` — this is the TB-1 boundary for SVN. `forbid(unsafe)` gives
  memory safety; the handoff §5/§6 additionally requires bounds on every declared length/count, on tree
  size, and on directory-copy depth/fan-out, panic-freedom (no `unwrap`/`expect`/indexing), and typed
  refusals — these are C-2a/C-2d and are only real if test-enforced (guardrail 1). Note two amplification
  vectors are removed **by construction**: the dumpstream is uncompressed (no decompression bomb), and
  delta-format dumps are **refused** this increment (§4), so svndiff bombs are out of scope until svndiff is
  deliberately added. **Disposition:** accepted, contingent on the §6 bounds shipping as tests.

- **F-4 — INV-2 holds: no source-provided code executes (INV-2, C-2b).** Two checks. (a) `svnadmin` is a
  **trusted system tool** (TB-4 operator environment), not source-provided code; running it is not an INV-2
  event. (b) `svnadmin dump` reads the revision store and **does not invoke the repository's hook scripts**
  (hooks fire on commit/revprop-change through the server paths, never on `dump`), so a source's hook
  scripts are carried as versioned data if present, never run. `svn:externals` — the one SVN construct that
  could reach another repository — is **refused** (§4), not resolved. **Disposition:** accepted, with a
  standing condition: any future tier that links `libsvn`, resolves externals, or runs a richer `svn`
  subcommand **re-triggers this review** for INV-2.

- **F-5 — INV-3 holds: no network; writes only where told (INV-3, C-6a/C-7).** URL/remote sources are
  refused and `svnrdump` (network) is excluded (§4); `svnadmin dump` is a local read. Source-declared node
  and `copyfrom` paths are **data** — keys in the tree model and strings in the IR — **never** write
  destinations (C-2c/C-7); the only filesystem writes are the operator's `--ir`/`--out`, and the only path
  handed to the subprocess is the operator-supplied **repository** path (TB-4 trusted), passed as a fixed
  argv element with **no shell** (§6). **Disposition:** accepted.

- **F-6 — INV-1 holds: honesty is in the object and non-suppressible (INV-1, C-1a…e).** The stated spine is
  `Stated`; copies are `Stated`; reconstructed branches/tags are `Derived(ReconstructedBranch)` carrying the
  convention (PR-5) and, for tags, the not-immutable caveat — a reader tells judgment from fact without
  re-running (C-1e); `svn:mergeinfo` is dropped-with-record and **never** a merge parent (no manufactured
  ancestry — the T-1 archetype for SVN); a convention-violating layout is recorded loudly with **no**
  fabricated ref. Reconstruction is opt-in, but **honesty is not**: turning reconstruction off yields only
  the stated spine; turning it on never yields an *unmarked* branch (C-1c). **Disposition:** accepted;
  three properties must be test-enforced — reconstructed refs count as derived on the fidelity surface
  (`derived.reconstructed-branch=N`), mergeinfo is never an ancestry edge, and a violated layout fabricates
  nothing.

- **F-7 — INV-6 holds, with one subprocess-specific determinism caveat named (INV-6, C-9).** Reading a
  dumpstream is deterministic and independent of the physical backend (svnadmin normalizes FSFS/BDB to one
  stream); revnum is encoded 8-byte big-endian for a stable topo tiebreak; `import_time = None`. **The
  caveat:** decoding the *same repository* on two hosts is byte-identical only for the *same* `svnadmin`
  version — a different version could order revprops or frame the stream differently. Decoding the *same
  dumpfile* is always identical. **Disposition:** accepted; treat the **dumpstream** as the canonical
  deterministic input (a determinism test pins a fixed dumpfile), and record the source form (live dump vs
  supplied dumpfile) in provenance so `verify --against-source` (VF-2) is well-defined. Recorded as
  **RR-svn-svnadmin-version**.

- **F-8 — INV-5 holds trivially: the boundary is not enlarged (INV-5, C-5).** With zero new crate
  dependencies there is nothing a downstream target could be forced to link; the IR and `verify --internal`
  stay consumable with the target's own surface, and the RFC 009 D-7 isolation test ships with the decoder.
  **Disposition:** accepted; the cleanest INV-5 case of any decoder so far.

## Guardrails this review binds to the implementation

1. **Untrusted-parser bounds are tested, not asserted** (F-3): malformed, oversized-length-declaring,
   deep-tree, and huge-directory-copy-fan-out dumpstreams each refuse cleanly and panic-free.
2. **INV-2 test** (F-4): a repository carrying a hook script (and a versioned hook-like file) decodes with
   the hook **never executed**; an `svn:externals` fixture is refused with a named reason.
3. **INV-1 tests** (F-6): reconstructed refs show as derived on the fidelity report; `svn:mergeinfo` never
   becomes a merge parent; a convention-violating layout produces no fabricated ref, only a loud record.
4. **INV-3 test** (F-5): a URL/remote source is refused; source-declared paths are never write targets.
5. **Determinism + isolation** (F-7/F-8): decode a fixed dumpfile twice → byte-identical; the IR +
   `verify --internal` build and run with no `brygge-decode-svn` present.
6. **Subprocess hardening** (F-2/F-5): `svnadmin dump` invoked with a fixed argument vector, no shell,
   read-only; the dumpstream treated as untrusted regardless of origin.
7. **Re-trigger condition** (F-4): a move to Tier L (`libsvn`/FFI), external resolution, or a richer `svn`
   subcommand re-runs this review.

## Threat-model delta (the gate requires the revisit)

**No new trust boundary of a new kind**, but one existing boundary gains a **new form** worth recording in
`brygge-03` at its next revision:

- **C-4b / INV-4 refinement.** The threat model says a C surface, if unavoidable, is confined to a
  *dedicated FFI crate*. Tier D demonstrates a **third, stronger option: confinement to a subprocess**,
  which isolates the C parser's memory-safety risk into a *separate address space* rather than brygge's own.
  Recommend C-4b read: "isolated to a dedicated FFI crate **or, preferably where available, to a
  subprocess**." This is a genuine strengthening the SVN review surfaced.
- **RR-svn-svnadmin (new residual).** brygge trusts the operator's `svnadmin` binary and its output (TB-4);
  a hostile or compromised `svnadmin` on the host could feed a false dumpstream. Mitigated by ASSUME-1 (the
  host is trusted) and by the dumpfile path (which bypasses the subprocess). Additionally, `svnadmin`
  parsing a hostile *repository* is a semi-trusted-tool surface (a TB-2 variant in subprocess form);
  operators should run brygge over untrusted sources in the same sandbox RR-1 recommends (container /
  restricted user / no ambient credentials).
- **RR-svn-svnadmin-version (new residual, F-7).** Decode of a live repository depends on the `svnadmin`
  version; named as a stated non-determinism (C-9) and mitigated by treating the dumpfile as the canonical
  input and recording the source form in provenance.

Fold the C-4b refinement and both residuals into `docs/src/brygge-03-threat-model` at its next revision
(the release touching this decoder **updates** the model, per the document's own closing rule).

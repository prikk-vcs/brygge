# Architect security review — the `brygge-decode-cvs` Tier R decoder (RFC 007)

**Required by** the GOVERNANCE security gate (a change that adds a source decoder and an untrusted-input
path must get an architect review against `brygge-03` and revisit it) and RFC 009 D-6. **Owner approval:**
not separately required — Tier R adds **no** dependency and **no** subprocess, so the owner's OQ-A tier
ruling (2026-09-10) stands as the dependency decision; this is the architect review the gate requires.
**Verdict:** **proceed to implementation**, with the controls below bound as tests. **Scope:** the trust
surface Tier R introduces — a new untrusted-input parser (RCS) and the changeset reconstructor — and how
brygge-03's invariants survive it. Not the reconstruction logic's fidelity (that is the program-design
handoff, `cvs-decoder-implementation-handoff-v1.md`). **Basis:** RFC 007 D-1…D-9 and that handoff.

## What enters the trust surface

| Metric | Value |
|---|---|
| New crate dependencies | **zero** — `brygge-ir` only; RCS `,v` files are uncompressed text |
| C toolchain / `*-sys` / FFI in the tree | **none** — the RCS reader is pure Rust; no RCS/CVS C library linked |
| New process executed at decode time | **none** — no `cvs`/`rcs` subprocess; the reader reads `,v` files directly |
| Network backend | **none**; a `:pserver:`/remote source is refused (INV-3) |
| New untrusted-input parsers | **two** — `rcs.rs` (the `,v` reader) and `cluster.rs` (the reconstructor), pure-Rust, `forbid(unsafe)` |

## Findings and dispositions

- **F-1 — The cleanest supply-chain surface of any decoder; T-4 scarcely applies (INV-4).** Zero new crates
  (contrast gix's 141 for Git), and — unlike SVN Tier D — **no subprocess either**. The audit surface for
  CVS is brygge's own pure-Rust code, full stop. **Disposition:** accepted; the strongest T-4/A-SUPPLY
  posture to date.

- **F-2 — Pure Rust end to end; the C-isolation question does not even arise (T-2/TB-2, C-4b).** SVN's
  format complexity forced a `svnadmin` subprocess to keep the C out of brygge's address space; RCS is
  simple enough to parse in pure Rust, so there is **no C to isolate** — no FFI crate and no subprocess.
  The threat-model's C-4b preference order (pure Rust > subprocess > FFI) lands on its first, best rung.
  **Disposition:** accepted; CVS spends none of the C-surface risk the gradient's other sources carry.

- **F-3 — brygge's whole defense is the two pure-Rust parsers, and both must be bounds-checked (T-2/T-8,
  C-2a/C-2d, INV-2).** The `,v` bytes are attacker-controllable (TB-1). `rcs.rs` must bound every `@`-string
  length, the delta-chain length, and the reconstructed size, and be panic-free (no `unwrap`/`expect`/
  indexing); `cluster.rs` must bound revision/file counts and the working set. These are C-2a/C-2d and are
  only real if test-enforced (handoff §6/§8, guardrail 1). Note two amplification vectors are absent **by
  construction**: RCS is uncompressed (no decompression bomb), and there is no delta *dump* to smuggle
  svndiff — the RCS reverse/forward diffs are the ed-style `a`/`d` form, bounded during application.
  **Disposition:** accepted, contingent on the §6 bounds shipping as tests.

- **F-4 — INV-2 holds: no source-provided code executes (INV-2, C-2b).** No `cvs`/`rcs` tool is run
  (there is no subprocess at all); `,v` content — including any script or `CVSROOT` hook definition stored
  as file content — is carried as **data**, never invoked; a `:pserver:`/remote source is refused (§ F-5),
  not contacted. **Disposition:** accepted, with the standing condition that any future tier driving the
  `cvs` CLI re-triggers this review for INV-2.

- **F-5 — INV-3 holds: no network; writes only where told (INV-3, C-6a/C-7).** The reader reads local `,v`
  files; a `:pserver:` or URL source is refused. CVS revision numbers, paths, and symbolic names are
  **data** — keys and `PathOp` paths in the IR — never write destinations (C-2c/C-7); the only writes are
  the operator's `--ir`/`--out`. **Disposition:** accepted.

- **F-6 — INV-1 holds, and CVS is its purest test (INV-1, C-1a…e).** CVS is the first source where the
  **atom itself is a judgment**: every `ChangeAtom` is `Derived(ReconstructedChangeset)` with its clustering
  params and confidence, and `honesty::summary` counts them, so the fidelity report is dominated by
  `derived.reconstructed-changeset` — uncertainty prominent, not buried (SRC-C2). There is **no
  manufactured verification anywhere**: reconstructed refs are `Derived`, under-floor changesets are
  flagged/refused rather than presented as certain, and — the sharpest point — **changeset-level VF-2 is
  honestly *declined*** (D-7): brygge does not offer a round-check it cannot honestly make, and says so on
  the surface and in the VF-5 statement before the run. This is INV-1 (no manufactured verification) at its
  strongest. **Disposition:** accepted; three properties must be test-enforced — every atom derived on the
  fidelity report, the per-changeset floor flags/refuses under-floor changesets, and no ref is fabricated
  where reconstruction is off.

- **F-7 — INV-6 is load-bearing here: reconstruction must be deterministic (INV-6, C-9, T-9).** Because the
  changeset grouping is a *heuristic*, non-determinism would not only break VF-1 — it would be a place a
  tamper could hide (T-9): two runs producing different groupings would make "faithful" uncheckable. The
  reconstructor must be a **pure, total function** of the per-file revisions and the recorded `window`, with
  a stable total tie-break (handoff §5b), so re-decode is byte-identical. This is more security-critical for
  CVS than for any prior source, because more of the output is derived. **Disposition:** accepted, contingent
  on a determinism test on a fixed fixture (guardrail 5) and the `window` recorded in provenance (PR-5).

- **F-8 — INV-5 holds trivially: the boundary is not enlarged (INV-5, C-5).** Zero new crate dependencies →
  nothing a downstream target could be forced to link; the IR and `verify --internal` stay consumable with
  the target's own surface, and the RFC 009 D-7 isolation test ships with the decoder. **Disposition:**
  accepted.

## Guardrails this review binds to the implementation

1. **Untrusted-parser bounds are tested** (F-3): a truncated / overlong-`@`-string / cyclic-or-overlong-delta
   `,v` each refuse cleanly and panic-free.
2. **INV-2 test** (F-4): a repository whose `,v` content includes a script-like/hook-like file decodes with
   nothing executed; a `:pserver:`/remote source is refused.
3. **INV-1 tests** (F-6): every atom shows `derived:reconstructed-changeset` on the fidelity report; an
   under-floor changeset is flagged/refused with a named reason; reconstruction off fabricates no ref.
4. **INV-3 test** (F-5): a `:pserver:` source is refused; source-declared paths are never write targets.
5. **Determinism + isolation** (F-7/F-8): decode a fixed fixture twice → byte-identical; the IR +
   `verify --internal` build and run with no `brygge-decode-cvs` present.
6. **Re-trigger condition** (F-4): a move to a `cvs`-CLI tier or an RCS/CVS C library re-runs this review.

## Threat-model delta (the gate requires the revisit)

**No new trust boundary and no new boundary *form*.** CVS Tier R adds neither a dependency nor a subprocess,
so unlike SVN Tier D it prompts no `C-4b`/`TB-2` refinement — it simply lands on C-4b's already-stated first
rung (pure Rust). One residual is worth recording:

- **RR-cvs-reconstruction (new residual).** A CVS changeset is **brygge's derived judgment**, not a source
  record; a consumer that *ignores the `Derived` atom status* could over-trust the grouping as if CVS had
  recorded it. This is the honesty-in-the-object invariant (INV-1/HO-4) doing exactly its job — the marking
  is *in* every atom, the fidelity report leads with it (SRC-C2), and the VF-5 faithfulness statement states
  the limit before the run — so the residual is "a consumer that discards the honesty brygge attached,"
  mitigated as far as brygge can mitigate it (it cannot force a downstream reader to honour a marking, only
  to receive it un-loseable). Record it in `brygge-03` at its next revision as the CVS-specific sharpening of
  the general derived-marking reliance, alongside `RR-2`.

Fold `RR-cvs-reconstruction` into `docs/src/brygge-03-threat-model` at its next revision (the release
touching this decoder **updates** the model, per the document's own closing rule).

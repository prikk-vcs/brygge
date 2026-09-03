# RFC 009 — Dependency-surface &amp; supply-chain policy

**Status.** Accepted (2026-09-04). Phase-A0, brought early because it **gates every decoder's library
choice** and is the project's defining risk: brygge exists precisely so prikk needn't take on heavy
source-parsing dependencies (PU-5). This RFC sets the policy for which dependencies are acceptable, how
they are isolated, how the source is read without linking risk where possible, and the CI gates that
keep the surface honest.
**Tracks.** ROADMAP Phase A0; requirements PU-5, §8 (`BN-5`), `CT-05`; threat model `T-2`, `T-4`, `T-5`,
`INV-2`, `INV-4`, `INV-5`; `GOVERNANCE.md` (owner approves new heavy deps). Track A.
**Touches.** The workspace's crate boundaries (which crate may link what), the `forbid(unsafe_code)`
posture, a `deny.toml`, the CI supply-chain gates, and the decision criteria every source RFC (004–007)
must apply.

## Summary

The heavy source-parsing libraries are simultaneously the enabling asset and the largest attack/audit
surface (`A-SUPPLY`, `T-4`). This RFC contains that surface: it **isolates** heavy dependencies behind
the per-source decoder crates, **prefers reading a source without linking a heavy or C library at all**,
keeps any C/FFI in a **single dedicated crate**, forbids executing source-provided code (`INV-2`),
requires that **nothing brygge emits enlarges a target's audited surface** (`INV-5/BN-5/CT-05`), and
wires **`cargo-deny` + `cargo-audit`** as CI gates. It does not pick each source's library — that is each
source RFC's decision — but it fixes the criteria and the guardrails they decide within.

## The constraints that scope this policy

- prikk's whole claim is verifiability on a **five-crate** audited surface; brygge must be able to hand
  prikk (or any target) an import that is checkable **without linking a single brygge dependency**
  (`BN-5/CT-05/INV-5`).
- A Git decoder needs `gix` (pure Rust, ~100 crates) or `libgit2` (C); SVN/CVS/hg have no mature
  pure-Rust *library*. So the dependency question is unavoidable and per-source.
- The source repository is **untrusted input** (`T-2/INV-2`): whatever reads it must not execute
  source-provided code and must be bounded.

## Decisions

- **D-1 — Isolation: heavy dependencies live only in `brygge-decode-<source>` crates.** `brygge-ir`, the
  verification path (`verify --internal`), and the encoders link **none** of them (`C-4a`). The CLI wires
  decoders in but the honesty/verify/IR core does not depend on them. This makes `INV-5/CT-05` structural
  one layer in, and is tested (RFC 001's "no heavy deps in `brygge-ir`" check, generalized: the IR +
  verify path build and run with no decoder crate present).
- **D-2 — A source-reading preference order** (each source RFC picks the highest feasible tier and
  justifies it):
  1. **A mature, pure-Rust library** — for **Git this is `gix`** (pure Rust, no C, `forbid(unsafe)`
     stays maximal, permissive license). Preferred.
  2. **A pure-Rust reader of the on-disk format** — e.g. **CVS** RCS `,v` files, and **Mercurial**
     revlogs, are documented formats readable without a library. Preferred where no mature library
     exists and the format is stable.
  3. **Driving the source's own CLI in a locked-down subprocess** — e.g. **SVN** (`svnrdump`/`svn`) or
     **hg** where reading the format directly is impractical. Permitted with the guardrails in D-4;
     avoids linking a C library at the cost of a runtime tool dependency (which is declared, not silent).
  4. **A C library isolated in a single dedicated FFI crate** (`brygge-ffi-<lib>`) — last resort, only
     with **owner approval** (`GOVERNANCE.md`); the one place `unsafe`/C exists, mirroring `prikk-ffi`.
- **D-3 — `forbid(unsafe_code)` everywhere except a dedicated FFI crate.** Every brygge crate forbids
  unsafe; if D-2 tier 4 is ever used, `unsafe` is confined to that one crate and nowhere else.
- **D-4 — Never execute source-provided code, and sandbox the untrusted read** (`INV-2/T-2`). No hooks,
  filters, submodule fetches, or scripts are run. When a tier-3 subprocess is used it runs with hooks
  and filters disabled, **no network**, and no ambient credentials (e.g. for Git-family tools: an empty
  hooks path, cleared filter/attribute processing, disabled protocols); the operator is advised to run
  untrusted-source imports in a sandbox (`RR-1`). brygge itself performs **no network I/O** (`INV-3`).
- **D-5 — License policy.** brygge is Apache-2.0; dependencies must be license-compatible. `gix`
  (MIT/Apache) is clean; **`libgit2` (GPLv2-with-linking-exception) and any copyleft C library are a
  reason to prefer tiers 1–3** and require explicit owner sign-off if adopted. The acceptable-license
  allowlist lives in `deny.toml`.
- **D-6 — Supply-chain gates in CI, and pinning.** `cargo-deny` (advisories, the license allowlist,
  banned/duplicate crates) and `cargo-audit` run in CI and **fail the build** on a new advisory or a
  disallowed license; exact versions are pinned and the lockfile is committed; the dependency set is kept
  as small as the mission allows. A new or upgraded heavy dependency requires **owner approval + an
  architect security review** and a threat-model revisit (`GOVERNANCE.md`, the security gate).
- **D-7 — The downstream boundary is a tested property** (`INV-5/BN-5/CT-05`): a test consumes a brygge
  IR/proposal and runs `verify --internal` with **none** of brygge's decoder/FFI dependencies present,
  proving a target checks a brygge import on its own surface alone.

## Open questions

- **OQ-A — hg and SVN tier choice.** Whether Mercurial is read via its revlog format (tier 2) or the
  `hg` CLI (tier 3), and whether SVN uses `svnrdump` (tier 3) or `libsvn` FFI (tier 4), are decided in
  RFCs 005/006 against D-2's criteria and the license/effort trade-off. This RFC only fixes the order and
  the guardrails.
- **OQ-B — Whether to vendor dependencies** for the strongest supply-chain guarantee, or rely on
  pinning + lockfile + gates. *Leaning:* pin + lockfile + gates first; consider vendoring if the audit
  posture demands it.

## Consequences

- Every source RFC inherits a clear decision framework and cannot, on its own, enlarge the C/`unsafe` or
  license surface without owner sign-off.
- The "brygge carries the weight, the target does not" property (`INV-5`) is enforced by crate boundaries
  and a test, not by hope.
- The CI gains supply-chain gates from the start, so the surface is watched before the first heavy
  dependency lands with the Git decoder (RFC 004).

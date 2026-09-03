# brygge — Roadmap

How brygge gets from a design set to a dependable migration tool. This is a **direction with
milestones**, not a dated schedule: work ships when it is correct, tested, and honest. Requirement and
design ids (e.g. `PR-4`, `IX-06`, `HO-1`, `INV-2`) refer to the design set in
[`docs/src/`](docs/src/) (`brygge-01` requirements, `brygge-02` external design, `brygge-03` threat
model). The governing upstream is prikk **RFC 113** (the import contract).

## Guiding rules (constant across the roadmap)

- **Design before implementation.** Requirements → external design → threat model → **RFC + handoff** →
  implementation → tests → example. Never inverted. (Owner's standing directive.)
- **Two tracks, deliberately decoupled.** The **decode → IR track** depends only on the (decades-stable)
  source systems and on nothing in prikk, so it is what brygge **stabilizes first** (PU-6). The
  **encode-to-prikk track** is gated on prikk's open decisions (UD-1…UD-3, OQ-1…OQ-3) and advances only
  as they land; it never blocks the first track.
- **Honesty is a security property, not a feature** (INV-1). No milestone ships a surface that could
  read as native/verified imported history.
- **Carry the weight at the boundary** (INV-4/INV-5). Heavy decoder deps stay isolated behind the
  decoder crates; brygge output is consumable/checkable with only the target's own surface.
- **The difficulty gradient is the build order** (requirements §7): Git → Mercurial → SVN → CVS, each
  de-risking the IR before the next stresses it.

---

## Track A — decode → IR (stabilize the first half)

The near-term product. Complete and useful with no encoder and no prikk (PU-1).

### Phase A0 — Foundations (the IR and the tool spine)
The substrate every decoder and encoder shares. No source-specific parsing yet.
- The **IR internal representation** satisfying `IR-1…IR-6` / `IX-01…07`: faithfulness-with-provenance,
  per-atom epistemic status, opaque source ids first-class, the loss boundary, encoder-agnostic,
  versioned (RFC 001).
- The **honesty machinery** (RFC 002): derived-vs-stated marking, loss-boundary recording, and the
  fidelity summary **recoverable from the objects** (HO-1/HO-2/HO-4, FS-02).
- **Determinism, the IR artifact format, versioning, and an integrity digest** (RFC 003): `VF-1`,
  `IX-07`, and the tamper-detectability the threat model needs (C-3b).
- The **dependency-surface & supply-chain policy** (RFC 009, security-foundational, brought early):
  `gix` vs `libgit2`, FFI isolation, `cargo-deny`/`cargo-audit` gates (INV-4).
- The **tool spine**: `decode`/`inspect`/`verify`/`summary` command surface (CL-*), machine-readable
  output (CL-07), outcome-class exit codes (CL-08).

### Phase A1 — Git decoder → **the first stable decode/IR deliverable**
- Decode Git → IR: content/ancestry/messages as claims (PR-1/2/3); commit SHAs and GPG signatures
  preserved opaquely (PR-4); **every inferred rename marked derived** with its parameters (HO-1);
  identity inference lives in the (later) encoder, visibly, not hidden in the IR (IR-1).
- `inspect`, `verify --internal` (VF-3), and `verify --against-source` (VF-2, the round-check).
- The floor **mechanism** (CF-03) with refusal-with-reason (FA-3); the floor's **contents** for Git are
  owner-set (OQ-3).
- **This phase reaches a durable decode/IR contract for Git — the "first half stabilized."**

### Phase A2 — Mercurial decoder
- Validates the IR's cross-source claim (IX-06) with an epistemically **different** source: hg often
  **states** renames (SRC-H2), so hg imports carry fewer derived marks than Git — a visible, checkable
  consequence. Named branches vs bookmarks, phases, obsmarkers handled per SRC-H3.
- Proves source-extensibility: a new source is a new decoder against the unchanged IR (PU-3).

### Phase A3 — SVN decoder
- Branch identity reconstructed by convention as **derived** records (SRC-S1); mergeinfo
  dropped-with-record or carried-as-advisory, never promoted (SRC-S2). Stresses derived-branch discipline.

### Phase A4 — CVS decoder (honest, lossy, labelled)
- Changeset reconstruction by clustering, every changeset marked derived (SRC-C1/C2); the surface states
  **before running** that a VF-2-faithful import is not achievable (SRC-C3/FS-06). The honesty stress test.

## Track B — encode → target (gated; runs in parallel where it can)

### Phase B0 — prikk **reviewable proposal** (buildable now, unsealed)
- `encode prikk` emits a **labelled, unsealed, `Unverifiable`** proposal with provenance content (PX-*),
  in a clearly-interim form (RFC 008). No seal path, no `Import` block prikk would reject today
  (GATED-1/2/3). Useful for review immediately; safe because the target admits nothing.

### Phase B1 — real prikk imports (gated on prikk)
- Advances only as prikk lands **UD-1** (an import-shaped `Attestation`), **UD-2** (an authorized import
  block kind or a `Normal`-block ruling), **UD-3/OQ-2** (whether imports may be sealed and by whom),
  **OQ-1** (what the importer signs), and format stability (**UD-5**). Each is owner/prikk territory;
  brygge tracks them and targets whatever prikk settles.

### Phase B2 — a second target encoder
- Proves PU-3: a non-prikk target's encoder written against the IR alone, no brygge change. Optionally a
  snapshot target, to prove the IR privileges no identity model (IX-05).

---

## Milestones & versions

| Milestone | Version | Contents | Track |
|---|---|---|---|
| **M0** | 0.1.0-dev | Foundations: IR contract v1, honesty machinery, determinism+integrity, dep policy + supply-chain gates, tool spine | A0 |
| **M1** | **0.1.0** | **Git decode → IR + inspect + verify (internal & against-source).** The first stable decode/IR deliverable | A1 |
| **M2** | 0.2.0 | Mercurial decoder; IR cross-source claim validated | A2 |
| **M3** | 0.3.0 | SVN decoder | A3 |
| **M4** | 0.4.0 | CVS decoder (lossy, labelled) | A4 |
| **B0** | ships within 0.x once M1 lands | prikk reviewable-proposal encoder (unsealed) | B0 |
| **IR-1.0** | — | The **IR contract is frozen** once M1 proves it and M2 validates it cross-source; thereafter additive-only (see release cycles) | A |
| **1.0.0** | 1.0 | Decode/IR half stable across ≥ 2 sources + prikk proposal encoder + IR contract frozen. (Sealed prikk imports may still be gated — 1.0 is the *decode/IR product's* stability, not the gated encoder's.) | A + B0 |

The **IR contract version (IX-07) is a first-class compatibility promise, separate from the tool
version** — a consumer (a foreign encoder, an inspector) pins the IR contract, not the brygge binary.

---

## Release cycles

- **Milestone-driven minors.** Each source completes a minor (0.1 Git, 0.2 hg, 0.3 SVN, 0.4 CVS). A
  minor ships only when its source's decode + inspect + both verify modes are green and its fidelity
  surface is honest per source (VF-5).
- **The IR contract has its own semver, tracked in the IR artifact.** Pre-freeze (before IR-1.0): a
  minor may change the IR contract with a version bump and a stated migration. Post-freeze: the IR
  contract is **additive-only**; a breaking change is a new major of the contract, deliberate and rare.
- **Security releases are out-of-band.** A dependency advisory (`cargo-audit`/`cargo-deny`, C-4d) or a
  threat-model control failure triggers a prompt patch release; the threat model is revisited per the
  project rule (a release touching a new parser, a new dependency, or an untrusted-input path **updates**
  `brygge-03`; others **re-verify** it).
- **Tags are bare versions (no `v`)**, gates are CI-enforced, and the release mechanics mirror the
  ecosystem's other projects (bare-version tag → gate → release). Publishing/tagging is **owner-only**
  (see `GOVERNANCE.md`).
- **What "done" means for a release:** the gates green (fmt · clippy `-D warnings` · test · **supply-chain
  gates**), the fidelity/honesty surfaces present and unsuppressible (INV-1), and — for any release
  touching untrusted input or dependencies — the threat model updated.

---

## Dependencies on prikk (do not block Track A)

brygge names these so no plan silently assumes them; all are prikk/owner territory (RFC 113 §4a) and gate
only Track B past B0:

- **UD-1** import-shaped `Attestation`; **UD-2** authorized import block kind; **UD-3/OQ-2** sealing
  ruling; **OQ-1** importer signing (DC-35); **OQ-3** the per-source floor contents; **UD-5** format
  stability + sync. The UD table in `brygge-01` §11 is to be **re-verified against the current prikk**
  (now 0.28) when Track B design begins.

## What to build first

Per RFC 113 §4a and the gradient: **RFC 001 (IR foundations) and RFC 009 (dependency policy) first**,
then RFC 002/003 (honesty + determinism), then the **Git decoder (RFC 004)** to reach M1. Nothing in
Track A waits on prikk. The architect writes each RFC + handoff; the implementer builds against it.

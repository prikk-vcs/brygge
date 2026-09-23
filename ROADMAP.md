# brygge — Roadmap

How brygge gets from a design set to a dependable migration tool. This is a **direction with
milestones**, not a dated schedule: work ships when it is correct, tested, and honest. Requirement and
design ids (e.g. `PR-4`, `IX-06`, `HO-1`, `INV-2`) refer to the design set in
[`docs/src/`](docs/src/) (`brygge-01` requirements, `brygge-02` external design, `brygge-03` threat
model). The governing upstream is prikk **RFC 113** (the import contract; accepted, owner-ruled
2026-09-13).

> **Status (2026-09-23): Track A is built, not yet released.** All four sources (Git, hg, SVN, CVS) decode
> into the IR, but **no version of brygge has been released**. The incoming architect's intake review
> found defects in correctness, honesty and contract evolution that must be fixed before a first release.
> The owner has authorized the release plan below: a correction cycle ending in **0.1.0, the first
> published release**. brygge is in **v0 development**: breaking changes made to improve or fix it are
> acceptable. Track B (encode → prikk): **prikk decides, brygge informs proactively**. brygge sends its
> import requirements to prikk; the encoder is built once prikk's import foundations exist (see Track B).
> For the invariants, the architecture and how to build, read
> [`HANDOFF.md`](docs/src/development/handoffs/HANDOFF.md).

## Guiding rules (constant across the roadmap)

- **Design before implementation.** Requirements → external design → threat model → **RFC + handoff** →
  implementation → tests → example. Never inverted. (Owner's standing directive.)
- **The owner's design philosophy decides close calls:** *finally clean, safe and secure, robust and
  sophisticated design*. APIs and the user experience must not let anyone be confused or misunderstand:
  one name per concept, nothing silently ignored, approximated or implied.
- **Two tracks, deliberately decoupled.** The **decode → IR (intermediate representation) track** depends
  only on the (decades-stable) source systems and on nothing in prikk, so it is what brygge stabilizes
  first (PU-6). The **encode-to-prikk track** follows prikk; it never blocks the first track.
- **prikk decides; brygge informs, proactively.** prikk is the primary project and owns its import
  contract. brygge knows an importer's requirements best, so it **proactively** sends prikk its
  requirements (with rationale and evidence) and its questions, whenever they are needed.
  - It states needs, constraints and, where useful, options with trade-offs.
  - It never presses prikk to design in the direction brygge would like, never presents a brygge design as
    prikk's, and never ships anything that could set a precedent for prikk (prikk RFC 113 §6).
  - brygge then conforms to what prikk decides.
  - Letters are sent only with the owner's authorization.
- **Honesty is a security property, not a feature** (INV-1). No milestone ships a surface that could
  read as native/verified imported history.
- **Carry the weight at the boundary** (INV-4/INV-5). Heavy decoder deps stay isolated behind the
  decoder crates; brygge output is consumable/checkable with only the target's own surface.
- **The difficulty gradient is the build order** (requirements §7): Git → Mercurial → SVN → CVS, each
  de-risking the IR before the next stresses it.

---

## Track A — decode → IR (stabilize the first half)

The near-term product. Complete and useful with no encoder and no prikk (PU-1).

### Phase A0 — Foundations (the IR and the tool spine) — built
The substrate every decoder and encoder shares.
- The **IR internal representation** satisfying `IR-1…IR-6` / `IX-01…07`: faithfulness-with-provenance,
  per-atom epistemic status, opaque source ids first-class, the loss boundary, encoder-agnostic,
  versioned (RFC 001).
- The **honesty machinery** (RFC 002): derived-vs-stated marking, loss-boundary recording, and the
  fidelity summary **recoverable from the objects** (HO-1/HO-2/HO-4, FS-02).
- **Determinism, the IR artifact format, versioning, and an integrity digest** (RFC 003): `VF-1`,
  `IX-07`, and the tamper-detectability the threat model needs (C-3b).
- The **dependency-surface & supply-chain policy** (RFC 009): `gix` vs `libgit2`, FFI isolation,
  `cargo-deny`/`cargo-audit` gates (INV-4).
- The **tool spine**: the command surface (CL-*), machine-readable output (CL-07), outcome-class exit
  codes (CL-08).

### Phase A1 — Git decoder — built (M1)
- Content, ancestry and messages as claims (PR-1/2/3); commit SHAs and signatures preserved opaquely
  (PR-4); inferred renames marked derived with their parameters (HO-1); against-source verification
  (VF-2); the owner-ratified floor (FA-3).

### Phase A2 — Mercurial decoder — built (M2)
- Validates the IR's cross-source claim (IX-06) with a source that often **states** renames (SRC-H2).
  Named branches vs bookmarks, phases, obsmarkers handled per SRC-H3.

### Phase A3 — SVN decoder — built (M3)
- Branch identity reconstructed by convention as **derived** records (SRC-S1); mergeinfo
  dropped-with-record, never promoted (SRC-S2).

### Phase A4 — CVS decoder (honest, lossy, labelled) — built (M4)
- Changeset reconstruction by clustering, every changeset marked derived (SRC-C1/C2); the surface states
  **before running** that a VF-2-faithful import is not achievable (SRC-C3/FS-06).

## Track B — encode → target (prikk decides, brygge informs)

The owner's rulings of 2026-09-23 replace the earlier B0 "interim proposal" plan. An interim, prikk-shaped
format from brygge would be built to be thrown away, and it could set a precedent for prikk's own import
design (prikk RFC 113 §6).

### Phase B0 — requirements to prikk (proactive, no code)
- Once RFC 011 fixes the IR's provenance content, brygge drafts **letter 001 to prikk**: what an importer
  needs from prikk's import contract, stated as requirements with rationale, plus open questions. Topics:
  - the provenance an import declaration must carry;
  - deterministic import-time fields (UD-4);
  - who the importer is, and where its signing key lives (RFC 113 §4.3–§4.4);
  - a floor pre-flight that reports every refusal before writing;
  - personal data in author identities.
- It is sent with the owner's authorization. prikk decides if, when and how to answer.

### Phase B1 — the prikk encoder (RFC 008)
- **Starts** once prikk's architect has accepted the RFC 113 foundations design (after the owner schedules
  prikk's import theme, prikk ROADMAP theme 17).
- brygge's RFC 008 then **conforms** to that design.
  - brygge's own standing constraint (BN-4) is that it holds no prikk maintainer key. How and by whom
    the import declaration is signed is prikk's decision; letter 001 asks.
  - prikk's floor (RFC 113 §4.5) is enforced by the encoder as a pre-flight that reports every refusal
    before anything is written.
- Gaps found while conforming go back to prikk as further letters (requirements or questions), never as
  a proposed prikk design.

### Phase B2 — a second target encoder
- Proves PU-3: a non-prikk target's encoder written against the IR alone, with no brygge change.
  Optionally a snapshot target, to prove the IR privileges no identity model (IX-05).

---

## Release plan (authorized by the owner 2026-09-23)

| Release | Theme | Scope | Entry | Exit (in addition to the gate suite) |
|---|---|---|---|---|
| **0.1.0** — first published release | **Honest decode** | The intake review's correction set: the `verify --internal` checks, CVS mainline-only with its user guidance, text/path integrity, non-history exclusion, resource bounds, output neutralization, the three-verb CLI (`decode`/`inspect`/`verify`), the narrowed public API; **RFC 011 (the IR contract re-cut)**; per-source user guides (the published VF-5 statements); threat model v0.3; `CHANGELOG.md` | The owner's rulings on the intake review (done 2026-09-23) | Every correction closed with its tests; RFCs 001–007 and 009 moved to `done/` as "Implemented (0.1.0)"; release notes; the owner authorizes the cut |
| **0.2.0** | **Scale** | RFC 010 increments 2 (SVN dumpstream iterator) and 3 (CVS reconstruction bound); a new increment bounding the Git snapshot cache; a Git scenario in `tools/bench`; increment 4 only if measured | 0.1.0 released | Before/after measurements recorded; byte-identical output |
| **0.3.0** | **Source reach** | CVS branch-aware import (lifts 0.1.0's mainline-only limit); SVN delta dumps (svndiff); hg hashed long paths; CVS adaptive clustering windows | 0.2.0 released; per-item RFC amendment and security review | Each lifted limit has fixtures against real tools |
| **B0** | **Requirements to prikk** | Letter 001: an importer's requirements and questions (no code) | RFC 011 accepted | The owner authorizes sending |
| **B1** | **prikk encoder** | RFC 008, conforming to prikk's import foundations | prikk's foundations accepted (see Track B) | Per RFC 008 |
| **1.0.0** | — | The owner's decision alone. Its criteria are to be restated when 1.0 is discussed (the former "prikk proposal encoder (B0)" criterion was withdrawn with B0) | — | — |

**0.1.0 also includes the hg *published view*:** secret and hidden changesets are excluded by brygge
itself, and counted in the report, rather than refused (owner ruling D-4, 2026-09-23).

## Milestones (built, not released)

| Milestone | Contents | Status |
|---|---|---|
| **M0** | Foundations: IR contract, honesty machinery, determinism + integrity, dependency policy + supply-chain gates, tool spine | built |
| **M1** | Git decoder + inspect + verify (internal & against-source) | built |
| **M2** | Mercurial decoder; IR cross-source claim validated | built |
| **M3** | SVN decoder | built |
| **M4** | CVS decoder (lossy, labelled) | built |
| **IR contract** | Frozen at 1.0.0 on 2026-09-08 as a **pre-release** freeze. It is re-cut by RFC 011 before the first release (owner ruling D-1, 2026-09-23); the released label is settled with RFC 011 | re-cut in 0.1.0 |

The **IR contract version (IX-07) is a first-class compatibility promise, separate from the tool
version**: a consumer (a foreign encoder, an inspector) pins the IR contract, not the brygge binary.

---

## Release cycles

- **v0 policy (owner, 2026-09-23).** brygge has never been in production use. Until 1.0, breaking changes
  made to improve or fix it are acceptable, and are stated in the release notes.
- **One minor per theme.** Patch releases for fixes that change no scope. Each release has entry and exit
  criteria. The architect reports readiness with a release recommendation; **the owner authorizes every
  cut, tag and publication.**
- **The IR contract has its own version, carried in every artifact.** How it evolves (tagged fields with
  a critical bit: an unknown critical field is refused, an unknown non-critical field is skipped and
  reported) is specified by RFC 011.
- **Security releases are out-of-band.** A dependency advisory (`cargo-audit`/`cargo-deny`, C-4d) or a
  threat-model control failure triggers a prompt patch release. The threat model is revisited per the
  project rule: a release touching a new parser, a new dependency, or an untrusted-input path **updates**
  `brygge-03`; others **re-verify** it.
- **Tags are bare versions (no `v`)** and gates are CI-enforced. Publishing and tagging are **owner-only**
  (see [`GOVERNANCE.md`](docs/src/development/handoffs/GOVERNANCE.md)).
- **What "done" means for a release:**
  - the gates are green (fmt · clippy `-D warnings` · test · **supply-chain gates**);
  - the fidelity/honesty surfaces are present and unsuppressible (INV-1);
  - for any release touching untrusted input or dependencies, the threat model is updated;
  - RFCs shipped in the release move to `done/`.

---

## Dependencies on prikk (do not block Track A)

Re-verified against prikk 0.46.0 on 2026-09-23:

- **Ruled by prikk's owner, 2026-09-13 (RFC 113 §4.3–§4.5):**
  - OQ-1 — the importer signs the import declaration;
  - OQ-2/UD-3 — only an adopted maintainer seals imported history;
  - OQ-3 for Git — refuse, never approximate.
- **Met:** UD-5, format stability (prikk RFC 114) and sync.
- **Still unbuilt in prikk:**
  - UD-1 — an import-shaped `Attestation`;
  - UD-2 — an authorized `Import` block kind;
  - UD-4 — deterministic import-time fields.
- **prikk's import theme (theme 17) is unscheduled.** brygge informs it proactively (Track B0); the
  encoder waits for it (Track B1).

`brygge-01` §11 is updated to this state in the 0.1.0 documentation sweep.

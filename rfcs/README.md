# brygge RFCs

Design decisions for brygge are recorded as RFCs, following the ecosystem's **five-folder lifecycle**
(the same one prikk and stikk use; the canonical policy is `done/000-rfc-lifecycle-policy.md`).

> New to brygge? Start at [`../docs/src/development/handoffs/HANDOFF.md`](../docs/src/development/handoffs/HANDOFF.md)
> for the whole map; this file is the authoritative record of RFC **state** (the folder an RFC lives in is
> the source of truth).

```
proposed/   a decision drafted for review, not yet settled
accepted/   settled — an implementer may build against it (each gets a handoff under handoffs/NNN-slug/)
done/       shipped — kept forever, never deleted
archive/    withdrawn or superseded, kept for the record
```

An RFC moves `proposed → accepted` when its design is settled and the owner has approved any decision
that is the owner's (see `GOVERNANCE.md`); `accepted → done` when the work ships. Every accepted RFC
gets a **program-design handoff** under `rfcs/handoffs/NNN-slug/` before implementation.

**The upstream contract is prikk RFC 113** (*History import foundations*), which lives in the prikk
repository, not here. brygge's RFCs realize the *decoder/IR/encoder* side of that contract; RFC 113
§4a's owner-open questions (OQ-1…OQ-3) were **ruled 2026-09-13** — see `ROADMAP.md`'s Track B for what
gates brygge's encode-to-prikk work now (prikk's own foundations being accepted).

## RFC index (planned)

Numbering is brygge's own. **Track A** (decode → IR) depends on nothing in prikk and is buildable now;
**Track B** (encode → prikk) past a reviewable proposal is gated on prikk's own foundations being
accepted (the owner's OQ-1…OQ-3 are already ruled; see `ROADMAP.md` Track B). Build order follows the
difficulty gradient (requirements §7).

| RFC | Scope | Track / Phase | Gate |
|---|---|---|---|
| **001** | **IR foundations & obligations** — the intermediate representation satisfying `IR-1…IR-6` / `IX-01…07`: faithfulness-with-provenance, per-atom epistemic status, opaque source ids, the loss boundary, encoder-agnostic, versioned | A0 | none — **first** |
| **002** | **Honesty & provenance machinery** — derived-vs-stated marking, loss-boundary recording, the fidelity summary recoverable from the objects (HO-1/2/4, FS-02) | A0 | none |
| **003** | **Determinism, IR artifact format & versioning, integrity digest** — VF-1, IX-07, tamper-detectability (C-3b) | A0 | none |
| **009** | **Dependency-surface & supply-chain policy** — `gix` vs `libgit2`, FFI isolation, `cargo-deny`/`cargo-audit` gates (INV-4/INV-5) — *security-foundational, brought early* | A0 | none — **early** |
| **004** | **Git decoder** — identity inference (renames marked derived), the floor mechanism | A1 | decode: none · floor contents: OQ-3 |
| **005** | **Mercurial decoder** — stated renames (SRC-H2); named-branch/bookmark/phase/obsmarker handling | A2 | decode: none · floor contents: OQ-3 |
| **006** | **Subversion decoder** — branch reconstruction by convention (derived), mergeinfo discipline | A3 | decode: none · floor contents: OQ-3 |
| **007** | **CVS decoder** — changeset reconstruction, lossy-but-labelled verdict (SRC-C3) | A4 | decode: none · floor contents: OQ-3 |
| **008** | **prikk encoder** — reserved; conforms to prikk's import foundations once they exist (ROADMAP Track B1) | B1 | prikk theme 17 |
| **011** | **IR contract re-cut** — tagged records with a critical bit, a strict canonical form, typed flags, copies with their source point, text as bytes, carried extras (owner ruling D-1) | 0.1.0 | accepted; OQ-A ruled (`0.2.0`) |

The list will grow (a second-target encoder RFC to prove PU-3; further sources under "etc."). Each RFC
is written by the architect and reviewed/approved per `GOVERNANCE.md`.

## State

**brygge 0.1.0 is released (2026-09-24): all four sources on the gradient decode into IR contract
`0.2.0` (RFC 011).** The RFCs that 0.1.0 implements are in `done/`. RFC 010 stays accepted: its first
increment (the input ceilings) shipped in 0.1.0, and increments 2–4 (streaming) are planned for 0.2.0.
Per the lifecycle policy, **the folder an RFC lives in is the source of truth for its state**. This
table is the index the policy asks each project to keep, grouped by state as RFC 000 recommends. Update it
in the same commit that moves an RFC between folders.

### Accepted

| RFC | Scope | Handoff(s) |
|---|---|---|
| [010](accepted/010-bounded-memory-and-streaming.md) | Bounded memory & streaming (increment 1 shipped in 0.1.0; increments 2–4 in 0.2.0) | `handoffs/010-bounded-memory-and-streaming/` |

### Done

| RFC | Scope | Handoff(s) |
|---|---|---|
| [000](done/000-rfc-lifecycle-policy.md) | RFC lifecycle policy: brygge uses the **5-folder variant** (`proposed → accepted → done`, plus `archive/` and optional `draft/`) | — |
| [001](done/001-ir-foundations.md) | IR foundations & obligations. Implemented (0.1.0) | `handoffs/001-ir-foundations/` |
| [002](done/002-honesty-and-provenance-machinery.md) | Honesty & provenance machinery. Implemented (0.1.0) | (folded into 001's handoff) |
| [003](done/003-determinism-format-and-versioning.md) | Determinism, IR artifact format & versioning, integrity digest. Implemented (0.1.0), as re-cut by RFC 011 | (folded into 001's handoff) |
| [004](done/004-git-decoder.md) | Git decoder. Implemented (0.1.0) | `handoffs/004-git-decoder/` |
| [005](done/005-mercurial-decoder.md) | Mercurial decoder. Implemented (0.1.0) | `handoffs/005-mercurial-decoder/` |
| [006](done/006-subversion-decoder.md) | Subversion decoder. Implemented (0.1.0) | `handoffs/006-subversion-decoder/` |
| [007](done/007-cvs-decoder.md) | CVS decoder. Implemented (0.1.0) | `handoffs/007-cvs-decoder/` |
| [009](done/009-dependency-surface-and-supply-chain-policy.md) | Dependency-surface & supply-chain policy. Implemented (0.1.0) | `handoffs/009-dependency-surface-and-supply-chain-policy/` |
| [011](done/011-ir-contract-recut.md) | IR contract re-cut before the first release (supersedes parts of 001/002/003; contract `0.2.0`). Implemented (0.1.0) | `handoffs/011-ir-contract-recut/` |

### Proposed

None currently.

### Archive

None currently.

### Reserved / upcoming

- **008 — prikk encoder.** Reserved (not yet written); conforms to prikk's import foundations once they
  exist (ROADMAP Track B1, gated on prikk's own foundations being accepted — the owner's OQ-1…OQ-3 are
  already ruled).

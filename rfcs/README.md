# brygge RFCs

Design decisions for brygge are recorded as RFCs, following the ecosystem's **five-folder lifecycle**
(the same one prikk and stikk use; the canonical policy is `done/000-rfc-lifecycle-policy.md`).

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
§4a's owner-open questions (OQ-1…OQ-3) gate brygge's encode-to-prikk work.

## RFC index (planned)

Numbering is brygge's own. **Track A** (decode → IR) depends on nothing in prikk and is buildable now;
**Track B** (encode → prikk) past a reviewable proposal is gated on prikk (UD-1…UD-3) and the owner
(OQ-1…OQ-3). Build order follows the difficulty gradient (requirements §7).

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
| **008** | **prikk encoder** — the reviewable-proposal form now; sealed imports later | B0 now; B1 gated | UD-1/UD-2/UD-3, OQ-1/OQ-2 |

The list will grow (a second-target encoder RFC to prove PU-3; further sources under "etc."). Each RFC
is written by the architect and reviewed/approved per `GOVERNANCE.md`.

## State

- **Accepted:** [RFC 001 — IR foundations](accepted/001-ir-foundations.md) (with a handoff under
  [`handoffs/001-ir-foundations/`](handoffs/001-ir-foundations/ir-foundations-handoff-v1.md)) — design
  settled; the implementer may build `brygge-ir` against it.
- **Done:** [RFC 000 — RFC lifecycle policy](done/000-rfc-lifecycle-policy.md) (brygge uses the
  **5-folder variant**: `proposed → accepted → done`, plus `archive/` and optional `draft/`).

Per the lifecycle policy, the folder is the source of truth for state; this section is the index the
policy asks each project to keep. Update it in the same commit that moves an RFC between folders.

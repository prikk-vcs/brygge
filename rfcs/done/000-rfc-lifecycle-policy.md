# RFC 000 — RFC lifecycle policy (adopted)

**Status.** Done (adopted at project start). brygge uses the ecosystem's five-folder RFC lifecycle,
identical in shape to prikk's and stikk's, so a contributor moving between projects finds the same
process.

## The five folders

- **`proposed/`** — a decision drafted for review. It records the problem, the options, and a
  recommendation, but nothing may be implemented against it yet.
- **`accepted/`** — the design is settled and any owner-level decision it needed has been made (see
  `GOVERNANCE.md`). An implementer may now build against it. Each accepted RFC has a **program-design
  handoff** under `rfcs/handoffs/NNN-slug/` written *before* code.
- **`done/`** — the work has shipped. Done RFCs are **kept forever, never deleted**; they are the
  project's decision record.
- **`archive/`** — a proposal withdrawn or superseded, kept for the record with a note pointing to what
  replaced it.

## Movement

`proposed → accepted → done` is the normal path; `proposed → archive` when a proposal is dropped. An RFC
is never edited to erase a decision; a changed decision is a new RFC (or an explicit amendment section,
dated) so history stays legible.

## What needs an RFC

Any decision that is not obvious from the design set: a new IR obligation's realization, a new source
decoder, the dependency/supply-chain policy, the encoder form, the artifact format. Routine
implementation that follows an accepted handoff does not.

## Relationship to the design set

The design set (`docs/src/brygge-01/02/03`) is the standing contract. An RFC records a *decision* that
realizes or refines it; where an RFC and the design set disagree, the design set is corrected first (or
the RFC explicitly amends it). Tests validate design-set items; RFCs explain why the code is shaped as
it is.

*This policy mirrors the canonical `.git-exclude/rules/000-rfc-lifecycle-policy.md`; where they differ,
the canonical rules govern.*

# RFC 001 — IR foundations: the intermediate representation

**Status.** Proposed (2026-09-03) — the first brygge RFC, and the substrate every decoder and encoder
shares. Defines *what the IR is*: its logical model, how epistemic status and provenance are carried,
why it holds evidence-for-identity rather than identity, and the shape of its durable artifact.
Handoff:
[`../handoffs/001-ir-foundations/ir-foundations-handoff-v1.md`](../handoffs/001-ir-foundations/ir-foundations-handoff-v1.md).
**Tracks.** ROADMAP Phase A0 (foundations); requirements §10 (`IR-1…IR-6`), external design §2.2
(`IX-01…07`); prikk RFC 113 §3.1 and §4.2 (the architect's IR rulings). Track A — depends on nothing in
prikk.
**Touches.** A new `brygge-ir` crate (the light core: the IR model, the artifact format, integrity, and
the honesty machinery); the workspace layout; the read/write boundary the CLI and every decoder/encoder
use. No source parsing and no target encoding here — those are later RFCs built *on* this.

## Summary

brygge's value flows through the IR: decoders write it, `inspect`/`verify` read it, encoders consume it,
and a foreign encoder for a second target depends only on it (PU-3). This RFC fixes the IR's shape so
those four uses are possible and so the honesty disciplines (`HO-1/HO-2/HO-4`) are *properties of the
representation*, not conventions a decoder might forget. It is internal design — where the requirements
and external design deliberately stopped short of a schema (they are obligations and a black-box
contract), this RFC and its handoff define the model.

The one idea the whole IR turns on, from RFC 113 §3.1 and requirement `IR-1`: **design for
faithfulness-with-provenance, not neutrality.** A neutral IR converges on "snapshots plus metadata" —
exactly what prikk is not — and has nowhere to record that identity inference happened. So the IR
records **what each source literally guaranteed, plus a marked place for what a decoder or encoder
derived** — and it carries *evidence for* identity (stated and inferred renames) rather than identity
itself, leaving a node-identity encoder to author identity visibly (`IR-6`, `IX-05`).

## The constraints that scope this design

- **The atom differs per source and is epistemically different even when it looks the same** (RFC 113
  §3.1): a Git commit, an SVN revision, a reconstructed CVS changeset, an hg changeset. The IR must hold
  them in one frame *and say which kind of thing each is* (`IR-2`).
- **Derived ≠ stated, marked at the operation** (RFC 113 §4.2, `HO-1`): a reader tells an inferred
  rename from a source-stated one without re-running the heuristic.
- **The boundary of loss is itself recorded** (RFC 113 §3.1 binding ruling, `HO-2/IR-4`).
- **Source opaque ids and signatures are first-class** (`PR-4/IR-3`) — the only cryptographic link back
  to the source (`VF-2`).
- **Encoder-agnostic** (`IR-6/IX-05`): no prikk `NodeId` baked in; a snapshot target and a node-identity
  target both encode from the same IR.
- **Durable, inspectable, versioned, deterministic, integrity-checkable** (`IX-01/IX-07`, `VF-1`,
  `C-3b`), and — a security constraint (`INV-4/C-4a`) — the IR core links **none** of the heavy decoder
  dependencies, so `verify --internal` (`VF-3`) runs without them.

## Decisions

- **D-1 — The IR is a content store plus a graph of change-atoms plus per-import records.**
  - a **content store** of content-addressed blobs (file bytes at each state — `PR-1`); content-address
    gives dedup, integrity, and determinism for free.
  - a **DAG of change-atoms** (`PR-2`): each atom has an ordered parent list, a set of **path
    operations**, metadata-as-claims (author/committer/message/time — `PR-3`), the atom's **source
    opaque ids and signatures** (`PR-4`), and an **atom-level epistemic status**.
  - top-level **ImportProvenance** (`PR-6`) and **LossBoundary** (`HO-2/IR-4`) records, one per import.
- **D-2 — Epistemic status is a field on every assertion, not just on atoms** (`IR-2/IX-02`, RFC 113
  §4.2). A value is `Stated` (the source recorded it) or `Derived { by, decoder_version, params,
  confidence }`. A reconstructed CVS changeset is `Derived` at the atom level while its file contents are
  `Stated`; an hg `hg mv` rename is `Stated` while a Git similarity rename is `Derived`.
- **D-3 — The IR records the source's literal operations as `Stated`, and any inference as a *separate,
  `Derived`* annotation that never overwrites the stated fact.** A Git rename is stored as `Stated`
  delete + `Stated` create, *plus* an optional `Derived` rename-hint (`from`, `to`, confidence, params)
  — not as a single "rename" that hides the source's actual delete+create. This is the concrete form of
  D-2 and the anti-laundering guarantee at the model level (`HO-1`, threat `T-1`).
- **D-4 — The IR carries *evidence for* identity, never identity** (`IR-1/IR-6/IX-05`). No prikk
  `NodeId`. A snapshot encoder reads the path→blob trees directly; a node-identity encoder reads the
  operations + rename-hints and **authors** node identity itself, recording *its* inference as `Derived`
  in the target — so identity inference is the encoder's visible judgment, never pre-baked in the IR.
- **D-5 — The artifact is canonical, content-addressed, versioned, and integrity-digested.**
  Deterministic serialization (fixed field order, sorted maps, no wall-clock in any identity-bearing
  field — `VF-1`, `ID-4`); a **manifest** carrying the **IR contract version** (`IX-07`, semver, its own
  cadence) and a **digest over the canonical bytes** so tampering/truncation is *detectable* (`C-3b`).
  Detectability, **not** authentication — the IR is unsigned; authenticated provenance is the target's
  attestation, and is gated (RFC 113 §4.1, `RR-4`).
- **D-6 — The IR core is a light, dependency-isolated crate.** A new `brygge-ir` crate holds the model,
  the artifact codec, the integrity check, and the honesty machinery, and **links none of the heavy
  decoder libraries** (`INV-4/C-4a`). Decoders (heavy deps) and encoders depend on `brygge-ir`; `verify
  --internal` and `inspect` run through `brygge-ir` alone. The workspace becomes `brygge-ir` +
  `brygge` (CLI) now; `brygge-decode-*` / `brygge-encode-*` land with their RFCs. This makes the
  boundary property (`INV-5/BN-5/CT-05`) structural, one layer in.
- **D-7 — The loss boundary and the fidelity summary are computed *from* the IR, not alongside it**
  (`HO-2/HO-4`, `FS-02`). `LossBoundary` enumerates, by class, what was dropped (representation:
  packfiles/index/reflogs; advisory: mergeinfo/obsmarkers) and references the `Derived` records; the
  fidelity summary is a pure function of the IR, so `brygge summary` reproduces it with no run log.

## Open questions

- **OQ-A — The exact metadata codec.** A compact, canonical, self-describing binary is wanted; the
  choice is between a minimal pure-Rust dependency (e.g. a canonical CBOR writer) and a hand-rolled
  canonical encoder. Criterion: determinism + the `brygge-ir` no-heavy-deps rule (D-6). *Recommended:* a
  minimal pure-Rust canonical encoder; decide in the handoff/first implementation, pinned and audited.
- **OQ-B — Per-atom snapshot vs delta storage.** Store each atom's full path→blob tree (simple,
  content-addressing dedups the blobs anyway) or path-operations against the parent (compact, but a
  reconstruction step). *Leaning:* operations for fidelity (they *are* what a source states, and D-3
  needs them), with the resolved tree derivable; the handoff settles it against determinism and size.
- **OQ-C — The IR contract's versioning cadence and freeze point.** The contract is versioned from day
  one; the ROADMAP promises a freeze (additive-only) once Git (M1) proves it and Mercurial (M2)
  validates it cross-source. This RFC records the promise; the mechanics are the handoff's.
- *Not owner-gated.* The IR is Track A; none of RFC 113 §4.3–§4.5 (OQ-1…OQ-3) block it. Those gate the
  *encoder-to-prikk* path, not the IR.

## Consequences

- brygge gains a light core crate (`brygge-ir`) that is the durable product boundary of PU-3: a foreign
  encoder or an inspector depends on it and nothing heavier.
- The honesty disciplines become model invariants: a decoder *cannot* emit an unmarked inference,
  because inference has no representation except a `Derived` record (D-3); the loss boundary and fidelity
  summary are recoverable by construction (D-7).
- Identity inference is pushed to the encoder, visibly (D-4) — the single decision that lets one IR serve
  both a snapshot target and prikk without lying to either.
- Every later RFC (002 honesty machinery, 003 determinism/format, 004 Git decoder, …) builds on this
  model; a change to the IR model after decoders exist is expensive, so this RFC is the one to get right
  before any source is parsed.

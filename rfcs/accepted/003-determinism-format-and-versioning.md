# RFC 003 — Determinism, artifact format &amp; versioning, integrity

**Status.** Accepted (2026-09-04). Resolves RFC 001's three open questions (OQ-A codec, OQ-B snapshot vs
operations, OQ-C versioning/freeze) and specifies the artifact's canonical serialization, determinism
contract, integrity, and the IR-contract version lifecycle in detail. RFC 001 decided *that* the
artifact is canonical/content-addressed/versioned/digested; this RFC decides *exactly how*.
**Tracks.** ROADMAP Phase A0; requirements §5 (`VF-1`), §6 (`ID-4`), §10 (`IX-07`); external design
§2.2 (`IX-01/IX-07`), `OP-01`; threat model `C-3b`, `INV-6`. Track A — not prikk-gated.
**Touches.** `brygge-ir`'s `artifact` module (codec, container, manifest, digest) and the determinism
guarantees the whole product rests on.

## Summary

Determinism is a *security* property here, not a nicety: if brygge is not reproducible, "faithful" is
uncheckable (`VF-1`) and a tamper can hide in the noise (`T-9`). This RFC pins the canonical
serialization, the content-addressing and digest algorithms, the identity-bearing-vs-provenance field
split, the atom ordering, and the IR-contract version lifecycle with its freeze point — so that the same
inputs always produce byte-identical output and any change to that output is detectable.

## The constraints that scope this design

- **Byte-deterministic output** for the same brygge version + source + params (`VF-1/OP-01`).
- **The only permitted non-determinism is a value brygge cannot control** (e.g. an import wall-clock),
  and it must be **provenance-only, never identity-bearing** (`ID-4/UD-4`).
- **Tamper/truncation detectable** without authentication (`C-3b/RR-4`; the IR is unsigned).
- **Versioned with a stability promise** a consumer can pin (`IX-07`), separate from the tool version.
- **`brygge-ir` stays dependency-light** (RFC 001 D-6) — the codec must not pull a heavy dep.

## Decisions

- **D-1 (resolves RFC 001 OQ-A) — a small, hand-rolled canonical binary encoding for IR metadata.** The
  IR metadata schema is brygge's own and small; a hand-rolled canonical encoder (explicit field order,
  length-prefixed byte strings, LEB128 varints, maps emitted in sorted-key order, one canonical form per
  value) is fully auditable and keeps `brygge-ir` free of a serialization dependency (D-6, `INV-4`). No
  general-purpose codec's "deterministic mode" is trusted in its place. The encoder is spec'd in the
  handoff and covered by round-trip + canonical-form tests.
- **D-2 (resolves RFC 001 OQ-B) — atoms store the source's literal path *operations*, not resolved
  snapshots.** RFC 001 D-3 requires the literal ops (a rename is delete+create + a marked hint); storing
  operations keeps exactly what the source stated. The resolved path→blob tree at any atom is
  **derivable by replay**, not stored. File **content** is stored once per `BlobId` in the
  content-addressed store (dedup); operations reference blobs. Within an atom, operations are emitted in
  canonical path order.
- **D-3 — SHA-256 for content-addressing and the integrity digest.** Blobs are addressed by SHA-256 of
  their bytes; `AtomId` is SHA-256 over the atom's canonical encoding (parents, ordered ops,
  metadata-claims, source ids, status). The artifact's **manifest digest** is SHA-256 over the canonical
  metadata section plus the sorted list of `BlobId`s. (SHA-256 matches the ecosystem's audited `sha2`
  usage.) `verify --internal` recomputes and compares (`C-3b`).
- **D-4 — The identity-bearing / provenance-only split is explicit, with a named-non-determinism
  registry.** *Identity-bearing* (covered by the digest and by `AtomId`): everything the source stated —
  content, ops, ancestry, metadata-claims, source ids, epistemic status. *Provenance-only* (excluded
  from the digest and from all ids): the import wall-clock time, if recorded at all (`ID-4/UD-4`). This
  RFC's **registry of non-identity fields is closed and short**; adding one is an RFC change. A field not
  in the registry is identity-bearing by default.
- **D-5 — Deterministic ordering.** Atoms are serialized in a topological order of the ancestry DAG with
  a **total tiebreak by canonical source atom id**, so re-decode of the same source yields the same order
  and the same bytes. Refs, ops, blob-id lists, and map keys are all emitted in a defined sort order.
- **D-6 — The artifact is a single self-contained, portable file** with an internal layout: a
  **manifest** (magic + IR `contract_version` + counts + the digest), a **canonical metadata section**
  (atoms, refs, provenance, loss), and a **content-addressed blob store** (blobs indexed by `BlobId`,
  appended for streaming on large imports, `OP-02`). One file is easy to hand to `inspect`/`encode` and
  to a third party for the `VF-2` round-check. (Directory packaging is a permitted impl alternative; the
  logical structure is fixed.)
- **D-7 (resolves RFC 001 OQ-C) — the IR-contract version lifecycle and freeze.** The `contract_version`
  is semver, carried in the manifest, **independent of the brygge tool version**. **Pre-freeze**
  (`0.x`): a minor may change the schema with a stated migration note. **Freeze:** once Git (ROADMAP M1)
  proves the IR *and* Mercurial (M2) validates it cross-source, the contract is declared **1.0** and is
  thereafter **additive-only** — new optional fields and new (versioned) enum variants only; no field is
  removed or repurposed. A breaking change after 1.0 is contract **2.0**, deliberate and rare, shipped
  with a converter. **Consumers pin the contract major**; a reader refuses a major it does not know
  rather than misread it (the same discipline as prikk's format gates).

## Open questions

- **OQ-A — Compression of the blob store.** Large imports may want the blob store compressed; compression
  must not break determinism (a canonical/reproducible compressor, or compress outside the digested
  bytes). *Leaning:* defer; store blobs raw for M1, revisit for large-repo performance with a
  determinism-preserving scheme.
- **OQ-B — Streaming read for very large artifacts** (`OP-02`): the single-file layout supports indexed
  blob access; the exact index format is a handoff detail.

## Consequences

- `brygge-ir` gains a precise, dependency-light, auditable artifact codec; re-decode is byte-identical
  and tamper is detectable — the concrete basis for `VF-1/VF-3` and threat controls `C-3b/C-9`.
- RFC 001's three open questions are closed, unblocking the `brygge-ir` implementation (accepted RFC 001
  + this) toward M1.
- The contract-version freeze gives foreign encoders and inspectors (PU-3) a real stability promise to
  pin, while keeping the pre-1.0 period free to refine as the first two sources exercise the IR.

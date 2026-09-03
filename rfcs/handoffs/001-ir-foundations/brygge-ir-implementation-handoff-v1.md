# Handoff — `brygge-ir` implementation (consolidated build spec)

**Consolidates:** the accepted **RFC 001** (IR model), **RFC 002** (honesty machinery), and **RFC 003**
(determinism/format/versioning), under the **RFC 009** dependency policy. This is the single build spec
for the `brygge-ir` crate. For the model's type sketches and rationale, see the companion
[`ir-foundations-handoff-v1.md`](ir-foundations-handoff-v1.md); for the *why* of each decision, the RFCs.
This document is the *how and in what order*, with every open question already resolved.
**Status.** Inherits Accepted. The implementer may build against it now.
**Scope.** The `brygge-ir` crate (the light core) + a thin `brygge` CLI dependency edge + one example.
**Out:** any source decoder or target encoder; the full CLI command surface (later RFCs); anything
prikk-gated.

---

## 1. Crate & workspace layout (RFC 001 D-6, RFC 009 D-1)

Adopt the ecosystem's virtual-workspace layout:

```
Cargo.toml                 # [workspace] virtual manifest, resolver = "3"
deny.toml                  # cargo-deny policy (RFC 009 D-5/D-6)
crates/
  brygge-ir/               # THE LIGHT CORE — #![forbid(unsafe_code)] #![warn(missing_docs)], NO heavy deps
    src/lib.rs
    src/status.rs          # EpistemicStatus (Stated | Derived{kind,by,version,params,confidence})
    src/model.rs           # ChangeAtom, PathOp, RenameHint, RefRecord, MetadataClaims, SourceIdentity, Ir
    src/content.rs         # BlobId (SHA-256) + the content store
    src/artifact.rs        # canonical codec + single-file container + manifest + digest + version gate
    src/honesty.rs         # derivation & loss taxonomies, LossBoundary, summary(&Ir) -> FidelityReport
    examples/ir_roundtrip.rs
  brygge/                  # existing CLI crate; depends on brygge-ir (commands are later RFCs)
```

Tests are **siblings** (`#[cfg(test)] mod tests;` in `foo.rs`, tests in `foo/tests.rs`), never inline.

## 2. The settled decisions (don't re-litigate — build to these)

| Area | Decision | Source |
|---|---|---|
| Model | content store + DAG of change-atoms + per-import `ImportProvenance`/`LossBoundary`; **no `NodeId`** (evidence-for-identity only) | 001 D-1/D-4 |
| Epistemic status | `Stated` vs `Derived{…}` on **every assertion** | 001 D-2, 002 D-1 |
| Renames | literal `Stated` delete+create **plus** a marked `RenameHint`; never collapsed | 001 D-3 |
| Derivation taxonomy | closed, versioned enum: `InferredRename`, `ReconstructedChangeset`, `ReconstructedBranch`, `InferredMerge`, `NormalizedMetadata`, `Other{note}`; each fixes its required `params` | 002 D-1 |
| Loss taxonomy | `Representation` / `AdvisoryUnreliable` / `Other`; the **PR-9 class is never a silent drop** | 002 D-2 |
| Storage | atoms store **literal operations** (tree derivable by replay); blobs stored once per `BlobId` | 003 D-2 |
| Codec | **hand-rolled canonical binary**: explicit field order, LEB128 varints, length-prefixed bytes, sorted-key maps, one canonical form per value; **no serialization dependency** | 003 D-1 |
| Hashing | **SHA-256** for `BlobId`, `AtomId` (over the atom's canonical bytes), and the manifest digest | 003 D-3 |
| Determinism | identity-bearing = everything the source stated; **provenance-only (excluded from digest & ids) = the import wall-clock only** (closed registry) | 003 D-4/D-5, ID-4 |
| Artifact | one **self-contained portable file**: manifest (magic + `contract_version` + counts + digest) · canonical metadata · content-addressed blob store | 003 D-6 |
| Versioning | `contract_version` semver, **independent of the tool version**; a reader **refuses an unknown major**; freeze (additive-only) after M1+M2 | 003 D-7, IX-07 |
| Fidelity report | `summary(&Ir)` is **pure**; human + **versioned machine form** (`report_version`, its own `CT-04` contract); sections preserved/derived/dropped/refused | 002 D-3 |
| Non-suppressibility | the API offers **no** constructor/serializer that omits status, loss boundary, or provenance; verbosity ≠ honesty | 002 D-6, HO-5 |
| Two claims | authorship always `Unverified`; preserved source signatures shown as verifying nothing in a target; one shared renderer | 002 D-5, VF-4 |

## 3. Build order (bottom-up; each green before the next)

1. **`status.rs` + `model.rs`** — the types (see the companion handoff §3 for the sketches). Make illegal
   states unrepresentable: there is no field to store identity (D-4), and inference has no representation
   except a `Derived` record (D-2/D-3).
2. **`content.rs`** — `BlobId` = SHA-256; the content store (insert/get, dedup).
3. **`artifact.rs`** — the canonical encoder/decoder (003 D-1), the single-file container + manifest
   (003 D-6), the SHA-256 manifest digest (003 D-3), the `contract_version` write + **refuse-unknown-major**
   read gate (003 D-7). This is where determinism lives (003 D-4/D-5) — canonical order everywhere,
   import time excluded from the digest.
4. **`honesty.rs`** — the derivation + loss taxonomies (002 D-1/D-2), `LossBoundary` construction, and the
   pure `summary(&Ir) -> FidelityReport` with its versioned machine form (002 D-3). The always-complete
   guarantee (002 D-6) is enforced here and in the `artifact` API.
5. **CLI edge** — just enough for the example to run through `brygge-ir` (full `inspect`/`verify`/`summary`
   commands are RFC 002/003 surface work / a later increment).
6. **`examples/ir_roundtrip.rs`** — §5 below.

## 4. Tests & gates (the acceptance checklist)

Sibling tests covering:
- **Byte-determinism** (VF-1): build → serialize twice → identical; re-decode order stable via the source-id tiebreak (003 D-5).
- **Status survives** (IR-2/HO-1): a `Derived{InferredRename, params…}` and a `Stated` op are distinguishable after round-trip; **D-3** — the literal delete+create are not collapsed away.
- **Loss + report** (HO-2/HO-4/FS-02): `LossBoundary` and `summary(&Ir)` are recoverable/pure from the artifact; a `PR-9`-class drop is rejected (002 D-2).
- **Integrity** (C-3b): a flipped byte fails the digest; an **unknown `contract_version` major is refused**, not misread (003 D-7).
- **No identity type** (D-4): assert there is no `NodeId`-shaped field.
- **No heavy deps** (RFC 009 D-1/D-7, the load-bearing security test): `brygge-ir` and the example build with **no** decoder/FFI crate in the dependency tree; a target could consume the artifact + `verify --internal` on its own surface alone.
- **Non-suppressibility** (002 D-6): a well-formed artifact always yields a complete `FidelityReport`.

Gates (all green): `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` · `cargo test --workspace --locked` · `cargo deny check` · `cargo audit`. (Set up `deny.toml` + the CI supply-chain gates here, per RFC 009 D-6 — even though `brygge-ir` adds no heavy dep, the gates must exist before the Git decoder does.)

## 5. Example

`crates/brygge-ir/examples/ir_roundtrip.rs` — **no source repo, no decoder**: hand-build a small IR (an
`Add`; a `Modify` with a `Stated` `RenameHint` on one path and a `Derived{InferredRename}` hint on
another; a `Derived{ReconstructedChangeset}` atom to exercise atom-level status), serialize to a
`.brygge-ir` file, read it back, print an inspect-style listing (per-atom epistemic status + the loss
boundary), reproduce the `FidelityReport` from the artifact, and run the internal integrity + honesty
checks. Demonstrates the whole light core standing alone.

## 6. Acceptance criteria

1. `brygge-ir` builds with `#![forbid(unsafe_code)]` and **no heavy dependency** in its tree (tested).
2. The model realizes 001 D-1…D-4 and 002 D-1/D-2/D-5/D-6; the artifact realizes 003 D-1…D-7.
3. Round-trip is byte-deterministic; the digest detects tamper; an unknown contract major is refused.
4. `summary(&Ir)` is pure, complete, and reproducible from the artifact (human + machine forms).
5. `deny.toml` + `cargo-deny`/`cargo-audit` gates are wired and green; all standard gates green.
6. `ir_roundtrip` runs decoder-free.

## 7. Out of this increment, queued next

- **RFC 004 — the Git decoder** (`gix`, tier-1 per RFC 009 D-2; the first heavy dep, isolated in
  `brygge-decode-git`; `INV-2` untrusted-input handling) → the `decode`/`inspect`/`verify` command
  surface → ROADMAP **M1 (0.1.0)**, the first stable decode/IR deliverable.

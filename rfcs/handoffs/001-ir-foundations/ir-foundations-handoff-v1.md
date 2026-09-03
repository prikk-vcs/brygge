# Handoff — IR foundations (v1)

**Companion to:** RFC 001 (Proposed 2026-09-03). Inherits its state.
**Realizes:** requirements §10 (`IR-1…IR-6`), external design §2.2 (`IX-01…07`) and §2.3 (`FS-*`), and
prikk RFC 113 §3.1 / §4.2. ROADMAP Phase A0.
**Design items:** `IR-1…IR-6`, `IX-01…07`, `PR-1…PR-6`, `HO-1/HO-2/HO-4`, `VF-1/VF-3`, `ID-4`,
`C-3b`, `C-4a`, `INV-4/INV-5/INV-6`, `FS-02`.

This is the program design and decision record for the increment. **Implementation, tests, and the
example follow it.** Where this handoff and RFC 001 or the design set disagree, the RFC/design wins and
this handoff is corrected first. It defines the IR *model and artifact* — internal design the
requirements/external-design deliberately left to this layer. No source parsing and no target encoding.

---

## 1. Scope

**In:**
- A new **`brygge-ir`** crate: the logical IR model, the artifact codec (write/read), the integrity
  check, and the honesty machinery (loss boundary + fidelity summary as pure functions). Links **no**
  heavy decoder dependency (`C-4a/INV-4`).
- The **artifact format**: a content-addressed blob store + canonical metadata + a versioned,
  integrity-digested manifest (`IX-01/IX-07`, `C-3b`, `VF-1`).
- The read/write **API boundary** decoders and encoders use, and the pure `summary(&Ir)` function
  `brygge summary` will call (`FS-02`).

**Out (later RFCs):**
- The **command surface** (`decode`/`inspect`/`verify`/`summary`) — spine in RFC 002/003; here only the
  `brygge-ir` API those commands wrap, plus a library example.
- **Any source decoder** (RFC 004+) and **any target encoder** (RFC 008). No `gix`/`libgit2`/SVN/CVS.
- **Determinism/format *hardening* and contract-versioning mechanics** beyond the rules below — RFC 003.
- Anything prikk-gated (encoder provenance object, sealing) — RFC 113 OQ-1…OQ-3.

## 2. Crate & module layout

```
brygge-ir/               # the light core — #![forbid(unsafe_code)], #![warn(missing_docs)], no heavy deps
  src/lib.rs
  src/model.rs           # the logical types (§3) + model.rs/model/ per 2018 module style
  src/status.rs          # EpistemicStatus (the load-bearing type)
  src/content.rs         # the content store + BlobId (content-addressing)
  src/artifact.rs        # canonical codec: write/read + the manifest + the integrity digest
  src/honesty.rs         # LossBoundary construction + summary(&Ir) -> FidelitySummary (pure)
  examples/ir_roundtrip.rs
brygge/                  # the existing CLI crate — will depend on brygge-ir (commands come in RFC 002/003)
```
Workspace root becomes a virtual manifest over `crates/*` if we adopt the ecosystem's layout (as stikk
did); either way `brygge-ir` is a peer crate the CLI and future decoders/encoders depend on. Tests are
**siblings** (`#[cfg(test)] mod tests;`), never inline.

## 3. The IR logical model (design, not final code)

Types sketched to fix shape and field→requirement mapping; exact signatures are the implementer's.

```rust
// Content-addressed file bytes (PR-1, IX-01, VF-1). Dedup + integrity + determinism for free.
pub struct BlobId(/* content hash of the bytes */);
// blobs live in the artifact's object store, keyed by BlobId.

// The single most important type (IR-2, HO-1, RFC 113 §4.2): every assertion is one of these.
pub enum EpistemicStatus {
    Stated,                                        // the source recorded it
    Derived {                                      // a decoder/encoder inferred it
        by: DecoderId, decoder_version: Version,
        params: Params,                            // PR-5: the exact parameters that produced it
        confidence: Option<Confidence>,
    },
}

// PR-4 / IR-3 — the only cryptographic link back; carried opaque, target-meaningless.
pub struct SourceIdentity {
    kind: SourceKind,                              // git | hg | svn | cvs | …
    repo_id: OpaqueBytes, atom_id: OpaqueBytes,    // repo identity; commit SHA / revision / cvs tag
    signatures: Vec<OpaqueSignature>,              // GPG etc. — verify nothing in any target (NG-3)
}

pub enum PathOp {                                  // per-atom operations, each self-marked
    Add    { path: Path, blob: BlobId, mode: Mode, status: EpistemicStatus },
    Modify { path: Path, blob: BlobId, mode: Mode, status: EpistemicStatus },
    Delete { path: Path,                           status: EpistemicStatus },
}

// D-3: a rename is NEVER an op that hides the source's delete+create. It is a separate marked hint
// beside Stated delete+create ops. hg `hg mv` => status: Stated; git similarity => status: Derived.
pub struct RenameHint { from: Path, to: Path, status: EpistemicStatus }

pub struct MetadataClaims {                        // PR-3 — claims, never verified facts
    author: Option<Identity>, committer: Option<Identity>,
    message: Option<String>, times: Times,         // source-stated; not target-authoritative
}

pub struct ChangeAtom {
    id: AtomId,                                    // IR-local deterministic id — NOT a source id, NOT a NodeId (D-4)
    parents: Vec<AtomId>,                          // PR-2 ancestry (order significant)
    ops: Vec<PathOp>, rename_hints: Vec<RenameHint>,
    metadata: MetadataClaims, source: SourceIdentity,
    status: EpistemicStatus,                       // atom-level: Stated (git/hg/svn) | Derived (reconstructed cvs changeset)
}

pub struct RefRecord {                             // branches / tags / bookmarks / named-branches (PR-2)
    name: String, kind: RefKind, target: AtomId,
    status: EpistemicStatus,                       // SVN branch-by-convention => Derived
    source: Option<SourceIdentity>,
}

pub struct ImportProvenance {                      // PR-6 — one per import
    source: SourceIdentity,                        // repo-level
    brygge_version: Version, decoder: DecoderId, decoder_version: Version,
    params: Params,                                // PR-5 — every inference parameter used
    // Any import timestamp is provenance-only, NEVER identity-bearing (ID-4 / UD-4).
}

pub struct LossBoundary {                          // HO-2 / IR-4
    dropped: Vec<DropRecord>,                      // { class, reason } — representation vs advisory
    derived: Vec<DerivedRef>,                      // references to the Derived records, by class/count
}

pub struct Ir {                                    // the top-level container
    contract_version: Version,                     // IX-07 (its own semver)
    atoms: Vec<ChangeAtom>,                         // deterministic topological order
    refs: Vec<RefRecord>,
    provenance: ImportProvenance, loss: LossBoundary,
    // + the content store (blobs) + a digest over the canonical bytes (held in the manifest, §4)
}
```

**The model enforces honesty structurally:** there is no way to express an inference except a `Derived`
record (D-2/D-3), and no way to store identity (there is no `NodeId` field — D-4), so a decoder
*cannot* launder a guess or pre-bake identity even by mistake.

## 4. The artifact format

- **A container** (a directory, or a single file with an internal layout — OQ for the implementer):
  a **manifest**, a **canonical metadata section**, and a **content-addressed blob store**.
- **Manifest** carries the **IR contract version** (`IX-07`), counts, and a **digest over the canonical
  bytes** of (metadata + the sorted blob ids) — `C-3b`. `verify --internal` recomputes and compares.
  The IR is **unsigned**: this is tamper-*detectability*, not authentication (`RR-4`; authenticated
  provenance is the target's, gated).
- **Canonical metadata encoding** (`VF-1`): fixed field order, sorted maps, canonical integer/string
  forms, **no wall-clock or environment value in any identity-bearing field**. Codec choice is OQ-A of
  RFC 001 — a minimal pure-Rust canonical encoder is the recommendation (keeps `brygge-ir` heavy-dep
  free, D-6). Pin and audit whatever is chosen.
- **Blobs** are stored once per `BlobId` (dedup) and referenced from ops.
- **Deterministic ordering:** atoms in a defined topological order with a total tiebreak (e.g. by source
  atom id) so re-decode yields byte-identical output.

## 5. Determinism & integrity (VF-1, C-3b, ID-4)

- Same inputs → **byte-identical artifact**. The only permitted non-determinism is a value brygge cannot
  control (e.g. an import timestamp), which is **provenance-only and never identity-bearing** — the
  digest and the atom ids do not depend on it.
- The integrity digest makes a flipped byte or a truncation detectable by `verify --internal`.
- A `tests/` case builds an IR, serializes twice, and asserts the two byte streams are identical; another
  flips a byte and asserts the digest check fails.

## 6. The honesty machinery (HO-2/HO-4, FS-02)

- `LossBoundary` is **constructed by the decoder** as it drops/derives, but its *representation* lives in
  the IR (§3) so it survives serialization.
- `summary(&Ir) -> FidelitySummary` is a **pure function of the IR**: preserved / derived (with params &
  confidence) / dropped (by class) / refused. `brygge summary` (later) calls it and reproduces the exact
  end-of-run summary **from the artifact alone** — the external proof of `HO-4`/`FS-02`.
- There is no "verbosity off for honesty" switch anywhere in `brygge-ir` (`HO-5/CF-02`): the fields are
  always present; only rendering varies.

## 7. Security surface (threat model brygge-03)

- **`INV-4/C-4a` — `brygge-ir` links no heavy decoder dependency.** Enforced by a test/CI check that the
  crate's dependency tree contains none of `gix`/`libgit2`/SVN/CVS libraries, and that the example builds
  with only `brygge-ir`. This is the internal form of `INV-5/BN-5/CT-05`.
- **`INV-6` — integrity + determinism** are the model's tamper controls (§5); the IR is unsigned by
  design (`RR-4`).
- `brygge-ir` executes **no source-provided content** and reads no repository — it only models bytes a
  decoder already extracted; `INV-2`'s untrusted-input handling lives in the decoders (RFC 004+), but the
  IR model must not add a foot-gun (e.g. a path field must be inert data, never used as a write target —
  `C-2c/C-7`; the artifact writer writes only under the operator-given output path).
- `#![forbid(unsafe_code)]`; no `unwrap`/`expect`/`indexing` in production paths.

## 8. Test plan

- **Round-trip determinism** (`VF-1`): build → serialize → deserialize → serialize; byte-identical.
- **Epistemic status survives** (`IR-2/HO-1`): a `Derived` rename-hint keeps its params/confidence; a
  `Stated` op stays `Stated`; the two are distinguishable after a round-trip.
- **D-3 preserved:** a rename is stored as `Stated` delete+create **plus** a hint — the delete+create are
  not collapsed away.
- **Loss boundary recoverable** (`HO-2/IR-4`) and **`summary(&Ir)` is a pure function** reproducing the
  same result on re-read (`FS-02`).
- **Integrity** (`C-3b`): a flipped byte fails the digest check.
- **No heavy deps** (`INV-4`): a check that `brygge-ir`'s dependency tree and the example are free of any
  decoder library — the load-bearing security test of this increment.
- **No identity in the IR** (`D-4`): a compile-level/absence test that there is no `NodeId`-shaped field.
- Gates: `fmt` / `clippy --all-targets --all-features -D warnings` / `test`; (supply-chain gates apply
  once a dependency is added).

## 9. Example

`brygge-ir/examples/ir_roundtrip.rs` — **no source repo, no decoder**: hand-build a tiny 2-atom IR (an
`Add`, then a `Modify` + a `Stated` rename-hint on one path and a `Derived` rename-hint on another),
serialize to an artifact, read it back, print an `inspect`-style listing (each atom's epistemic status
and the loss boundary), and run the internal checks (`VF-3`). It demonstrates the whole `brygge-ir`
product working with only the light core on the dependency path.

## 10. Acceptance criteria

1. `brygge-ir` exists, `#![forbid(unsafe_code)]`, and its dependency tree contains **no** heavy decoder
   library (tested — `INV-4/C-4a`).
2. The model (§3) represents `Stated` vs `Derived` per assertion (`IR-2`), records renames as marked
   hints beside literal ops (`D-3/HO-1`), carries source opaque ids/signatures (`PR-4/IR-3`), and holds
   **no** target identity type (`D-4/IR-6`).
3. The artifact is content-addressed, canonically encoded, **versioned** (`IX-07`), and
   **integrity-digested** (`C-3b`); round-trip is **byte-deterministic** (`VF-1`), with any timestamp
   provenance-only (`ID-4`).
4. `LossBoundary` and `summary(&Ir)` are recoverable/pure from the artifact alone (`HO-2/HO-4/FS-02`).
5. The `ir_roundtrip` example runs against a hand-built IR with no source and no decoder.
6. Gates green.

## 11. Out of this increment, queued next

- **RFC 002 — honesty & provenance machinery**: the `inspect`/`summary` command surface and the
  non-suppressibility enforcement built on §6 (the model guarantees are here; the *surface* is there).
- **RFC 003 — determinism/format hardening + contract-version mechanics** (the freeze process, OQ-C).
- **RFC 004 — the Git decoder** (the first source; `INV-2` untrusted-input handling lands here), toward
  ROADMAP M1 (0.1.0, the first stable decode/IR deliverable).

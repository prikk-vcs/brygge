# Handoff — `brygge-ir` re-cut to IR contract 0.2.0 (RFC 011)

**Governing RFC:** RFC 011 (IR contract re-cut), **Accepted 2026-09-23**, with OQ-A ruled: contract
**`0.2.0`**. This handoff inherits that state. Read RFC 011 D-1…D-10 first. This document turns them into
an exact wire schema and a build.

**Policy:** v0, never released. This is a deliberate breaking change: no compatibility with pre-release
artifacts, beyond refusing them clearly.

**Order:** handoff **5**. Start it after batch-1 handoffs 1–4 are approved, because it adapts code they
change (the CLI, the decoders). The per-decoder handoffs (CVS, hg, SVN, Git batch 2) follow it and build
on its types.

---

## 1. Scope in one paragraph

Replace `brygge-ir`'s positional codec with **tagged records carrying a critical bit**, a **strict
canonical form** and a **digest over the stored bytes**. Put the **version gate first**. Change the
model per RFC 011:
- `Text` and `Time`;
- `CopyRecord` in place of `RenameHint`;
- `PathOp::Replace`;
- `Flag`s;
- `Signature` and `Extra`;
- `Annotation`;
- no `import_time`.

Update the honesty report (report v2). Adapt the decoders and the CLI **mechanically**, so everything
compiles and behaves as before on the new types. Where a correct value is directly at hand, fill it
(§5); every other decoder improvement belongs to the per-decoder handoffs. Publish the schema as
`docs/src/reference/ir-artifact-format.md`.

## 2. The wire format (the contract — build exactly this)

### 2.1 Primitives

| Primitive | Encoding |
|---|---|
| `uvarint` | unsigned LEB128, **minimal** (the reader rejects a trailing `0x80`-continued zero) |
| `svarint` | zig-zag, then `uvarint` |
| `bytes` | the field's value bytes as-is (the length comes from the field header) |
| `text` | `bytes` that must be valid UTF-8 |
| `id32` | exactly 32 raw bytes |
| `list<T>` | `uvarint(count)`, then `count` items, each a record, enum or `id32` as the schema says |
| `map` | `uvarint(count)`, then `(uvarint(klen) ‖ key text ‖ uvarint(vlen) ‖ value text)*`, with keys **strictly ascending** (bytewise) |
| **record** | `uvarint(n)`, then `n` fields, in **strictly ascending tag order** |
| **field** | `uvarint(tag) ‖ uvarint(len) ‖ value (exactly len bytes)`, where `tag = (id << 1) \| critical` |
| **enum** | `uvarint(variant) ‖ record` (a payload-less variant has the empty record `0x00`) |

### 2.2 Canonical rules (every one is enforced on read, and violating one is `Error::NonCanonical`)

1. **Required** fields are always present, even when zero or empty.
2. An **optional** field that is absent is omitted.
3. A **list** field that is empty is omitted.
4. Tags are strictly ascending.
5. Varints are minimal.
6. Every length is exact.
7. There are no trailing bytes: none after a record, none after a field value, none after the blob store.
8. **Every field defined in 0.2.0 has `critical = 1`.**
9. **Unknown fields:**
   - an unknown id with `critical = 1` fails with `Error::UnknownCriticalField { record, tag }`;
   - an unknown id with `critical = 0` is skipped by its length, and counted in
     `Decoded::skipped_non_critical_fields`.

### 2.3 Container

```
"BRYGGEIR" (8) · format u8 = 2 · version: uvarint major ‖ uvarint minor ‖ uvarint patch · digest (32)
· metadata: uvarint(len) ‖ Ir record
· blobs:    uvarint(count) ‖ ( id32 ‖ uvarint(len) ‖ bytes )*   — ids strictly ascending, each id = SHA-256(bytes)
```

**Read order** (nothing later is decoded before an earlier check passes):
1. The magic.
2. The format byte: `1` gives `Error::PreReleaseArtifact` ("this artifact was made by a pre-release
   brygge build; re-decode the source"); any other value except `2` gives `Error::Decode("unknown
   artifact format N")`.
3. The version: a major or minor different from `CURRENT` (`0.2.x`) gives
   `Error::UnsupportedContract { found, supported }` ("artifact contract 0.Y is not supported by this
   build (0.2)"); any patch is accepted.
4. The digest: SHA-256 over the **stored** bytes from the first version byte to the end of the input,
   **excluding the 32 digest bytes**. A mismatch is `Error::DigestMismatch`.
5. The metadata record.
6. The blob store.
7. The structural checks (§2.5).

### 2.4 Records (field `id`: name, type; all `critical = 1` in 0.2.0; "req" = required, "opt" = optional,
"list" = omitted when empty)

**`Ir`** (the metadata root):

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | atoms | list<ChangeAtom> | list |
| 2 | refs | list<RefRecord> | list |
| 3 | provenance | ImportProvenance | req |
| 4 | dropped | list<DropRecord> | list |
| 5 | flags | list<Flag> | list |

**`ChangeAtom`:**

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | id | id32 | req |
| 2 | parents | list<id32>, **in source order** | list |
| 3 | ops | list<PathOp> | list |
| 4 | copies | list<CopyRecord> | list |
| 5 | metadata | MetadataClaims | opt (omitted when every claim is absent) |
| 6 | source | SourceIdentity | req |
| 7 | status | EpistemicStatus | req |

**AtomId** = SHA-256 over the canonical encoding of this record **without field 1** (i.e. `uvarint(n-1)`
and fields 2–7).

**`PathOp`** (enum):

| Variant | Name | Fields |
|---|---|---|
| 0 | Add | {1 path text req, 2 blob id32 req, 3 mode uvarint req, 4 status EpistemicStatus req} |
| 1 | Modify | {1 path, 2 blob, 3 mode, 4 status} |
| 2 | Delete | {1 path, 4 status} |
| 3 | Replace | {1 path, 2 blob, 3 mode, 4 status} (RFC 011 D-7) |

**`CopyRecord`:**

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | from | text | req |
| 2 | from_atom | id32 | req |
| 3 | to | text | req |
| 4 | status | EpistemicStatus | req |

**`MetadataClaims`:**

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | author | Identity | opt |
| 2 | author_time | Time | opt |
| 3 | committer | Identity | opt |
| 4 | commit_time | Time | opt |
| 5 | message | Text | opt |

**`Identity`:** 1 name Text (req) · 2 email Text (opt).

**`Text`:** 1 bytes (req; may be empty) · 2 encoding text (opt; set only when the source declared one).

**`Time`:** 1 seconds svarint (req) · 2 offset_minutes svarint (opt; must fit `i16`).

**`SourceIdentity`:**

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | kind | SourceKind | req |
| 2 | repo_id | bytes | req |
| 3 | atom_id | bytes | req |
| 4 | signatures | list<Signature>, in source order | list |
| 5 | extras | list<Extra>, in source order | list |

**`SourceKind`** (enum): 0 Git · 1 Hg · 2 Svn · 3 Cvs · 4 Other{1 label text req}.

**`Signature`:** 1 label text (req; e.g. `gpgsig`, `gpgsig-sha256`) · 2 bytes (req).

**`Extra`:** 1 label text (req) · 2 bytes (req).

**`EpistemicStatus`** (enum): 0 Stated{} · 1 Derived{Derivation}.

**`Derivation`:**

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | kind | DerivationKind | req |
| 2 | by | text | req |
| 3 | decoder_version | text | req |
| 4 | params | map | list (omitted when empty) |
| 5 | confidence | uvarint | opt; **must be ≤ 100** |

**`DerivationKind`** (enum): 0 InferredRename · 1 ReconstructedChangeset · 2 ReconstructedBranch ·
3 InferredMerge · 4 NormalizedMetadata · 5 Other{1 note text req}.

**`RefRecord`:**

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | name | text | req |
| 2 | kind | RefKind | req |
| 3 | target | id32 | req |
| 4 | status | EpistemicStatus | req |
| 5 | source | SourceIdentity | opt |
| 6 | annotation | Annotation | opt |

**`RefKind`** (enum): 0 Branch · 1 Tag · 2 Bookmark · 3 NamedBranch · 4 Other{1 label text req}.

**`Annotation`:** 1 tagger Identity (opt) · 2 time Time (opt) · 3 message Text (opt).

**`DropRecord`:** 1 class LossClass (req) · 2 what text (req) · 3 reason text (req).

**`LossClass`** (enum): 0 Representation · 1 AdvisoryUnreliable · 2 Other.

**`Flag`:** 1 kind FlagKind (req) · 2 what text (req) · 3 count uvarint (req, ≥ 1) · 4 reason text (req).

**`FlagKind`** (enum): 0 ConventionViolation · 1 BelowConfidenceFloor.

**`ImportProvenance`:**

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | source | SourceIdentity | req |
| 2 | brygge_version | text | req |
| 3 | decoder | text | req |
| 4 | decoder_version | text | req |
| 5 | params | map | list |

### 2.5 Canonical orders and structural checks (enforced on read; the builder produces them)

- **atoms** are in the builder's canonical order: topological, with the ready set ordered by
  `(source.atom_id bytes, AtomId)`. The reader **recomputes** that order and rejects any other. Atom ids
  are unique; every parent names an **earlier** atom; each stored id equals its recomputed AtomId.
- **ops** are strictly ascending by path (so there is one op per path per atom). **copies** are strictly
  ascending by `(to, from, from_atom)`. Each copy's `from_atom` names an atom **earlier** in the order, never the atom itself.
- **refs** are strictly ascending by `(name, kind variant)`, and every target is an atom.
- **dropped** entries are strictly ascending by `(class variant, what)`. **flags** are strictly ascending
  by `(kind variant, what)`.
- **blobs:** every op's blob is present, and **every stored blob is referenced by at least one op** (no
  orphans).

## 3. The Rust API (`crates/brygge-ir`)

**Types** mirror §2.4 one-to-one.
- `RenameHint` is renamed `CopyRecord`, and the `rename_hints` field is renamed `copies`.
- `MetadataClaims` uses `Option<Identity>` / `Option<Time>` / `Option<Text>`.
- `Identity { name: Text, email: Option<Text> }`.
- `Text { bytes: Vec<u8>, encoding: Option<String> }` with `Text::as_utf8(&self) -> Option<&str>` and
  `Text::utf8(s: &str) -> Text` (a convenience constructor for UTF-8 sources, `encoding: None`).
- `Time { seconds: i64, offset_minutes: Option<i16> }`.
- `SourceIdentity.signatures: Vec<Signature>` and `extras: Vec<Extra>`.
- `Ir.flags: Vec<Flag>`; `LossBoundary` stays, holding `dropped`.
- `ImportProvenance` loses `import_time`.

**Every public type and field** gets a one-sentence rustdoc stating its meaning, its unit, and what
`None` or empty means (RFC 011 D-10). In particular:
- **`MetadataClaims`** documents the one-claim rule: `author`/`author_time` hold what the source
  attributes the change to; `committer`/`commit_time` only a distinct recording identity or time the
  source states.
- **`CopyRecord`** documents "a move is exactly: `from_atom` is the atom's first parent **and** the atom
  deletes `from`", and gains `ChangeAtom::is_move(&self, copy: &CopyRecord) -> bool`.
- **`DerivationKind`** documents its required params:
  - `InferredRename`: `rename_algorithm`, `rename_threshold`;
  - `ReconstructedChangeset`: `window_secs`, `cluster_keys`, `date_rule`;
  - `ReconstructedBranch`: `layout` or `source`.

**Artifact API:**
- `to_bytes(&Ir) -> Vec<u8>`.
- `from_bytes(&[u8]) -> Result<Decoded>` with `Decoded { pub ir: Ir, pub skipped_non_critical_fields: u64 }`.

**Errors (`brygge_ir::Error`):**
- **Added:** `PreReleaseArtifact`, `UnsupportedContract { found: ContractVersion, supported:
  ContractVersion }`, `UnknownCriticalField { record: &'static str, tag: u64 }`, `NonCanonical(String)`.
- **Kept:** `Decode`, `DigestMismatch`, `Invariant`.
- **Removed:** `UnsupportedContractMajor`.

**`version.rs`:**
- `CURRENT = 0.2.0`.
- `ContractVersion::accepts(self) -> bool` implements the `0.y` rule (RFC 011 D-3), and its doc states
  it exactly. This replaces the misleading `is_readable` doc.

**`IrBuilder`:**
- `add_atom(draft) -> Result<AtomId>`. It returns `Invariant` on two ops for one path, or on a copy whose
  `from_atom` is not an already-added atom.
- `add_flag(Flag)`.
- `set_loss(LossBoundary)`.
- `finish()` sorts refs, drops and flags canonically, and **prunes unreferenced blobs**.

**`honesty.rs` (report version 2):**
- `FidelityReport` loses `refused` and gains `flagged: BTreeMap<String, u64>` (kind label → summed
  count). `REPORT_VERSION = 2`.
- The machine form gains `flagged.<kind>=N` lines.
- The human form gains a **flagged** section, shown only when non-empty:
  `flagged:   recorded, and why this import needs your attention:` with one line per kind:
  `convention-violation: N`, `below-confidence-floor: N`.

**Canon module:**
- Replace `CanonWriter`/`CanonReader` with record-aware writers and readers that enforce §2.2.
- The reader never allocates beyond the remaining input: check every length and count first.
- Keep it `pub(crate)` unless a consumer needs it. Today none does; the decoders use the model and the
  builder.

## 4. The CLI (`crates/brygge`)

- `from_bytes` now returns `Decoded`. `inspect` prints `skipped_non_critical_fields` when it is non-zero:
  `note: N field(s) from a newer contract were not understood and were skipped (none of them can change
  what this artifact claims)`.
- **Exit 30 comes from `ir.flags`** (any flag gives 30), not from string matching. Remove
  `brygge_decode_svn::LAYOUT_UNMATCHED` and `brygge_decode_cvs::UNDER_FLOOR`; they were marked
  transitional by the project-hygiene handoff.
- **`verify`:**
  - rename every "rename hint" to "copy";
  - `replay` also checks every copy: `from` exists in `from_atom`'s replayed tree. Tree retention counts
    copy references as well as first-parent references, so memory stays bounded;
  - `Replace` needs its path present;
  - `source-invariants`: for cvs, "no rename hint" becomes "no copy record";
  - `derivations` adds `date_rule` to `ReconstructedChangeset`'s required params **from the CVS handoff
    onward**. Until then, accept its absence and say so in a code comment tied to that handoff.
- **`inspect --atoms`:**
  - shows copies as `copy <from>@<from_atom short> -> <to> [status]`, marking moves as `move`;
  - shows flags;
  - shows signatures by label and extras by label (the bytes as a length, not a dump), all neutralized
    through `display`.
- Machine formats: `inspect_version=3` and `verify_version=3` (the record names change).
- **The verdict is three-valued** (carried from review 003, R-5; mandatory).
  - When a requested `--against-source` could not be checked (`not-checked`) and nothing that ran
    failed, the verdict is **`incomplete`**: human `=> INCOMPLETE (a requested check could not run)`,
    machine `verify.result=incomplete`, exit `1`.
  - `pass` means everything requested ran and held. `fail` means something that ran did not hold
    (exit `50`, which takes precedence).
  - The verdict must never read `PASS` when the exit code is not `0`.
  - Test: the nonexistent-source case prints `INCOMPLETE` and `verify.result=incomplete`, and exits 1.

## 5. Mechanical decoder adaptation (keep behaviour; fill what is directly at hand)

| Decoder | Fill now | Leave for its handoff |
|---|---|---|
| **Git** | `Text` from the **raw bytes** already read (message, names, emails): no `to_str_lossy`, since the bytes are carried now. The commit signature becomes `Signature { label: "gpgsig" }`. Inferred renames become `CopyRecord` with `from_atom` = the first parent. `Time.seconds` as today, offset `None` | `encoding`, timezone offsets, `mergetag` and other extras, tag annotations, `gpgsig-sha256` |
| **hg** | `Text` from the changelog bytes: parse the changelog as bytes; only the manifest hex and the date line must be ASCII. **Remove** the interim `UnsupportedFormat { requirement: "non-UTF-8 changeset metadata" }` refusal the input-ceilings handoff added, since the text is now carried byte-exact. Stated copies become `CopyRecord` with `from_atom` = p1 (the current implied meaning) | the correct `from_atom` via `copyrev`/linkrev, offsets, extras, the one-claim rule |
| **SVN** | Copies become `CopyRecord` with the **correct** `from_atom`: keep a `revnum → AtomId` map; `copyfrom-rev` is directly at hand. The layout violation becomes `Flag { kind: ConventionViolation, … }` | `Replace`, offsets, the one-claim rule |
| **CVS** | Log bytes become `Text` (no lossy conversion). The under-floor record becomes `Flag { kind: BelowConfidenceFloor, count: <the changesets> }` | mainline-only, `date_rule`, clustering, offsets, the one-claim rule |

Also: the `ir_roundtrip` example, `tools/bench`, and every test are adapted to the new types.

## 6. Documentation

- **New:** `docs/src/reference/ir-artifact-format.md`, **"IR artifact format — contract 0.2.0"**:
  - §2 of this handoff, rewritten as a reference for a foreign encoder or reader author: the primitives,
    the canonical rules, the container, every record table, the orders, and the version and
    critical-field rules;
  - a short worked example: the hex of a minimal artifact with one atom, annotated.

  It is the published IX contract (PU-3).
- **Update:**
  - `crates/brygge-ir/README.md` (contract 0.2.0, the critical-bit rule, a link to the reference);
  - the `ir_roundtrip` example's comments;
  - `CHANGELOG.md` under Unreleased/Changed, marked **breaking**: "IR contract re-cut to 0.2.0 (RFC 011);
    pre-release artifacts must be re-decoded".

## 7. Required tests

**Codec and container** (in `brygge-ir`, sibling tests):
1. **Round-trip:** byte-identical for each record and enum variant, including `Replace`, `Other{…}`
   variants, and every optional field both present and absent.
2. **Strictness: one test per rule in §2.2 and §2.5,** each yielding `NonCanonical`. For example:
   - an overlong varint;
   - descending or duplicate tags;
   - an encoded-but-empty optional;
   - an empty list encoded;
   - unsorted or duplicate map keys, ops, copies, refs, drops, flags and blobs;
   - atoms out of canonical order;
   - a parent or a `from_atom` naming a later atom;
   - an orphan blob;
   - trailing bytes at each level;
   - a length exceeding the input;
   - confidence 101;
   - flag count 0.
3. **Unknown fields:** an injected critical field (any record) gives `UnknownCriticalField` naming the
   record and tag. An injected non-critical field is skipped, `skipped_non_critical_fields` equals 1,
   and the rest decodes identically.
4. **Version gate:**
   - format 1 gives `PreReleaseArtifact`, and **the metadata is never parsed** (prove it by making the
     metadata garbage);
   - contract 0.3.0 or 1.0.0 gives `UnsupportedContract`, before metadata;
   - 0.2.7 is accepted.
5. **Digest:** flip one byte in each section (version, metadata, blob store); each gives `DigestMismatch`.
6. **AtomId:** equals SHA-256 of the record without field 1. A test pins one literal hash for a fixed
   atom (a compatibility vector).
7. **Isolation:** `sha2` remains the only dependency (the CI gate from the hygiene handoff).

**Adaptation:**
- All existing decoder and CLI tests pass on the new types.
- Determinism holds: decode twice gives identical bytes.
- The SVN tests assert the correct `from_atom` for a copy from an older revision.
- The flags drive exit 30 for SVN (unresolved layout) and CVS (under the floor).

## 8. Acceptance criteria

- The wire format matches §2 exactly. `docs/src/reference/ir-artifact-format.md` matches the code (the
  reviewer reads both).
- The strictness, unknown-field, version-gate and digest tests all pass. No non-canonical input is
  accepted.
- No lossy text conversion remains for Git messages and names, or CVS logs.
- Pre-release artifacts are refused with the stated message.
- The five gates plus the isolation gate pass, `--locked`; `Cargo.lock` unchanged.

## 9. Prohibited shortcuts

- No lenient reading "for robustness". Strictness is the integrity guarantee.
- No default-filling of missing required fields.
- No re-encoding to compute the digest: it is over the stored bytes.
- No field with `critical = 0` in 0.2.0.
- No decoder behaviour changes beyond §5.

## 10. Security gate

This is a new parser for untrusted artifacts (TB-5), so the architect reviews it against `brygge-03`:
- bounds before allocation;
- no recursion on attacker-controlled depth (nesting is fixed by the schema);
- panic-freedom (add a small fuzz-style test feeding truncations and single-byte mutations of a valid
  artifact: every one is a typed error, never a panic).

## 11. Review request

File `.git-exclude/review-request/007-brygge-ir-recut.md` with:
- the standard sections;
- the AtomId compatibility vector;
- the annotated minimal-artifact hex from the reference page;
- the §2.2/§2.5 rule → test mapping.

Do not commit until "Approved".

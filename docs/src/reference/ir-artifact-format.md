# IR artifact format — contract 0.2.0

This is the published wire contract for a brygge IR artifact (requirement `PU-3`): everything a foreign
encoder or reader needs to produce or consume one, without linking `brygge-ir` itself. It is normative —
`brygge-ir`'s own codec (`crates/brygge-ir/src/canon.rs`, `model.rs`, `artifact.rs`) implements exactly
what is written here (RFC 011, §2).

Design intent (RFC 011 D-1): **tagged records with a critical bit**, a **strict canonical form** — one
logical value has exactly one valid byte encoding — and a **digest over the stored bytes**, never a
re-encoding. Strictness is the integrity guarantee: an implementation that accepts a non-canonical byte
string would let two different byte strings mean the same value, defeating content-addressing
(`AtomId`/`BlobId`) and the artifact's own digest.

## 1. Primitives

| Primitive | Encoding |
|---|---|
| `uvarint` | Unsigned LEB128, **minimal**: the last byte (continuation bit clear) is never `0x00` unless it is also the first byte. A reader rejects any other encoding (a "trailing `0x80`-continued zero") as non-canonical. |
| `svarint` | Zig-zag encode the signed value to an unsigned one (`(v << 1) ^ (v >> 63)`), then `uvarint`. |
| `bytes` | The field's value bytes, exactly as given — no length prefix of its own. The length comes from the enclosing field header. |
| `text` | `bytes` that must be valid UTF-8. |
| `id32` | Exactly 32 raw bytes — no length prefix (the width is fixed by the schema). |
| `list<T>` | `uvarint(count)`, then `count` items back to back, each a record, an enum, or `id32`, as the schema for that list says. |
| `map` | `uvarint(count)`, then `count` entries of `uvarint(klen) ‖ key(text) ‖ uvarint(vlen) ‖ value(text)`, with keys **strictly ascending** by byte value. |
| **record** | `uvarint(n)`, then `n` **fields**, in **strictly ascending tag order**. |
| **field** | `uvarint(tag) ‖ uvarint(len) ‖ value` — exactly `len` bytes of value. `tag = (id << 1) \| critical` (`critical` is `0` or `1`). |
| **enum** | `uvarint(variant) ‖ record` — the variant's own fields (a payload-less variant is the empty record, one byte: `0x00`). |

## 2. Canonical rules

Every rule below is enforced on read. Violating one is a decode failure (`Error::NonCanonical` in
`brygge-ir`'s own reader) — never silently accepted, never repaired.

1. A **required** field is always present, even when its value is zero or empty.
2. An **optional** field that is absent is **omitted** — never present with an empty/sentinel value.
3. A **list** (or `map`) field that is empty is **omitted** — never present with `count = 0`.
4. Field tags within one record are **strictly ascending** — no duplicate, no descending pair.
5. Every varint is **minimal** (§1).
6. Every declared length is **exact**: a field's value, a record's own field-count-implied span, a list
   item — none may be shorter or longer than what its length says.
7. There are **no trailing bytes**: not after a record's last field, not after a field's value, not after
   the container's blob store.
8. **Every field defined in contract 0.2.0 has `critical = 1`.** (The critical bit exists for a future
   minor version to add a non-critical field additively — see §4.)
9. **Unknown fields:**
   - an unrecognized id with `critical = 1` fails the read (`Error::UnknownCriticalField { record, tag
     }`) — this build cannot safely ignore a field that might change the record's meaning;
   - an unrecognized id with `critical = 0` is **skipped** by its declared length, and counted (surfaced
     as `Decoded::skipped_non_critical_fields`) — informational only, since a skipped field is, by
     construction, one that cannot change what the artifact claims.

## 3. Container

```
"BRYGGEIR" (8 bytes)
· format: u8 = 2
· version: uvarint(major) ‖ uvarint(minor) ‖ uvarint(patch)
· digest: 32 bytes
· metadata: uvarint(len) ‖ <Ir record, exactly len bytes>
· blobs: uvarint(count) ‖ ( id32 ‖ uvarint(len) ‖ bytes )*
```

Blob ids are **strictly ascending**, and each id equals `SHA-256(bytes)`.

**Read order** — nothing later is ever inspected before an earlier check has passed:

1. **The magic.** Anything else: `Error::Decode`.
2. **The format byte.** `1` means a **pre-release** artifact (from before contract 0.2.0/RFC 011):
   `Error::PreReleaseArtifact`, and **the metadata is never parsed** — the wire shape underneath changed
   too much between formats to read any further safely. Any value other than `1` or `2`:
   `Error::Decode("unknown artifact format N")`.
3. **The version.** A major or minor different from this build's `CURRENT` (`0.2.x`) gives
   `Error::UnsupportedContract { found, supported }` ("artifact contract 0.Y is not supported by this
   build (0.2)"). Any patch is accepted. See §4 for the exact acceptance rule.
4. **The digest.** SHA-256 over the **stored bytes**, from the first version byte to the end of the
   input, **excluding the 32 digest bytes themselves** (i.e. `version ‖ metadata ‖ blobs`, concatenated
   exactly as stored — never a re-encoding). A mismatch: `Error::DigestMismatch`.
5. **The metadata** — the `Ir` record (§5).
6. **The blob store.**
7. **The structural checks** (§6) — canonical order and referential integrity across the whole artifact.

## 4. Version acceptance (RFC 011 D-3, the "0.y" rule)

While the contract's major version is `0`: a **breaking** change (a critical field or variant added,
changed, or removed) bumps the **minor** (`0.y`); a **purely additive, non-critical-only** change bumps
only the **patch** (`0.y.z`). A reader accepts only its own exact `0.y` — any other `y` is refused with
the message above — and within that `0.y`, unknown non-critical fields are skipped per rule 9. Once the
major reaches `1` (a brygge 1.0 release), the familiar rule resumes: major is breaking, minor is
additive, and a reader accepts any minor within its own major.

`brygge-ir`'s `CURRENT` is `0.2.0` (this document).

## 5. Records

Every field below is `critical = 1` (rule 8). "req" = required (rule 1); "opt" = optional (rule 2);
"list" = a list/map, omitted when empty (rule 3).

### `Ir` (the metadata root)

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | atoms | `list<ChangeAtom>` | list |
| 2 | refs | `list<RefRecord>` | list |
| 3 | provenance | `ImportProvenance` | req |
| 4 | dropped | `list<DropRecord>` | list |
| 5 | flags | `list<Flag>` | list |

### `ChangeAtom`

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | id | `id32` | req |
| 2 | parents | `list<id32>`, in source order | list |
| 3 | ops | `list<PathOp>` | list |
| 4 | copies | `list<CopyRecord>` | list |
| 5 | metadata | `MetadataClaims` | opt — omitted when every claim is absent |
| 6 | source | `SourceIdentity` | req |
| 7 | status | `EpistemicStatus` | req |

**`AtomId`** (field 1's value) = SHA-256 over the canonical encoding of this record **without field 1**:
that is, `uvarint(n-1)` followed by fields 2–7 (whichever of them are present), in the same ascending
order. A reader recomputes this from the other six fields and rejects a mismatch.

### `PathOp` (enum)

| Variant | Name | Fields |
|---|---|---|
| 0 | Add | `{1 path: text req, 2 blob: id32 req, 3 mode: uvarint req, 4 status: EpistemicStatus req}` |
| 1 | Modify | `{1 path, 2 blob, 3 mode, 4 status}` (same shape as Add) |
| 2 | Delete | `{1 path req, 4 status req}` (no blob/mode) |
| 3 | Replace | `{1 path, 2 blob, 3 mode, 4 status}` (RFC 011 D-7 — a source that distinguishes "replace in place" from "modify") |

### `CopyRecord`

A copy/rename hint, carried *beside* the literal ops that produced it (never replacing them). A **move**
is exactly: `from_atom` equals the atom's first parent, **and** the atom's own ops delete `from`.

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | from | text | req |
| 2 | from_atom | id32 | req |
| 3 | to | text | req |
| 4 | status | `EpistemicStatus` | req |

### `MetadataClaims`

The one-claim rule: `author`/`author_time` are what the source attributes the change to; `committer`/
`commit_time` are populated only when the source states a *distinct* recording identity or time.

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | author | `Identity` | opt |
| 2 | author_time | `Time` | opt |
| 3 | committer | `Identity` | opt |
| 4 | commit_time | `Time` | opt |
| 5 | message | `Text` | opt |

### `Identity`

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | name | `Text` | req |
| 2 | email | `Text` | opt |

### `Text`

Bytes as the source gave them — **not** assumed to be UTF-8 just because most sources are.

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | bytes | bytes | req — may be empty |
| 2 | encoding | text | opt — set only when the source declared one; absent never means "UTF-8", only "not stated" |

### `Time`

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | seconds | svarint | req — seconds since the Unix epoch, as the source stated (may be negative) |
| 2 | offset_minutes | svarint | opt — the source's own UTC offset in minutes; must fit `i16` |

### `SourceIdentity`

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | kind | `SourceKind` | req |
| 2 | repo_id | bytes | req |
| 3 | atom_id | bytes | req |
| 4 | signatures | `list<Signature>`, in source order | list |
| 5 | extras | `list<Extra>`, in source order | list |

### `SourceKind` (enum)

| Variant | Name | Fields |
|---|---|---|
| 0 | Git | (payload-less) |
| 1 | Hg | (payload-less) |
| 2 | Svn | (payload-less) |
| 3 | Cvs | (payload-less) |
| 4 | Other | `{1 label: text req}` |

### `Signature`

One opaque cryptographic signature the source carried over its own object (e.g. Git's `gpgsig`), never
verified by brygge.

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | label | text | req — e.g. `"gpgsig"`, `"gpgsig-sha256"` |
| 2 | bytes | bytes | req |

### `Extra`

One opaque, source-specific datum with no other home in the IR, carried rather than dropped.

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | label | text | req |
| 2 | bytes | bytes | req |

### `EpistemicStatus` (enum)

| Variant | Name | Fields |
|---|---|---|
| 0 | Stated | (payload-less) |
| 1 | Derived | the fields of `Derivation`, spliced in directly (not wrapped in a sub-field) |

### `Derivation`

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | kind | `DerivationKind` | req |
| 2 | by | text | req |
| 3 | decoder_version | text | req |
| 4 | params | map | list — omitted when empty |
| 5 | confidence | uvarint | opt — must be `≤ 100` |

### `DerivationKind` (enum)

| Variant | Name | Fields |
|---|---|---|
| 0 | InferredRename | (payload-less) — required params: `rename_algorithm`, `rename_threshold` |
| 1 | ReconstructedChangeset | (payload-less) — required params: `window_secs`, `cluster_keys` (`date_rule` from the CVS decoder's own handoff onward) |
| 2 | ReconstructedBranch | (payload-less) — required params: `layout` or `source` |
| 3 | InferredMerge | (payload-less) |
| 4 | NormalizedMetadata | (payload-less) |
| 5 | Other | `{1 note: text req}` |

### `RefRecord`

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | name | text | req |
| 2 | kind | `RefKind` | req |
| 3 | target | id32 | req |
| 4 | status | `EpistemicStatus` | req |
| 5 | source | `SourceIdentity` | opt |
| 6 | annotation | `Annotation` | opt |

### `RefKind` (enum)

| Variant | Name | Fields |
|---|---|---|
| 0 | Branch | (payload-less) |
| 1 | Tag | (payload-less) |
| 2 | Bookmark | (payload-less) |
| 3 | NamedBranch | (payload-less) |
| 4 | Other | `{1 label: text req}` |

### `Annotation`

A tag/ref's own annotation, when the source carries one distinct from the ref pointer itself (e.g. an
annotated Git tag object).

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | tagger | `Identity` | opt |
| 2 | time | `Time` | opt |
| 3 | message | `Text` | opt |

### `DropRecord`

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | class | `LossClass` | req |
| 2 | what | text | req |
| 3 | reason | text | req |

### `LossClass` (enum)

| Variant | Name |
|---|---|
| 0 | Representation |
| 1 | AdvisoryUnreliable |
| 2 | Other |

(All payload-less.)

### `Flag`

One flagged condition (RFC 011 D-8) — drives a CLI's convention/confidence exit class. Unlike a
`DropRecord`, nothing here was dropped; a flag says "this import needs attention", not "this datum is
missing".

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | kind | `FlagKind` | req |
| 2 | what | text | req |
| 3 | count | uvarint | req — at least 1 |
| 4 | reason | text | req |

### `FlagKind` (enum)

| Variant | Name |
|---|---|
| 0 | ConventionViolation |
| 1 | BelowConfidenceFloor |

(All payload-less.)

### `ImportProvenance`

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | source | `SourceIdentity` | req |
| 2 | brygge_version | text | req |
| 3 | decoder | text | req |
| 4 | decoder_version | text | req |
| 5 | params | map | list |

## 6. Canonical orders and structural checks

Enforced on read; a compliant writer produces them directly (no reordering pass needed on the read
side — a mismatch is a canonical-form violation, not something a reader repairs).

- **Atoms** are in the builder's canonical order: **topological**, with the ready set at each step
  ordered by `(source.atom_id bytes, AtomId)` — a full, deterministic tiebreak. A reader **recomputes**
  this order from the parent links and rejects any artifact whose atoms are not already in it. Atom ids
  are unique. Every parent — and every copy's `from_atom` — names an atom **strictly earlier** in this
  order, never the atom itself and never a later one.
- **Ops** within one atom are strictly ascending by path (so there is at most one op per path per atom).
  **Copies** within one atom are strictly ascending by `(to, from, from_atom)`.
- **Refs** are strictly ascending by `(name, kind variant)`, and every ref's `target` names an atom
  present in the artifact.
- **Dropped** entries are strictly ascending by `(class variant, what)`. **Flags** are strictly ascending
  by `(kind variant, what)`.
- **Blobs:** every op that carries a `blob` (`Add`/`Modify`/`Replace`) names one present in the blob
  store, and **every stored blob is referenced by at least one op** — no orphans either way.

## 7. Worked example: a minimal one-atom artifact

Built by hand (no decoder): one atom adding the path `a` with content `hi`, no parents, no refs, no
drops, no flags, `Stated` throughout. 219 bytes total, shown here as verified hex (every offset below was
checked against the actual bytes, not estimated):

```
0000: 42 52 59 47 47 45 49 52 02 00 02 00 1a 02 79 48
0010: a3 e1 7c ae 79 dd f2 14 88 50 10 7f c3 6a 71 81
0020: 49 7d e1 f1 61 15 4f 2d f1 44 7b a2 89 01 02 03
0030: 69 01 04 03 20 7c 58 68 f0 f3 1e 81 5a 16 ae eb
0040: d8 95 ba a8 93 15 e3 38 ab 8e 3b 2d 26 f5 e0 c4
0050: 6e 71 97 ed d6 07 31 01 00 04 03 01 61 05 20 8f
0060: 43 43 46 64 8f 6b 96 df 89 dd a9 01 c5 17 6b 10
0070: a6 d8 39 61 dd 3c 1a c8 8b 59 b2 dc 32 7a a4 07
0080: 03 a4 83 02 09 02 00 00 0d 0c 03 03 02 00 00 05
0090: 01 72 07 02 63 31 0f 02 00 00 07 1b 04 03 0b 03
00a0: 03 02 00 00 05 01 72 07 01 61 05 05 30 2e 31 2e
00b0: 30 07 01 64 09 01 30 01 8f 43 43 46 64 8f 6b 96
00c0: df 89 dd a9 01 c5 17 6b 10 a6 d8 39 61 dd 3c 1a
00d0: c8 8b 59 b2 dc 32 7a a4 02 68 69
```

Annotated, offset by offset. Rows inside a record are summarised; not every byte has its own row (the
`Add` op's inner fields and `ImportProvenance`'s are not each spelled out, for instance):

| Offset | Bytes | Meaning |
|---|---|---|
| `0x00`–`0x07` | `42 52 59 47 47 45 49 52` | Magic, `"BRYGGEIR"`. |
| `0x08` | `02` | Format `2`. |
| `0x09`–`0x0b` | `00 02 00` | Version: major `0`, minor `2`, patch `0` — each a one-byte `uvarint` since all three fit in 7 bits. |
| `0x0c`–`0x2b` | (32 bytes) | The digest — `SHA-256` of everything from `0x09` to the end of the file, **excluding the 32 digest bytes themselves (`0x0c`–`0x2b`)**, as §3 states. |
| `0x2c`–`0x2d` | `89 01` | `uvarint(137)`: the metadata (`Ir` record) is 137 bytes, spanning `0x2e`–`0xb6`. |
| `0x2e` | `02` | The `Ir` record's own field count: **2** (only `atoms` and `provenance` are present — `refs`/`dropped`/`flags` are all empty and so omitted, rule 3). |
| `0x2f` | `03` | Field tag `3` → id `1` (`atoms`), critical `1`. |
| `0x30` | `69` | Field length `105` (one byte). The `atoms` list spans `0x31`–`0x99`. |
| `0x31` | `01` | `list<ChangeAtom>` count: **1**. |
| `0x32` | `04` | The one `ChangeAtom`'s own field count: **4** (`id`, `ops`, `source`, `status` — `parents`/`copies`/`metadata` are all empty/absent and omitted). |
| `0x33`–`0x34` | `03 20` | Field tag `3` → id `1` (`id`), length `32`: the atom's `AtomId` follows, `0x35`–`0x54`. |
| `0x55`–`0x56` | `07 31` | Field tag `7` → id `3` (`ops`), length `49`: the `ops` list spans `0x57`–`0x87`. |
| `0x57` | `01` | `list<PathOp>` count: **1**. |
| `0x58` | `00` | The `PathOp` enum's variant: **0** (`Add`). |
| `0x59` | `04` | `Add`'s own field count: **4** (`path`, `blob`, `mode`, `status`). |
| `0x88` | `0d` | Field tag `13` → id `6` (`source`), length `12` (next byte `0x89`): the atom's `SourceIdentity`, `0x8a`–`0x95`. |
| `0x96`–`0x97` | `0f 02` | Field tag `15` → id `7` (`status`), length `2`: `EpistemicStatus`, `0x98`–`0x99` = `00 00` (`Stated`: variant `0`, an empty record). |
| `0x9a`–`0x9b` | `07 1b` | Back in the `Ir` record: field tag `7` → id `3` (`provenance`), length `27`: `ImportProvenance`, `0x9c`–`0xb6`. |
| `0xb7` | `01` | Blob-store `uvarint(count)`: **1**. |
| `0xb8`–`0xd7` | (32 bytes) | The blob's `id32` — `8f43…7aa4`, the `SHA-256` of the two content bytes below (this is in fact the well-known `SHA-256("hi")`). |
| `0xd8` | `02` | Blob length: **2**. |
| `0xd9`–`0xda` | `68 69` | The content, `"hi"` — the file's end. |

A reader following §3's read order would: verify the magic and format; parse the version and confirm
`0.2.x`; recompute the digest over `0x09`–end (excluding the digest field itself) and compare; parse the
137-byte `Ir` record (which recurses into the one `ChangeAtom`, its `ops`, `source`, and `status`, and the
`ImportProvenance`); parse the one blob and verify its id; then run the structural checks of §6 — a
single atom with no parents is trivially "in canonical order", its `Add` op's blob is present, and the
one stored blob is referenced by that op, so nothing is orphaned.

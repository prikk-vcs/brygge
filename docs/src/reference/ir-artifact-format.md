# IR artifact format — contract 0.3.0

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

## 3. Container (RFC 018 D-1: compact artifacts)

```
"BRYGGEIR" (8 bytes)
· format: u8 = 2
· version: uvarint(major) ‖ uvarint(minor) ‖ uvarint(patch)   -- 0, 3, 0
· digest: 32 bytes
· metadata frames: frames
· blob table: uvarint(count) ‖ ( id32 ‖ uvarint(raw_len) )*   -- in store order (§3b)
· blob frames: frames

frames := uvarint(nframes) ‖ frame*
frame  := uvarint(codec) ‖ uvarint(raw_len) ‖ uvarint(stored_len) ‖ <stored bytes, exactly stored_len>
codec  := 0 (stored: the bytes as they are) | 1 (DEFLATE, RFC 1951, raw — no zlib header or checksum)
```

**The logical stream** is what the digest covers, and what every length and content byte above is
*defined by* (not necessarily *stored as*, when a frame is DEFLATE-compressed):

```
version bytes ‖ uvarint(meta_len) ‖ <Ir record, exactly meta_len bytes>
‖ uvarint(count) ‖ ( id32 ‖ uvarint(raw_len) )*
‖ <every blob's bytes, concatenated, in store order>
```

`meta_len` is not itself stored anywhere — it is the sum of the metadata frames' own `raw_len`s, and the
reader recomputes it the same way the digest hash does: by summing those frames as it inflates them.
Frame headers (`nframes`/`codec`/`raw_len`/`stored_len`) and a `Codec::Stored` frame's identical stored
bytes are never hashed directly; only each frame's **decompressed** output is, in the order its frame
appears. The blob table's bytes (`count`, each `id32 ‖ raw_len`) are not compressed at all — they sit in
the logical stream exactly as stored.

### 3a. Packing (recomputed and enforced on read)

- **Metadata** is cut into frames of exactly 1 MiB (`1 << 20`) raw bytes each, the last one shorter. An
  empty metadata record is impossible (`provenance` is required), so there is always at least one frame.
- **Blobs**, in store order (§3b): a blob of 1 MiB or less joins the current frame if the frame's raw
  total would stay at or under 1 MiB, or starts a new one if it would not; a blob **over** 1 MiB is a
  frame of its own. A blob never spans two frames. An empty blob adds nothing, and belongs to whatever
  frame is open, or opens a new (empty) one if none is.
- A reader recomputes this plan from the metadata's total length and the blob table's `raw_len`s, and
  compares it against the actual frame count and each frame's `raw_len`. Any mismatch — two frames that
  should have been one, a blob split across a frame boundary, a frame holding packed blobs but claiming
  more than 1 MiB — is `Error::NonCanonical` ("frames do not match the packing rule").

### 3b. Store order (RFC 018 D-2)

The blob table's ids, and the blob frames' content, are in **store order**: each blob's position is set
by its first use — the earliest `Add`/`Modify`/`Replace` naming it, found by walking the `Ir`'s atoms in
their own canonical order and, within an atom, its ops in their own canonical path order — and blobs are
then sorted by that first use's **path** (as bytes), breaking ties by the atom's index. Grouping by path
rather than by history is what lets a DEFLATE frame compress a file's successive revisions together. A
reader recomputes this order from the decoded `Ir` and rejects a table that is not in it.

### 3c. Bounds (checked before any allocation)

- An unrecognized `codec`: `Error::Decode("unknown frame codec N")`.
- `codec = 0`: `stored_len` must equal `raw_len` exactly, or `Error::Decode`.
- `codec = 1`: `raw_len` must not exceed `1032 × stored_len + 64` — DEFLATE's documented maximum
  expansion — or `Error::Decode`, refused before any output buffer is allocated.
- **Inflating:** the decompressed output must be exactly `raw_len` bytes (no more, no less), and every
  stored byte must be consumed; an inflate that overruns, under-runs, or leaves stored bytes unconsumed
  is `Error::Decode`.
- **Error precedence:** a frame that does not inflate at all is reported immediately as
  `Error::Decode("<metadata|blob> frame N: …")` — there is no logical stream left to hash, so the digest
  is never consulted for this case. Otherwise, the usual rule holds: `Error::DigestMismatch` is checked,
  and takes precedence, **before** any other structural error (packing, store order, referential
  integrity) is reported — a corrupted-but-correctly-signed artifact is what those structural checks
  exist to catch; one that is not even correctly signed is reported as that, first.

Blob ids need not be ascending any more (store order, not id order, is canonical — §3b); each id still
equals `SHA-256(bytes)`.

**Read order** — nothing later is ever inspected before an earlier check has passed:

1. **The magic.** Anything else: `Error::Decode`.
2. **The format byte.** `1` means a **pre-release** artifact (from before contract 0.2.0/RFC 011):
   `Error::PreReleaseArtifact`, and **the metadata is never parsed** — the wire shape underneath changed
   too much between formats to read any further safely. Any value other than `1` or `2`:
   `Error::Decode("unknown artifact format N")`.
3. **The version.** A major or minor this build does not list as readable gives
   `Error::UnsupportedContract { found, supported }` ("artifact contract 0.Y is not supported by this
   build (reads 0.2, 0.3)"). Any patch is accepted. See §4 for the exact acceptance rule.
4. **Every frame, inflated** (§3c) — immediate refusal here bypasses the digest, as noted above.
5. **The digest**, over the logical stream reconstructed above. A mismatch: `Error::DigestMismatch`,
   before any of the checks below.
6. **The metadata** — the `Ir` record (§5).
7. **The blob table's packing and store order** (§3a/§3b).
8. **The structural checks** (§6) — canonical order and referential integrity across the whole artifact.

## 4. Version acceptance (RFC 011 D-3, widened by RFC 018 D-7)

While the contract's major version is `0`: a **breaking** change (a critical field or variant added,
changed, or removed) bumps the **minor** (`0.y`); a **purely additive, non-critical-only** change bumps
only the **patch** (`0.y.z`). A reader accepts a minor listed in its own build's `READABLE` set — any
other `y` is refused with the message above — and within an accepted `0.y`, unknown non-critical fields
are skipped per rule 9. Once the major reaches `1` (a brygge 1.0 release), the familiar rule resumes:
major is breaking, minor is additive, and a reader accepts any minor within its own major.

`brygge-ir`'s `CURRENT` (what it **writes**) is `0.3.0`; `READABLE` (what it **reads**) is `{0.2, 0.3}`
— RFC 018 D-7, OQ-2: a build that writes 0.3.0 keeps reading 0.2.0, the layout §8 describes.
[`Decoded::digest`](https://docs.rs/brygge-ir) documents both contracts' digest definitions together,
since they differ (this document's §3 for 0.3.0, §8 for 0.2.0).

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
| 6 | transforms | `list<TransformRecord>`, oldest first | list — RFC 018 D-5; omitted for a decoder's own artifact (no chain); at most 64 records (`Error::NonCanonical`, checked at decode time) |

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
| 2 | Transformed | `{1 step: uvarint req, 2 changed: list<text> req, 3 prior: EpistemicStatus req}` (RFC 018 D-5) |

**`Transformed`** (RFC 018 D-5, batch T): a transform step changed this assertion. `step` indexes
`Ir.transforms` (field 6, above); `prior` is the status this assertion had *before*
that step — itself `Stated`, `Derived`, or (for a multi-step chain) another `Transformed`, nested as deep
as the chain itself. `changed` names which part(s) of this assertion the step touched, from a closed
vocabulary (one for each of the three places `Transformed` may appear):

| Context | Vocabulary |
|---|---|
| An atom's own status | `author`, `author-time`, `committer`, `commit-time`, `message`, `ops`, `copies` |
| A `PathOp`'s own status | `blob`, `mode`, `path` |
| A `RefRecord`'s own status | `annotation`, `kind`, `name`, `target` |

A `CopyRecord`'s own status may never be `Transformed` at all — copies are not a valid location for it
(§6). `changed`'s own wire shape is `list<text>`, but — unlike every other `list<T>` in this schema —
each item is **self-delimiting** (`uvarint(len) ‖ bytes`, the same shape a bare `text` value would have as
a record field) rather than itself a record or a fixed-size id: nothing else encloses it to say where one
item ends and the next begins. `changed` is non-empty and strictly ascending by the item's own text (so no
part repeats).

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
| 4 | step | uvarint | req **iff** `class` is `Transform`, **absent** for every other class (RFC 018 D-5) — which transform step (an index into `Ir.transforms`) dropped this |

### `LossClass` (enum)

| Variant | Name |
|---|---|
| 0 | Representation |
| 1 | AdvisoryUnreliable |
| 2 | Other |
| 3 | Transform — RFC 018 D-5: a transform step dropped this; `DropRecord.step` names which one |

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

### `TransformRecord`

One transform program's own run over an artifact (RFC 018 D-5, batch T): what program, what version,
with what parameters, named by `consumed` — the §3 D-3 digest of the artifact this step **read**.
`Ir.transforms` holds these oldest first. A record's own *output* is never stored directly: record *k*'s
output is record *k + 1*'s own `consumed`, and the last record's output is the artifact's own overall
digest (§3).

| id | Field | Type | Kind |
|---|---|---|---|
| 1 | name | text | req — the plugin-name grammar `[a-z0-9][a-z0-9-]*`, at most 64 bytes |
| 2 | version | text | req — non-empty |
| 3 | params | map | list — omitted when empty |
| 4 | consumed | id32 | req — the D-3 digest of the artifact this step read |

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
- **Dropped** entries are strictly ascending by `(class variant, step, what)` — RFC 018 D-5 widened this
  from `(class variant, what)`; an absent `step` sorts **before** any present one, so every
  non-`Transform` drop (which never has a `step`) sorts ahead of every `Transform` drop at the same class
  variant (moot in practice, since `Transform` is its own variant, but this is the rule a reader checks).
  **Flags** are strictly ascending by `(kind variant, what)`.
- **Blobs:** every op that carries a `blob` (`Add`/`Modify`/`Replace`) names one present in the blob
  store, and **every stored blob is referenced by at least one op** — no orphans either way.

### Transform chain (RFC 018 D-5)

A reader enforces these over `Ir.transforms` and every place `EpistemicStatus::Transformed` or a
`Transform`-class drop can appear, beyond each record's own field shape (§5):

- **At most 64 transform records.** More is `Error::NonCanonical`, checked as `Ir.transforms` is decoded
  — before any of the checks below run.
- **`TransformRecord.name`** matches the plugin-name grammar (`[a-z0-9][a-z0-9-]*`, at most 64 bytes) and
  **`TransformRecord.version`** is non-empty, or `Error::Decode`/`Error::NonCanonical`.
- **Every `Transformed.step`** — on an atom's, an op's, or a ref's own status — is **strictly less than**
  `Ir.transforms.len()`. A `Transform`-class drop's `step` is checked the same way. Either violation is
  `Error::NonCanonical` ("…step N is not less than the chain's length M").
- **A nested `prior`'s own `step`, if `prior` is itself `Transformed`, is strictly less than the outer
  layer's `step`** — so a chain's steps strictly decrease as you walk from the outermost `Transformed`
  down through `prior` to the eventual `Stated`/`Derived` base. Violating this is `Error::NonCanonical`.
  (Decode also caps how deep this `prior` nesting may go, at 64 layers, independent of and before the
  check above against the chain's real length — a defensive bound against a maliciously deep chain, not
  a looser rule than the one above.)
- **Every `Transform`-class drop has a `step`, and no drop of any other class does** — enforced where
  `DropRecord` itself is decoded (§5), not here; this section's own job is only the range check above.
- **`Transformed.changed`** is non-empty, strictly ascending by its own text, and every entry parses
  against the closed vocabulary for *where* the `Transformed` sits (an atom's own vocabulary, an op's, or
  a ref's — listed with `EpistemicStatus` in §5). An entry outside that context's vocabulary is
  `Error::Decode` ("…uses a part outside its vocabulary"); a non-ascending, duplicate, or empty `changed`
  is `Error::NonCanonical`.
- **A `CopyRecord`'s own status may never be `Transformed`.** Unlike an atom's, an op's, or a ref's own
  status, a copy record is not a valid location for it at all — any `Transformed` found there is
  `Error::NonCanonical` ("Transformed may not appear on a copy record's own status"), regardless of its
  own `step`/`changed`/`prior`.
- **Contract 0.2.0 has none of this.** `Ir` field 6 (`transforms`), `EpistemicStatus` variant 2
  (`Transformed`), `LossClass` variant 3 (`Transform`), and `DropRecord` field 4 (`step`) are all 0.3.0
  additions (§4). A 0.2.0 artifact that smuggles any one of them in (hand-crafted bytes, since the shared
  record decoders are contract-oblivious by design — §5's decoders do not themselves know which contract
  they are reading) is refused, naming exactly which one: `Error::NonCanonical("contract 0.2.0 does not
  have Ir field 6 (transforms)")`, `"...EpistemicStatus variant 2 (Transformed)"`, `"...LossClass variant
  3 (Transform)"`, or `"...DropRecord field 4 (step)"`.

**Two things worth stating plainly about this chain:**

1. **A record's own output digest is never stored — only implied.** `TransformRecord.consumed` names the
   digest of the artifact a step *read*; the digest of what it *produced* is simply record *k + 1*'s own
   `consumed`, or, for the chain's last record, the artifact's own overall digest (§3). There is no
   separate "output" field anywhere, and none is needed.
2. **A chain's full losses are enumerable from the artifact alone.** Every `Transform`-class drop names
   its own `step`, so a reader can list, for each record in `Ir.transforms`, exactly what that step
   dropped and why — without running the transform programs themselves, or even knowing what they are
   beyond `name`/`version`/`params`.

**What a reader cannot check alone:** whether an atom, op, or ref with *no* `Transformed` mark was truly
left unchanged by a given step. The chain's own rules (above) only constrain what a `Transformed` mark
*claims*; they say nothing about an assertion that carries none. Confirming that a step actually touched
only what it marked, and nothing else, needs that step's own input to compare against — the artifact
`TransformRecord.consumed` names — which is a check over two artifacts, not one. That is batch P's own
output-side verification, not something this reader-only format page can state or enforce.

## 7. Worked example: a minimal one-atom artifact (contract 0.3.0)

Built by hand (no decoder): one atom adding the path `a` with content `hi`, no parents, no refs, no
drops, no flags, `Stated` throughout. 222 bytes total, shown here as verified hex (generated and
hex-dumped, not estimated):

```
0000: 42 52 59 47 47 45 49 52 02 00 03 00 81 6f 2b 29
0010: 42 47 1c 55 37 9d da 0e a6 68 01 11 2a 9a 65 8f
0020: 21 5a 9f 3a 16 69 fb c9 bd 87 38 cb 01 00 84 01
0030: 84 01 02 03 66 01 04 03 20 8d f0 ce 43 52 5d 8a
0040: a7 3f a1 a9 af 11 9c c3 3c 9d 3b 1c a6 f0 74 2a
0050: d8 0e 1c 52 aa 25 9a e6 6e 07 31 01 00 04 03 01
0060: 61 05 20 8f 43 43 46 64 8f 6b 96 df 89 dd a9 01
0070: c5 17 6b 10 a6 d8 39 61 dd 3c 1a c8 8b 59 b2 dc
0080: 32 7a a4 07 03 a4 83 02 09 02 00 00 0d 09 03 03
0090: 02 00 00 05 00 07 00 0f 02 00 00 07 19 04 03 09
00a0: 03 03 02 00 00 05 00 07 00 05 05 30 2e 31 2e 30
00b0: 07 01 64 09 01 31 01 8f 43 43 46 64 8f 6b 96 df
00c0: 89 dd a9 01 c5 17 6b 10 a6 d8 39 61 dd 3c 1a c8
00d0: 8b 59 b2 dc 32 7a a4 02 01 00 02 02 68 69
```

Annotated, offset by offset. The `Ir` record's own internal fields (§5) are not re-derived here — this
example's new ground, over contract 0.2.0's, is the frame structure wrapped around it:

| Offset | Bytes | Meaning |
|---|---|---|
| `0x00`–`0x07` | `42 52 59 47 47 45 49 52` | Magic, `"BRYGGEIR"`. |
| `0x08` | `02` | Format `2`. |
| `0x09`–`0x0b` | `00 03 00` | Version: major `0`, minor `3`, patch `0`. |
| `0x0c`–`0x2b` | (32 bytes) | The digest — `SHA-256` of the **logical stream** (§3), never these stored bytes directly. |
| `0x2c` | `01` | Metadata frames: `uvarint(nframes)` = **1**. |
| `0x2d` | `00` | That frame's codec: **0** (stored — DEFLATE did not shrink 132 bytes this repetitive). |
| `0x2e`–`0x2f` | `84 01` | `raw_len` = **132** (two-byte `uvarint`: `0x84,0x01` → `0x04 \| (0x01 << 7)` = 132). |
| `0x30`–`0x31` | `84 01` | `stored_len` = **132**, matching `raw_len` (codec 0's own rule). |
| `0x32`–`0xb5` | (132 bytes) | The metadata frame's stored (= raw, here) bytes: the `Ir` record, byte-identical to what contract 0.2.0 would encode for the same `Ir` (§5's schema did not change) — same internal layout as the 0.2.0 example in §8, just a different example `Ir` here. |
| `0xb6` | `01` | Blob table: `uvarint(count)` = **1**. |
| `0xb7`–`0xd6` | (32 bytes) | The blob's `id32` — `8f43…7aa4`, `SHA-256("hi")`. |
| `0xd7` | `02` | That blob's `raw_len` = **2** — the table entry, now *without* the bytes themselves (§3, vs. 0.2.0's interleaved `id ‖ len ‖ bytes` in §8). |
| `0xd8` | `01` | Blob frames: `uvarint(nframes)` = **1** (one blob, one frame — well under the 1 MiB cap, §3a). |
| `0xd9` | `00` | Codec: **0** (stored — 2 bytes of content never compresses smaller). |
| `0xda` | `02` | `raw_len` = **2**. |
| `0xdb` | `02` | `stored_len` = **2**. |
| `0xdc`–`0xdd` | `68 69` | The content, `"hi"` — the file's end. |

A reader following §3's read order would: verify the magic and format; parse the version and confirm it
is in `READABLE`; inflate the one metadata frame and the one blob frame (both trivially, codec 0);
reconstruct the logical stream and recompute its digest, comparing it with `0x0c`–`0x2b`; parse the
132-byte `Ir` record (recursing into the one `ChangeAtom`, its `ops`, `source`, and `status`, and the
`ImportProvenance`); check the blob table's one entry is in store order (trivially, with one blob) and
its packing matches §3a's plan (trivially, one frame); verify the blob's content hashes to its declared
id; then run the structural checks of §6 — a single atom with no parents is trivially "in canonical
order", its `Add` op's blob is present, and the one stored blob is referenced by that op, so nothing is
orphaned.

### 7a. Worked example: a one-record transform chain (RFC 018 D-5)

Built with `IrBuilder` (not a decoder, and not the example above — a fresh artifact): the same one atom
adding path `a` with content `hi`, no refs, no drops, no flags — but this time the atom's own status is
`Transformed { step: 0, changed: [author], prior: Stated }`, and `Ir.transforms` holds the one
`TransformRecord` that made it: name `"redact"`, version `"1.0.0"`, no params, `consumed` =
`SHA-256("example-artifact-before-step-0")` (an arbitrary 32 bytes for this illustration — not a digest
of anything real in this example, just something that looks like one). 302 bytes total, shown here as
verified hex (generated from real code and hex-dumped, then cross-checked field by field against §5's
rules by hand, offset by offset — not estimated):

```
0000: 42 52 59 47 47 45 49 52 02 00 03 00 ae c8 bd 0c
0010: d4 38 8e 0c 85 46 a8 a5 eb 7d 4b fe 3a 1f 01 04
0020: dc 7f 78 4a 17 61 74 f0 35 a7 43 ba 01 00 d4 01
0030: d4 01 03 03 79 01 04 03 20 07 20 2a 6d fc 82 6a
0040: bf 3a 13 12 b1 5b 56 66 b2 3d 39 09 dc 26 3e 5a
0050: 17 c4 72 9f cd ef 39 c7 e3 07 31 01 00 04 03 01
0060: 61 05 20 8f 43 43 46 64 8f 6b 96 df 89 dd a9 01
0070: c5 17 6b 10 a6 d8 39 61 dd 3c 1a c8 8b 59 b2 dc
0080: 32 7a a4 07 03 a4 83 02 09 02 00 00 0d 0b 03 03
0090: 02 00 00 05 00 07 02 72 31 0f 13 02 03 03 01 00
00a0: 05 08 01 06 61 75 74 68 6f 72 07 02 00 00 07 21
00b0: 04 03 0d 03 03 02 00 00 05 00 07 04 72 65 70 6f
00c0: 05 05 30 2e 31 2e 30 07 01 64 09 05 30 2e 31 2e
00d0: 30 0d 33 01 03 03 06 72 65 64 61 63 74 05 05 31
00e0: 2e 30 2e 30 09 20 23 a3 29 a6 8c 0a e6 6f ba 5e
00f0: 69 34 ef 9b 0e 38 88 ec 08 3b 47 16 15 1c 66 7e
0100: db 97 8c 58 c8 71 01 8f 43 43 46 64 8f 6b 96 df
0110: 89 dd a9 01 c5 17 6b 10 a6 d8 39 61 dd 3c 1a c8
0120: 8b 59 b2 dc 32 7a a4 02 01 00 02 02 68 69
```

Annotated, offset by offset. The parts already walked in §7 (the container header, the atom's `id`/`ops`/
`source`, the blob table and blob frame) are summarized rather than re-derived; this example's new ground
is the `status` field's `Transformed` encoding and the `Ir` record's `transforms` field:

| Offset | Bytes | Meaning |
|---|---|---|
| `0x00`–`0x0b` | — | Magic, format `2`, version `0.3.0` — as §7. |
| `0x0c`–`0x2b` | (32 bytes) | The digest, over the logical stream (§3). |
| `0x2c`–`0x31` | `01 00 d4 01 d4 01` | Metadata frames: `1`, codec `0` (stored), `raw_len`/`stored_len` both `212`. |
| `0x32` | `03` | The `Ir` record's own field count: **3** (`atoms`, `provenance`, `transforms` — `refs`/`dropped`/`flags` are all empty and omitted). |
| `0x33`–`0x34` | `03 79` | Field tag `3`→id `1` (`atoms`), length `121`: the one `ChangeAtom`, `0x35`–`0xad`. |
| `0x36` | `04` | The `ChangeAtom`'s own field count: **4** (`id`, `ops`, `source`, `status`). |
| `0x37`–`0x58` | — | Field `id` (32 bytes) — as §7, a different atom's `AtomId`. |
| `0x59`–`0x8b` | — | Field `ops`: one `Add` of path `a`, blob `8f43…7aa4` (`SHA-256("hi")`), mode `0o100644` (`a4 83 02`), **the op's own status `Stated`** — this example's transform touched only the atom's status, not this op's. |
| `0x8c`–`0x98` | — | Field `source`: `SourceIdentity{kind: Git, repo_id: "", atom_id: "r1"}`. |
| `0x99`–`0xad` | `0f 13` … | Field `status` (tag `15`→id `7`, length `19`): **`EpistemicStatus::Transformed`** — broken out below. |
| `0x9b` | `02` | `EpistemicStatus` variant **2** (`Transformed`). |
| `0x9c` | `03` | Its own record's field count: **3** (`step`, `changed`, `prior`). |
| `0x9d`–`0x9f` | `03 01 00` | Field `step` (tag `3`→id `1`, length `1`): **`0`** — the chain's first (and only) record. |
| `0xa0`–`0xa9` | `05 08 01 06` `"author"` | Field `changed` (tag `5`→id `2`, length `8`): `list<text>` count **1**, one self-delimiting item `uvarint(6) ‖ "author"` — the vocabulary's spelling for an atom's claimed author. |
| `0xaa`–`0xad` | `07 02 00 00` | Field `prior` (tag `7`→id `3`, length `2`): `EpistemicStatus::Stated` — what this atom's status was *before* step 0. |
| `0xae`–`0xd0` | `07 21` … | Back in the `Ir` record: field `provenance` (tag `7`→id `3`, length `33`) — `ImportProvenance{source: {kind: Git, atom_id: "repo"}, brygge_version: "0.1.0", decoder: "d", decoder_version: "0.1.0"}`, same shape as §5/§7. |
| `0xd1`–`0x105` | `0d 33` … | Field **`transforms`** (tag `13`→id **`6`**, length `51`) — broken out below. |
| `0xd3` | `01` | `transforms`: `list<TransformRecord>` count = **1**. |
| `0xd4` | `03` | The one `TransformRecord`'s own field count: **3** (`name`, `version`, `consumed`; `params` is empty and omitted). |
| `0xd5`–`0xdc` | `03 06` `"redact"` | Field `name` (tag `3`→id `1`, length `6`): `"redact"`. |
| `0xdd`–`0xe3` | `05 05` `"1.0.0"` | Field `version` (tag `5`→id `2`, length `5`): `"1.0.0"`. |
| `0xe4`–`0x105` | `09 20` + (32 bytes) | Field `consumed` (tag `9`→id `4`, length `32`): the step's own `SHA-256("example-artifact-before-step-0")`. |
| `0x106` | `01` | Blob table: `uvarint(count)` = **1**. |
| `0x107`–`0x126` | (32 bytes) | The blob's `id32` — `8f43…7aa4`, `SHA-256("hi")`. |
| `0x127` | `02` | That blob's `raw_len` = **2**. |
| `0x128`–`0x12b` | `01 00 02 02` | Blob frames: `1`, codec `0` (stored), `raw_len`/`stored_len` both `2`. |
| `0x12c`–`0x12d` | `68 69` | The content, `"hi"` — the file's end. |

A reader would decode exactly as §7 up through the blob table, and additionally: on reaching the atom's
`status` field, decode the `Transformed` enum (step `0`, one changed part `"author"`, `prior` `Stated`);
on reaching the `Ir` record's field `6`, decode the one `TransformRecord`; then, as part of §6's
structural checks, confirm `step` (`0`) is less than `Ir.transforms.len()` (`1`), confirm `"author"` is in
the atom-context vocabulary, and confirm the one atom's status is the only `Transformed` anywhere (so
nothing needed checking against a nested `prior`'s own step, and no `Transform`-class drop's step needed
checking either, since there is no drop at all in this example).

## 8. Contract 0.2.0 (read only)

This build still **reads** contract 0.2.0 (RFC 018 D-7, OQ-2) — the layout before RFC 018's frames —
but never **writes** it; `write_to` only ever produces §3's contract 0.3.0. Kept here exactly, for a
reader that still meets one of these artifacts.

### 8a. Container

```
"BRYGGEIR" (8 bytes)
· format: u8 = 2
· version: uvarint(major) ‖ uvarint(minor) ‖ uvarint(patch)   -- 0, 2, z
· digest: 32 bytes
· metadata: uvarint(len) ‖ <Ir record, exactly len bytes>
· blobs: uvarint(count) ‖ ( id32 ‖ uvarint(len) ‖ bytes )*
```

Blob ids are **strictly ascending**, and each id equals `SHA-256(bytes)`.

**The digest** is SHA-256 over the **stored bytes**, from the first version byte to the end of the
input, **excluding the 32 digest bytes themselves** (i.e. `version ‖ metadata ‖ blobs`, concatenated
exactly as stored — never a re-encoding, and never frame-wrapped). A mismatch: `Error::DigestMismatch`,
with the same precedence as §3's final bullet (checked before any structural error).

### 8b. Worked example: a minimal one-atom artifact

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
| `0x0c`–`0x2b` | (32 bytes) | The digest — `SHA-256` of everything from `0x09` to the end of the file, **excluding the 32 digest bytes themselves (`0x0c`–`0x2b`)**, as §8a states. |
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

A reader following §8a's layout would: verify the magic and format; parse the version and confirm
`0.2.x`; recompute the digest over `0x09`–end (excluding the digest field itself) and compare; parse the
137-byte `Ir` record (which recurses into the one `ChangeAtom`, its `ops`, `source`, and `status`, and the
`ImportProvenance`); parse the one blob and verify its id; then run the structural checks of §6 — a
single atom with no parents is trivially "in canonical order", its `Add` op's blob is present, and the
one stored blob is referenced by that op, so nothing is orphaned.

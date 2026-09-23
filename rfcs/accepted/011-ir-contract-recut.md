# RFC 011 — IR contract re-cut before the first release

**Status.** **Accepted (2026-09-23)** by the owner, under owner ruling **D-1 (a)**. **OQ-A is ruled:** the
released contract label is **`0.2.0`** under D-3's `0.y` rule (the recommendation, accepted with the RFC).
The implementation handoff is `../handoffs/011-ir-contract-recut/brygge-ir-recut-handoff-v1.md`.
**Tracks.** ROADMAP release 0.1.0 ("honest decode"). It supersedes parts of RFC 001 (D-3), RFC 002 (D-3)
and RFC 003 (D-1, D-3, D-4, D-6, D-7), recorded under *Consequences and supersessions*. It realizes intake-review corrections CR-09, CR-03
(text), CR-04 (field semantics) and CR-06 (carrying).
**Touches.** `brygge-ir` (the codec, model, artifact, honesty report); every decoder (which produces the
new records); the CLI (exit classes from typed records). No decode *policy* changes here: the decoders'
own corrections are in their handoffs.

## Summary

The IR held all four sources, but the intake review found that the contract as written cannot keep
three of its promises:

1. **"Additive within major 1" is impossible with a positional codec.** Fields have no tags or lengths,
   and the metadata section rejects trailing bytes. So adding a field to any struct breaks every existing
   reader, and even a new enum variant is refused by an old reader. The evolution promise was a label,
   not a property.
2. **Artifact bytes are malleable.** The reader accepts non-minimal varints and unsorted or duplicate map
   keys and blobs. The digest is recomputed from the *decoded* model, not taken over the stored bytes.
3. **Honesty has gaps the contract cannot express:**
   - the typed refusal and violation records (they travel today as `LossClass::Other` plus magic strings
     the CLI matches);
   - copy vs rename, and where a copy came from;
   - timezone offsets;
   - text that is not UTF-8;
   - annotated-tag metadata, `mergetag` signatures and source extras;
   - SVN's "this path is a new node".

   Each gap forces a decoder to drop, fabricate or approximate.

No brygge release has shipped (the owner, 2026-09-23: v0, never in production use, breaking changes to
improve or fix are acceptable). The owner therefore ruled to re-cut the contract once, now, rather than
carry these into the first release.

## Constraints

- **Honesty first (INV-1).** A reader must never silently skip anything that could change what an
  artifact means. Forward compatibility is allowed only for information whose omission cannot make a
  reader over-trust.
- **Determinism and integrity (INV-6, VF-1, C-3b).** One canonical byte form per artifact. The digest
  covers the bytes as stored.
- **Target-neutral (IR-6).** Nothing here anticipates prikk's import-attestation shape. That is prikk's
  to define (ROADMAP: "prikk decides; brygge informs").
- **Light core (RFC 009 D-1).** `brygge-ir` keeps `sha2` as its only dependency; the codec stays
  hand-rolled.
- **Clarity (the owner's philosophy).** Every public type and field has a one-sentence meaning that a
  foreign encoder author cannot misread.

## Decisions

### D-1 — Codec: tagged records with a critical bit (supersedes RFC 003 D-1)

**Record layout.**
- Every struct is encoded as a **record**: `uvarint(n)` followed by `n` fields.
- A **field** is `uvarint(tag) ‖ uvarint(len) ‖ value (len bytes)`.
- The tag's low bit is the **critical bit**: `tag = (id << 1) | critical`.

**Canonical form** (a reader rejects anything else):
- tags strictly ascending, so no duplicate is possible;
- minimal LEB128 everywhere;
- a field's value consumes exactly `len` bytes;
- an **absent** optional value and an **empty** list are **omitted**, never encoded as empty;
- strings are UTF-8 where the schema says *text*;
- map entries in strictly ascending key order;
- the blob store in strictly ascending `BlobId` order with no duplicates;
- no trailing bytes anywhere.

**Unknown fields:**
- an unknown **critical** field makes the reader **refuse** the artifact, naming the record and the tag;
- an unknown **non-critical** field is **skipped** by its length, and counted, and `inspect` reports
  "N newer non-critical fields not understood".

**Enums** are encoded as `uvarint(variant) ‖ record`. An unknown variant is a decode failure for the field
holding it, so any field whose enum may grow is declared critical. `DerivationKind`, `LossClass` and
`FlagKind` are honesty-bearing and are always critical.

**The rule for new fields:** a field is **critical** if a reader that ignored it could misread or
over-trust the artifact. Anything about status, derivation, loss, flags or identity evidence is
critical. It is **non-critical** only if it is purely additional information.

**Security.**
- Reading is bounded: each `len` and count is checked against the remaining input before allocation.
- Unknown fields are skipped, never parsed.
- Nesting depth is fixed by the schema; no attacker-controlled recursion.

### D-2 — Artifact container, version gate first, digest over the stored bytes (supersedes RFC 003 D-3/D-6)

**Layout:**
```
magic "BRYGGEIR" (8) · format u8 = 2 · contract version (3 × uvarint) · digest (32)
· metadata (uvarint len ‖ record) · blob store (uvarint count ‖ (id 32 ‖ uvarint len ‖ bytes)*)
```
- **The contract version sits in the fixed header** and is checked **before** anything else is decoded
  (CR-09.2). A `format = 1` artifact (every pre-release build) is refused with: *"this artifact was made
  by a pre-release brygge build; re-decode the source"*.
- **The digest is SHA-256 over the stored bytes** of the version, metadata and blob store sections.
  Because decoding is strict (D-1), the stored bytes are the only encoding of the artifact, so this equals
  the canonical-form digest. Tampering with any byte is detected (C-3b). It remains detectability, not
  authentication (RR-4).
- **`import_time` is removed.** It was the only non-identity field and was never set. The registry of
  non-identity fields (RFC 003 D-4) is now **empty**: everything in an artifact is identity-bearing.

### D-3 — The version label and its rule (**OQ-A, the owner's ruling**)

**Recommended: contract `0.2.0` for the first release.**
- `0.1.0` and `1.0.0` were used by pre-release builds, and are retired to avoid two meanings for one
  number.
- While brygge is v0:
  - **`0.y` changes on any breaking change**, i.e. any new or changed critical field or variant;
  - **`0.y.z` changes when only non-critical fields are added**;
  - a reader accepts any artifact of its own `0.y` (skipping unknown non-critical fields) and refuses
    other `0.y` values with *"artifact contract 0.Y is not supported by this build (0.X)"*.
- **Contract 1.0 is declared with brygge 1.0** (the owner's decision). From then on: major = breaking,
  minor = non-critical additions.

**The alternative (1.0.0 now)** is honest only if every future break bumps the major. It also signals a
stability that the owner's v0 policy does not promise, and that would mislead consumers.

### D-4 — Text claims are bytes with a declared encoding (CR-03, text part)

`MetadataClaims` text becomes `Text { bytes, encoding: Option<text> }`:
- `bytes` is exactly what the source stored;
- `encoding` is what the source *declared*, if anything (Git's `encoding` header). brygge never guesses
  an encoding.
- The API offers `Text::as_utf8() -> Option<&str>`, so consumers of UTF-8 sources are not burdened.

**Applies to:** messages, identity names and identity emails. **Paths and ref names stay UTF-8 text**:
non-UTF-8 ones are refused at decode (owner ruling D-3), since no target can hold them either.

### D-5 — Claim semantics: one source field, one claim (CR-04, CR-06 timezones)

- **Two claim pairs:**
  - `author`/`author_time`: the identity and time the source attributes the change to;
  - `committer`/`commit_time`: set **only** when the source states a distinct recording identity or time
    (Git today).

  A decoder never copies one source field into two claims.
- **Timestamps** become `Time { seconds: i64, offset_minutes: Option<i16> }`, where the offset is the one
  the source stated:
  - Git's `±HHMM`, parsed strictly (malformed → `None`, recorded);
  - hg's stored offset;
  - `0` for SVN (`svn:date` is UTC) and RCS (dates are UTC by definition).
- **CVS's representative changeset date** is a derivation, not a source claim. It is recorded in the
  atom's `Derivation.params` (`date_rule`).

### D-6 — Copies, with their source point (replaces `RenameHint`; supersedes RFC 001 D-3's type)

```
CopyRecord { from: Path, from_atom: AtomId, to: Path, status: EpistemicStatus }
```
- **Meaning:** `to` was created as a copy of `from` **as it was at atom `from_atom`**. This carries SVN
  `copyfrom-rev` and hg `copyrev`, both of which are lost today. For hg, `from_atom` is the nearest atom
  holding that file revision: the parent whose manifest has it, so an `hg mv` is a move, and otherwise the
  changeset that introduced it. *(Wording clarified 2026-09-23 while writing the hg handoff; the decision
  is unchanged.)*
- **A copy is a *move*** exactly when `from_atom` is the atom's first parent **and** the atom deletes
  `from`. This is derived, not stored: one fact, one place. `ChangeAtom::is_move(&CopyRecord)` gives
  consumers the answer, and `verify` checks that `from` exists in `from_atom`'s tree.
- **Name.** "Rename hint" misled readers into treating every copy as a rename. The literal ops stay
  beside it, unchanged (RFC 001 D-3's principle holds).

### D-7 — SVN's replacement is a stated op

`PathOp::Replace { path, blob, mode, status }` states that the source recorded a **new node** at a path
that existed (SVN `replace`). The IR carries identity *evidence*, and this is evidence. Replay requires
the path to be present. Git and hg never emit it.

### D-8 — Flags replace magic strings; the dead "refused" section goes (CR-09.3)

- **New IR list:** `flags: Vec<Flag { kind: FlagKind, what: text, count: u64, reason: text }>`, with
  `FlagKind ∈ { ConventionViolation, BelowConfidenceFloor }` (critical, grows by RFC).
  - SVN's unresolved layout and CVS's under-floor changesets become flags.
  - The CLI derives exit `30` from `flags`, never from matching strings.
  - `LossBoundary` stays for drops only.
- **`FidelityReport.refused` is removed.** A refusal (FA-3) is always whole-import by policy and produces
  **no artifact**, so an artifact can never contain one. The field could only ever be empty.
- **Vocabulary:** `refused` names the no-artifact outcome (exit 20), and **`flagged`** names a recorded
  convention or confidence problem in an artifact that was produced. brygge-02 CL vocabulary gains
  *flagged*; the architect updates it with this RFC's acceptance.

### D-9 — Source extras: carried, never dropped (CR-06)

- **`SourceIdentity` gains `extras: Vec<Extra { label: text, bytes }>`**: source-stated data with no IR
  semantics, carried opaquely. Examples: Git's extra commit headers other than `encoding` (which lives in
  `Text.encoding`, D-4, in one place only); `mergetag` (a signed, embedded tag object, so PR-4 material); hg's
  changeset extras beyond `branch` (e.g. `close=1`).
- **Signatures become `Signature { label: text, bytes }`** (`gpgsig`, `gpgsig-sha256`, …), so a consumer
  knows which scheme it holds.
- **`RefRecord` gains `annotation: Option<Annotation { tagger: Identity, time: Time, message: Text }>`**
  for annotated tags (RFC 004 OQ-B's deferred slot).
- **Effect:** the matching drop records disappear, and imports that were "recorded loss" only because of
  annotated tags become clean.

### D-10 — Every public item has a stated meaning, and the derivation registry is contractual

- Each public type and field in `brygge-ir` gets a one-sentence rustdoc stating its meaning (the unit, and
  what `None` means), written for a foreign encoder author.
- Each `DerivationKind` documents the params it **requires**, and `verify` enforces them. This is part of
  the contract:
  - `InferredRename`: `rename_algorithm`, `rename_threshold`;
  - `ReconstructedChangeset`: `window_secs`, `cluster_keys`, `date_rule`;
  - `ReconstructedBranch`: `layout` or `source`.
- `confidence` is `0..=100`; out-of-range values are rejected on decode.

## Open question — resolved

- **OQ-A — The released contract label — RULED 2026-09-23: `0.2.0`** under the 0.y rule of D-3 (recommended over the
  alternative: `1.0.0` with a bump to the major on every break).

The critical-bit mechanism (D-1) is **not** open: the owner approved it as part of ruling D-1 (a) on
2026-09-23.

## Consequences and supersessions

- **RFC 001:**
  - D-3's `RenameHint` → `CopyRecord` (D-6 here); the principle (literal ops plus a marked record) is
    unchanged;
  - D-1 gains `flags` (D-8).
- **RFC 002:**
  - D-3's report sections lose `refused` and gain `flagged`;
  - D-2's loss taxonomy is unchanged.
- **RFC 003:**
  - D-1 (positional codec) → D-1 here;
  - D-3/D-6 (digest recomputed from the model) → D-2 here;
  - D-4's registry → empty;
  - **D-7 (the freeze at 1.0.0 on 2026-09-08) is recorded as a pre-release freeze, superseded by this RFC
    before any release.** Its history is kept, not rewritten.
- **The artifact format byte → 2.** Every pre-release artifact is refused with the D-2 message. No
  migration: nothing was released.
- **Decoders** produce the new records. Each decoder's correction handoff carries its part:
  - Git: text bytes, timezones, extras, annotations, signature labels;
  - hg: text bytes, timezones, extras, copies with `from_atom`;
  - SVN: `Replace`, copies with `from_atom`, flags;
  - CVS: flags, `date_rule`.
- **`inspect` and `verify`** read the new records; machine formats bump their versions.

## Acceptance and verification

**Implementation order:** a `brygge-ir` handoff first (codec, model, artifact, honesty, and the
round-trip and strictness tests), then the decoder handoffs.

**Acceptance tests (in the `brygge-ir` handoff):**
- **Round-trip:** byte-identical for every record type.
- **Strictness:** each non-canonical form is rejected, one test each:
  - an overlong varint;
  - unsorted or duplicate tags;
  - unsorted or duplicate map keys and blobs;
  - an empty optional that is encoded instead of omitted;
  - trailing bytes;
  - a length that exceeds the input.
- **Unknown fields:** an injected unknown critical field → refused and named; an injected unknown
  non-critical field → skipped and counted.
- **Version gate:** a `format = 1` artifact → the pre-release message; a different `0.y` → refused before
  any metadata is decoded.
- **Digest:** any flipped byte, in any section → `DigestMismatch`.
- **Isolation:** `sha2` remains the only dependency (the CI isolation gate from the project-hygiene
  handoff).

**Security review:** this is a new untrusted-input parser for artifacts received from third parties
(TB-5), so the architect reviews it against `brygge-03`.

# Handoff — Git decoder corrections, batch 2: verified objects, encodings, timezones, extras, tag annotations

**Governing RFC:** RFC 004 (Git decoder), Accepted; this handoff inherits that state. It builds on the IR
contract 0.2.0 types (RFC 011, handoff 5).

**Corrections carried** (numbered in the architect's intake review, and in reviews 004/007; this handoff
defines them durably):

| ID | What |
|---|---|
| **Object ids unverified** (review 004 §6.2) | gix does not re-hash an object against its id on read, so a crafted repository can present content under a SHA it does not hash to, and brygge preserves that SHA as the source's identifier (PR-4) |
| Determinism of "count unavailable" (review 004 §6.1) | The raw gix error text lands in an identity-bearing drop record |
| CR-06 (Git) | Timezone offsets, the `encoding` header, `mergetag` and other commit headers are dropped; annotated tags lose their tagger and message |
| RFC 011 D-4/D-9 fills | Git's `Text.encoding`, `Time.offset_minutes`, `Extra`s, `Signature` labels (`gpgsig-sha256`) and `Annotation` |

**Order:** after handoff 5 is approved. It is independent of the CVS, hg and SVN handoffs.

---

## 1. Change scope (`crates/brygge-decode-git/`)

### 1.1 Verify every object against its id (architect decision)

- **Every object brygge reads** (commit, tree, blob, annotated tag) is re-hashed with
  `gix::objs::compute_hash(<repository hash kind>, <object kind>, <data>)`, which is gix-object's
  `compute_hash`, already in the tree, so no new dependency. The result is compared with the id it was
  requested by.
- **A mismatch** is `Error::Read("object <hex> does not match its content (corrupt or crafted
  repository)")`. The decode stops: a source whose object ids lie cannot be carried faithfully.
- **Consequences:**
  - the preserved SHA becomes a real link to the content (PR-4, VF-2);
  - it closes `RR-git-loose-object-symlink`. A symlinked loose object now either hashes to its id (so it
    is the right object) or is refused.
- **SHA-256 object format:** if the repository uses it (`extensions.objectFormat = sha256`), investigate
  whether gix 0.87 reads **and** hashes it correctly. If it does, verify with the SHA-256 kind. If it does
  not, refuse: `feature = "SHA-256 object format"`, added to `floor.rs`. Report the finding with gix
  file:line citations.

### 1.2 Deterministic "count unavailable" (review 004 §6.1)

- The record reads `commits reachable only from dropped refs (count unavailable)`, with a **fixed**
  reason: *"a commit in dropped-only history could not be read"*.
- The raw error goes to stderr only, neutralized, never into the artifact.

### 1.3 Text and its declared encoding (RFC 011 D-4)

- The commit's `encoding` header, when present, becomes `Text.encoding` **on the message only**, verbatim
  as declared. Git's header declares the message's encoding. Names and emails keep `encoding: None`,
  meaning "not stated".
- The `encoding` header itself is not also carried as an `Extra`: one fact, one place.

### 1.4 Timezones (RFC 011 D-5)

- **Parsing:** read the offset token that follows the seconds, strictly. It must be a sign followed by
  exactly four digits: `+HHMM` or `-HHMM`.
- **The value:** `offset_minutes = sign × (HH × 60 + MM)`. It must fit `i16` and have `MM < 60`.
- **Otherwise** it is `None`, counted as `unparseable timezone offsets (N)`, class `Other`. Never use
  gix's defaulting parser: it silently yields `+0000`.

### 1.5 Signatures and extras (RFC 011 D-9)

- **Signatures:** `gpgsig` → `Signature { label: "gpgsig" }`, as today, and `gpgsig-sha256` →
  `Signature { label: "gpgsig-sha256" }`, in header order.
- **Extras:** every **other** commit header except `tree`, `parent`, `author`, `committer`, `encoding`,
  `gpgsig` and `gpgsig-sha256` becomes an `Extra { label: <header name>, bytes: <value> }`, in header
  order.
  - That includes **`mergetag`**, a signed, embedded tag object and so PR-4 material, and any unknown
    header.
  - A multi-line header value is carried exactly as Git stores it, continuation lines joined with `\n`
    and the leading space removed, which is Git's own unfolding.

### 1.6 Annotated tags (RFC 011 D-9)

- An annotated tag's `RefRecord` gains `annotation: Annotation { tagger, time, message }`:
  - `tagger` and `message` as `Text` bytes;
  - `time` with its offset, parsed strictly as in §1.4.
- The tag's own signature stays in the ref's `source.signatures`, labelled `gpgsig` (the embedded PGP
  block, as gix splits it).
- **Remove** the `annotated tag tagger and message (N tag(s))` drop record: the data is now carried. A
  repository whose only recorded loss was annotated tags now exits `0`.

## 2. Non-change scope

- The IR contract, ceilings, history scope, repository-shape refusals, and rename inference.

## 3. Required tests

1. **Object verification:**
   - a loose object whose file content is replaced by another valid zlib object of the same kind (write
     it with `git hash-object`, then rename the file to the victim's path) → refused, naming the id;
   - a normal repository decodes, with every object verified;
   - the pack-independence and determinism tests still hold.
2. **SHA-256:** the investigation result, and a fixture (`git init --object-format=sha256`, if the
   installed git supports it) that either decodes verified or is refused with the named feature.
3. **Count unavailable:** the corrupt-stash fixture from review 004 gives the fixed reason, and decoding
   it twice gives identical bytes.
4. **Encoding:** a commit written with `encoding ISO-8859-1` and a Latin-1 message gives a byte-exact
   message and `encoding == Some("ISO-8859-1")`; the author name's encoding is `None`.
5. **Timezone:**
   - `+0900` → 540;
   - `-0130` → -90;
   - a malformed `+09` or `+0960` → `None`, counted.
6. **Extras:**
   - a merge commit with a `mergetag` header carries `Extra { label: "mergetag" }` with the embedded tag
     bytes;
   - a `gpgsig-sha256` commit carries both signature labels as present.
7. **Annotations:**
   - an annotated tag carries its tagger, time and message;
   - the old drop record is gone;
   - an annotated-tag-only repository exits `0`.

## 4. Security gate

Object verification is an integrity control (T-3, C-3b at the source end), so the architect reviews this
against `brygge-03`. At the 0.1.0 revision, `RR-gix-sha1`'s residual shrinks: a SHA-1 collision is still
possible, but a *mismatched* object no longer is. `RR-git-loose-object-symlink` closes.

## 5. Review request

File `.git-exclude/review-request/011-git-corrections-batch2.md` with the standard sections and the
SHA-256 investigation result.

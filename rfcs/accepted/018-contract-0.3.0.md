# RFC 018 — Contract 0.3.0 (0.7.0): compact artifacts, and transforms that record themselves

**Status.** **Accepted (2026-10-29)** by the owner: "Your recommendations are accepted. Besides, no actual user in
production yet." OQ-1…OQ-3 are ruled as recommended. Handoffs: `rfcs/handoffs/018-contract-0.3.0/`. Proposed
(2026-10-29) by the architect. It is the 0.7.0 contract RFC that RFC 015 OQ-1 and RFC 017 OQ-3 scheduled. Its
trigger, the 0.5.0 release, fired on 2026-10-29. It **widens RFC 017 D-4** (Part C) into an implementable design,
and that RFC's Part C is done when this one is.
**Two refinements at acceptance (the architect, 2026-10-29):** D-7's `--against-source` on a 0.2.0 artifact checks
it, instead of answering `not-checked`; and D-2's bulk-read API is
`brygge_ir::artifact::store_order(&Ir)`, not `ContentStore::iter`. The order is defined by the metadata, so one
function serves the writer, the reader's check and encoders alike, for a store read from an artifact and for one
still being built.
**Two contract refinements at the batch P handoff (the architect, 2026-10-30; 0.3.0 unreleased):**
- `parents` joins the atom vocabulary of `changed`, because removing an atom rewires its children;
- a `Transform` drop's `what` has a grammar the reader checks (`atom <id>`, `op <id> <path>`, `copy <id> <to>`,
  `ref <kind> <name>`), so every loss a transform records names exactly one thing.

D-6's output check becomes a public function, `brygge_ir::transform::check`, usable on any two artifacts.
`transform::Edit` builds a transformed artifact that passes it by construction.
**prikk's view** is asked by letter (brygge → prikk 004). §"prikk" says how a reply is folded in.

## Summary

- **The contract changes once, for two reasons:**
  - **artifacts are too large.** An artifact stores every unique file version uncompressed. fish-shell's is
    4.86 GB. The measurement below says a compact artifact is about **7× smaller**, read with pure Rust;
  - **transforms** (IR → IR: an author mapping, a path filter, pseudonymisation) **must record themselves,** so
    a transformed artifact can never pass for a decoder's original (RFC 017 D-4).
- **The artifact's identity becomes its logical content, not its file bytes.** The digest is taken over the
  uncompressed canonical stream. Two compressions of one artifact are the same artifact, with the same digest.
  A transform chain links to that digest.
- **What does not change:**
  - the IR model's meaning;
  - every decoder;
  - `AtomId` and `BlobId`;
  - the one-pass streaming reader and its checks (RFC 015);
  - the rule that brygge supports no third-party encoder, and now no third-party transform either.

## The measurement (2026-10-29, the architect)

- **The method:**
  - eza at the corpus pin (`98442ab`): 3,372 commits; a 79.9 MB artifact holding 7,171 blobs, 76.7 MB raw;
  - also brygge's own repository: 18 MB;
  - the blobs were extracted from the artifacts, and compressed in different groupings and with different codecs.

  The scripts and the full table are in the architect's record (review 098).

| Grouping of the blobs (eza) | Pure-Rust DEFLATE (`miniz_oxide` 0.9.1, level 6) | Pure-Rust zstd (`ruzstd` 0.9.1, its only level, "Fastest") | Native zstd -1 (for reference only) |
|---|---|---|---|
| each blob alone | 22.5 MB (3.4×) | 29.5 MB (2.6×) | — |
| 1 MiB frames, in first-use order | 18.7 MB (4.1×) | 22.8 MB (3.4×) | 11.1 MB |
| **1 MiB frames, grouped by path, then by first use** | **10.3 MB (7.5×)** | 10.2 MB (7.5×) | 5.2 MB (14.8×) |
| 256 KiB frames, grouped by path | 10.8 MB (7.1×) | 10.5 MB (7.3×) | — |
| the whole store as one stream (no random access) | — | — | 3.5 MB (`-19 --long`; 22×) |

- **The lesson:**
  - successive versions of a file are near-duplicates, so **putting them next to each other matters more than
    the codec;**
  - in pure Rust, DEFLATE and zstd compress equally well today: `ruzstd`'s encoder implements only one level, and
    a small window;
  - the metadata compresses about 2.6× (3.0 MB → 1.2 MB).
- **The whole artifact,** with the metadata, comes to about 11.5 MB from 79.9 MB.
- **For fish-shell,** that suggests about 0.7 GB. That is a projection; batch C measures it.
- **Batch C's measurement (review 099):**

  | | 0.2.0 | 0.3.0 | Ratio |
  |---|---|---|---|
  | eza | 79.9 MB | 10.7 MB | 7.45× |
  | TortoiseHg | 1.76 GB | 336 MB | 5.23× |
  | fish-shell | 4.91 GB | 1.06 GB | 4.64× |

  - **The cause of the spread is DEFLATE's 32 KiB window.** Grouping helps a file only when its earlier version
    is within 32 KiB.
  - In fish-shell, 60% of the content is in blobs over 1 MiB, and 19% in blobs of 64–256 KiB.
  - **Recorded as a later contract candidate:** a larger-window codec, or deltas between versions of a path.
- **DEFLATE also costs less time:**
  - decompressing all of eza's frames took 0.05 s;
  - compressing them took 0.55 s, against a decode that takes seconds.

## Decisions

### D-1 — The container (contract 0.3.0)

```
"BRYGGEIR" (8) · format: u8 = 2 · version: uvarint × 3 (0, 3, 0) · digest: 32
· metadata: frames
· blob table: uvarint(count) ‖ ( id32 ‖ uvarint(raw_len) )*      in store order (D-2)
· blob data:  frames                                               the blobs' bytes, concatenated in store order
frames := uvarint(count) ‖ ( uvarint(codec) ‖ uvarint(raw_len) ‖ uvarint(stored_len) ‖ bytes )*
codec  := 0 stored (bytes as they are) | 1 DEFLATE (RFC 1951, raw, no zlib header)
```

- **The format byte stays `2`.** The layout after the version is the contract version's. So brygge ≤ 0.6.0, given
  a 0.3.0 artifact, says "artifact contract 0.3 is not supported by this build (0.2)", and never "unknown
  artifact format 3".
- **The frames, fixed by the contract, so that the frame boundaries are canonical:**
  - **packing:** blobs fill a frame in store order up to **1 MiB** raw. A blob larger than that is a frame of its own;
    a blob never spans two frames;
  - **checking:** the reader recomputes the frame boundaries from the blob table, and refuses any other packing;
  - **the metadata** is cut into 1 MiB raw frames.
- **The codec's choice per frame is not canonical.**
  - The writer stores a frame raw (`codec 0`) when DEFLATE would not make it smaller.
  - The reader accepts either codec for any frame, because both give the same bytes, and the digest is over those
    bytes (D-3).
- **Bounds, before anything is allocated:**
  - every frame's `raw_len` is checked against the blob table and against the reader's existing size limits,
    before decompression;
  - the decompressor writes into exactly `raw_len` bytes. A stream that is longer, shorter, or does not end exactly
    is refused (the lesson of RUSTSEC-2026-0344);
  - a large single-blob frame streams through the hash, and is never held whole.
- **The level** (6) is the writer's choice, recorded in the format page as informative. It is not a rule a reader
  checks.

### D-2 — The store order: grouped by path, then by first use

- **A blob's first use** is the first op, in canonical atom order and then op path order, that names it (`Add`,
  `Modify` or `Replace`). Every blob has exactly one: no orphans, and at most one op per path per atom.
- **The store order** sorts blobs by `(the path of the first use, as bytes; the index of its atom)`. That is total and
  deterministic, and the reader recomputes it from the metadata and refuses any other order, as it does for the
  atom order today.
- **For readers:**
  - `ContentStore::get` stays random access, and costs at most one frame's decompression. The reader keeps a
    small, bounded cache of frames;
  - a new **`artifact::store_order(&Ir)`** gives the blob ids in store order. Reading them in that order
    decompresses each frame once;
  - the encoder guide recommends it for bulk work: `examples/encode-git` writes all its blobs first, in store order.

### D-3 — The digest is over the logical content

- **The digest** is SHA-256 over `version ‖ metadata ‖ blob table ‖ blob data`, **as uncompressed**: exactly the
  bytes the frames give, in order, and never a re-encoding.
- **What that gives:**
  - two files that differ only in compression are one artifact, with one digest;
  - **the digest stays reproducible across codec versions.**
- **The digest is a function of the IR alone** (clarified 2026-10-29, after prikk's letter 005):
  - the store order and the frame packing are fixed by the contract, and enforced by the reader, so two
    conforming writers cannot order or group the same IR differently;
  - the only freedom a writer has is each frame's codec, and its DEFLATE bytes, and the digest covers neither.
  - **The IR includes its provenance** (the brygge and decoder versions, and the parameters). So two decodes of
    one repository by different brygge versions are two IRs, with two digests. The digest names an IR: a history
    as decoded. `verify --against-source` is what ties it to the source.
- **The reader still verifies the digest in its one pass,** before it accepts anything (RFC 015 D-4), and
  `ArtifactChanged` keeps its meaning.
- **`Decoded::digest`** (0.6.0) keeps its name, with this meaning. The Git example's `Brygge-Artifact:` trailer
  and the transform chain (D-5) both cite it.
- **Reproducibility (VF-1), restated:**
  - the **digest** is the same for the same brygge version, source and parameters, whatever the codec's version;
  - the **file bytes** are the same for the same brygge build. Release binaries are built `--locked`.
- **The codec is not pinned with `=`** in `brygge-ir`. That pin would force one `miniz_oxide` on every crate that
  links `brygge-ir` (prikk among them). Instead:
  - a golden-bytes test shows when an upgrade changes the output;
  - the CHANGELOG says so when it does.

### D-4 — The codec: DEFLATE, through `miniz_oxide` (OQ-1)

- **Why DEFLATE:**
  - it is frozen (RFC 1951, 1996), and every language has a decoder, so foreign readers (PU-3) need nothing
    unusual;
  - `miniz_oxide` is pure Rust, forbids `unsafe`, and is very widely used. It is already in brygge's lock file,
    through `flate2` in `brygge-decode-hg`.
- **Its cost:** `brygge-ir` gains its first dependency after `sha2`: `miniz_oxide` and `adler2`. The isolation
  allowlist and RFC 009's light-core statement are updated, and the owner approves it (RFC 009 D-6).
- **zstd** would be smaller with a better encoder (native zstd -1 reaches 14.8×), but not with the pure-Rust one
  there is today. Adding codec `2` later is a contract change, and the frame layout leaves room for it.

### D-5 — Transforms: the chain, and the `Transformed` status

- **The chain:** `Ir` gains field 6, **`transforms: list<TransformRecord>`**, oldest first. It is omitted when
  empty, so a decoder's artifact never carries it. Each record:
  - `name` (text);
  - `version` (text);
  - `params` (a map);
  - **`consumed`** (id32): the digest (D-3) of the artifact it read.

  The first record's `consumed` is the decoder's original.
  - **Each record's output digest is not stored, because it is implied:** record *k*'s output is record *k + 1*'s
    `consumed`, and the last record's output is the artifact's own digest. A stored copy could only disagree with
    those (clarified 2026-10-29, prikk's letter 005).
  - **The losses can be enumerated from the artifact alone:** every `Transform` drop names its step, and the
    drops without a step are the decoder's own.
- **A new `EpistemicStatus` variant 2, `Transformed { step: uvarint, changed: list<text>, prior: EpistemicStatus }`,**
  on an atom, an op or a ref that a transform changed (ops added at the batch T handoff, so that replaced content
  never passes for stated):
  - `step` indexes the chain;
  - `changed` names the parts it changed, from a closed vocabulary (for example `author`, `message`, `ops`;
    `blob`, `path`; `name`, `target`);
  - **`prior` keeps the status it had before,** so an earlier `Derived` is never erased.

  This refines RFC 017 D-4's "a `Transformed` derivation kind". A derivation kind would replace the prior
  status, which is the laundering this design exists to stop.
- **A removal is a loss.** `LossClass` gains variant 3, `Transform`, and `DropRecord` gains `step` (uvarint),
  required for that class and absent otherwise.
- **What a transform may do:**
  - change atoms, ops, blobs and refs, and remove them;
  - **it may not add an atom**: that would be inventing history;
  - **it may not change `provenance`**, which stays the decoder's.
- **`verify`:**
  - **internally, the chain is well-formed:**
    - every `step` names a record;
    - every `prior` is well-formed;
    - every `Transform` drop names a record;
    - an artifact without a chain carries no `Transformed` status and no `Transform` drop;
  - **`--against-source` on a transformed artifact** re-decodes, and compares the re-decode's digest with the
    **first record's `consumed`**. The verdict is "**the chain's root corresponds** to the source; *N*
    transforms after it were not re-run". It never prints plain "corresponds". Replaying transforms is out of
    scope.
- **`inspect`** prints the chain, and the counts of what each step changed and dropped.
- **Encoders carry it:**
  - the guide gains the rule;
  - `examples/encode-git` adds `Brygge-Transformed: <step> <name> <version> (<changed>)` trailers, and lists the
    chain on stderr.

### D-6 — `brygge transform`: the transform boundary, checked by brygge (OQ-3)

- **The command:** **`brygge transform <name> <artifact> --out <file> [-- <plugin arguments>…]`** runs
  `brygge-transform-<name> --artifact <artifact> --out <temporary file> [<plugin arguments>…]`. The plugin is
  found by **exactly `encode`'s rules**, shared in code: `PATH` only, `.exe` only on Windows, the name grammar, no
  shell. The temporary file is in `--out`'s directory.
- **Before the plugin runs:**
  - verify first, with verify's own exit code;
  - the same banner as `encode`: "not part of brygge".
- **After it exits 0, brygge checks the output against the input. This is where the honesty is enforced:**
  1. the output passes `verify`'s internal checks;
  2. its chain is the input's chain plus **exactly one** record, named `<name>`, whose `consumed` is the
     input's digest;
  3. `provenance` is unchanged;
  4. **every difference is recorded:**
     - every output atom and ref is matched to its input by source identity;
     - one that differs is `Transformed` at the new step, with `prior` equal to its input status, and its
       `changed` names every part that differs;
     - one that is not marked is identical, except for parent ids rewritten for changed ancestors;
     - every input atom, op or ref that is gone has a `Transform` drop at the new step;
     - no atom is new.
- **The outcome:**
  - **if every check passes,** the file is renamed to `--out`;
  - **if any check fails,** the file is deleted, nothing is written to `--out`, and brygge exits **50** with the
    first difference it found named. A plugin's own non-zero exit code passes through, as for `encode`.
- **The residual, stated:** an artifact written with `brygge-ir`'s writer outside this command is not checked
  at transform time. A forged artifact that hides its chain still fails `--against-source`, because its
  digest is not the re-decode's.
- **An unsupported example,** `examples/transform-authors` (a mailmap-style author mapping, `publish = false`),
  is the test plugin. A guide, `writing-a-transform.md`, gives the rules above.

### D-7 — Artifacts of contract 0.2.0 (OQ-2)

- **brygge 0.7.0 reads 0.2.0 artifacts, and writes only 0.3.0.** `inspect`, internal `verify`, `encode` and
  `transform` accept them:
  - the metadata records are a subset of 0.3.0's;
  - only the blob store's layout and the digest's definition differ. In 0.2.0, stored bytes are the raw bytes.
- **`Decoded` gains `contract`** (the version the artifact declares).
- **`verify --against-source` on a 0.2.0 artifact works as on a 0.3.0 one.** It compares the re-decoded IR with
  the artifact's IR, never their bytes or digests, and the IR model is the same in both contracts. (Corrected at
  acceptance: the proposal said `not-checked`, from a wrong reading of how the comparison works.)
- **The "0.y" rule** (`ir-artifact-format.md` §4) is amended: a reader accepts its own `0.y` and the earlier `0.y`
  versions it lists, and nothing else.

## Batches, and the order

| Batch | Scope | Branch |
|---|---|---|
| **C** | D-1…D-4 and D-7: the container, the store order, the digest, the codec, `store_order`, reading 0.2.0, the format page rewritten for 0.3.0, the golden-bytes test. The corpus measured: artifact size, decode time and peak memory, for eza, fish-shell and TortoiseHg, against 0.6.0 | `main` |
| **T** | D-5: the chain, `Transformed`, the `Transform` loss, `verify` and `inspect`, the guide's and the Git example's carrying rules | `main` |
| **P** | D-6: `brygge transform`, the shared plugin lookup, the output check, `examples/transform-authors`, `writing-a-transform.md` | a topic branch (Windows and macOS logs, as for batch E) |
| **cut** | threat model v0.11; RFC 017 and RFC 018 to `done/`; `CHANGELOG` | — |

- **Batch C comes first,** because T and P build on its digest.
- Contract 0.3.0 is unreleased until the cut, so T may add critical fields under it.
- The deep mutation sweep runs on C and T: both change the untrusted parser.

## The threat model (v0.11, written at the cut)

- **C-3b:** decompression is bounded before allocation, and the reader writes into exactly the declared size.
- **The digest's new definition,** and that it no longer covers the codec's choices.
- **TB-7, the transform boundary,** and its output check.
- **RR-transform-trust:** like RR-plugin-trust, and the out-of-command residual of D-6.
- **INV-4:** `miniz_oxide` and `adler2` in the core's closure.

## prikk

- **brygge asks prikk, and decides nothing for it:**
  - whether prikk's import needs smaller artifacts, or a particular read pattern;
  - whether D-3's digest is the one prikk would cite;
  - whether prikk would accept transformed artifacts at all;
  - whether `miniz_oxide` in `brygge-ir` troubles its dependency policy.
- **A reply that arrives before the cut** is folded in, by an amendment the owner approves.
- **0.7.0 is not tagged** until prikk has replied, or the owner rules to go ahead without a reply.
- **prikk replied on 2026-10-29** (letter 005):
  - nothing conflicts;
  - the logical digest is the one prikk would cite;
  - prikk has no policy on transforms yet;
  - the dependency is no trouble.

  Its question and its suggestions are answered by the D-3 and D-5 clarifications above, and by D-1's bounds
  (batch C enforces them before inflating). They change no design. A per-frame digest, its other suggestion, is
  not needed: each blob's id is its digest, and `get` re-checks it on every later read (`ArtifactChanged`).

## Open questions (the owner's) — ruled 2026-10-29, as recommended

- **OQ-1 — The codec.**
  - **Recommended: DEFLATE, through `miniz_oxide`** (D-4). It is frozen, universal, pure Rust and widely used,
    and in pure Rust it is as small as zstd today.
  - **Alternative:** zstd, through `ruzstd`, which is already in the tree. It may compress better when its
    encoder matures. Its decoder had a high-severity advisory this month, and only one encoder level exists.
- **OQ-2 — 0.2.0 artifacts.**
  - **Recommended: read them, write 0.3.0** (D-7). Users keep every artifact they have. It costs one small branch
    in the reader.
  - **Alternative:** refuse them, with the message "re-decode with this build". That is lighter, but each
    archived artifact needs its source again.
- **OQ-3 — `brygge transform` in 0.7.0.**
  - **Recommended: yes** (D-6). Without it, a transform is honest only by convention. With it, brygge checks
    every difference at the boundary it controls.
  - **Alternative:** the contract only (D-5) in 0.7.0, and the command later. That is smaller, but the
    contract's rules would then stand unenforced.

## Consequences

- Artifacts are 4.6× to 7.5× smaller on the measured repositories, the most for histories of small files. Their identity no
  longer depends on the codec.
- Transforms become possible, and leave a record a reader can check: back to the decoder's original by digest,
  and back to the source by `--against-source`.
- The contract changes once, as RFC 015 OQ-1 and RFC 017 OQ-3 ruled.

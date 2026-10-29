# Handoff — RFC 018 D-1…D-4 and D-7: compact artifacts, contract 0.3.0 (batch C)

**Governing:** RFC 018, accepted 2026-10-29. OQ-1 (DEFLATE through `miniz_oxide`), OQ-2 (read 0.2.0, write 0.3.0)
and OQ-3 were ruled as recommended. **Target:** 0.7.0. **Batch C comes first:** batch T (transforms) and batch P
(`brygge transform`) build on its digest, and their handoffs follow once it closes. One review request, on
`main` (no process or platform code).

## 0. First steps, before any code change

1. **Commit the architect's files on their own,** as "RFC 018 accepted (contract 0.3.0); its batch C handoff":
   - `rfcs/accepted/018-contract-0.3.0.md` (moved from `proposed/`);
   - `rfcs/handoffs/018-contract-0.3.0/` (this file);
   - `rfcs/accepted/017-extensibility.md` (its status line points at RFC 018);
   - `rfcs/README.md` and `ROADMAP.md`.
2. **Two baselines from the unchanged code (0.6.0's format):**
   - **a 0.2.0 fixture.** Commit a small 0.2.0 artifact, written by today's `to_bytes` from an IR that has every
     record kind (the shape of `mutation.rs`'s `richer_ir`, with at least two blobs, one empty). Put it under
     `crates/brygge-ir/tests/data/contract-0.2.0.ir`, with its IR's builder code beside it in the test. It is
     D-7's proof that old artifacts still read;
   - **the corpus at 0.6.0.** For eza, fish-shell and TortoiseHg: the artifact's size, the decode's wall time,
     and the peak RSS of `brygge decode`. Use the release build, and the same machine and method as §6. Record the
     method in the request.

## 1. The dependency

- **The workspace dependency:**
  `miniz_oxide = { version = "0.9", default-features = false, features = ["with-alloc"] }`,
  - not pinned with `=` (RFC 018 D-3);
  - with a comment: RFC 018 D-4; approved by the owner on 2026-10-29 under RFC 009 D-6; pure Rust,
    `forbid(unsafe_code)`, already in the lock file through `flate2`.
- **In `brygge-ir`'s `[dependencies]`:** it is the second, after `sha2`.
- **`allowed-dependencies.txt`:** add `adler2` and `miniz_oxide`, each with a one-line reason.
  - `tools/check-ir-isolation.sh` (all targets) passes.
  - Report the closure on `x86_64` and `aarch64`.
- **Update the light-core statements** that say `sha2` is the only dependency: the workspace comment,
  `brygge-ir`'s README, and the crate doc. Leave `brygge-03`: the architect writes INV-4 at the cut.

## 2. The container (D-1), exactly

```
magic "BRYGGEIR" (8) · format u8 = 2 · version uvarint×3 = 0,3,0 · digest (32)
· metadata frames
· blob table: uvarint(count) ‖ ( id32 ‖ uvarint(raw_len) )*        in store order (§3)
· blob frames
frames  := uvarint(nframes) ‖ frame*
frame   := uvarint(codec) ‖ uvarint(raw_len) ‖ uvarint(stored_len) ‖ stored bytes
codec   := 0 (stored: the bytes as they are) | 1 (DEFLATE, RFC 1951, raw: no zlib header or checksum)
```

- **The logical stream.** It is what the digest covers, and what the content is defined by:
  `version bytes ‖ uvarint(meta_len) ‖ metadata record ‖ uvarint(count) ‖ (id32 ‖ uvarint(raw_len))* ‖ every
  blob's bytes, in store order`.
  - The digest is SHA-256 of exactly that stream (D-3).
  - Frame headers and stored bytes are not hashed. Their raw output is.
- **The packing rules, which the reader recomputes and enforces:**
  - **metadata:** `meta_len` bytes, cut into frames of exactly 1 MiB (`1 << 20`) raw, the last one shorter. An
    empty metadata record is impossible (`provenance` is required);
  - **blobs:** in store order. A blob of 1 MiB or less goes into the current frame if the frame's raw total
    stays at or under 1 MiB, or else starts a new one. A larger blob is a frame of its own. A blob never spans two frames. An empty
    blob adds nothing, and belongs to whatever frame is open, or to a new one if none is;
  - the frame count and every frame's `raw_len` must match these rules, or the read fails with
    `NonCanonical`.
- **The codec, per frame:**
  - **the writer:** DEFLATE at level 6, and codec 0 instead when DEFLATE is not smaller than the raw bytes;
  - **the reader:** accepts either codec, for any frame.
- **The bounds, all checked before any allocation:**
  - codec 0: `stored_len == raw_len`;
  - codec 1: `raw_len ≤ 1032 × stored_len + 64`, which is DEFLATE's maximum expansion. A frame that claims more
    is refused before it is inflated;
  - a frame that holds small blobs has `raw_len ≤ 1 MiB`;
  - **inflating:**
    - output beyond `raw_len` is refused;
    - so is a stream that ends short, or that leaves stored bytes unconsumed;
    - a single-blob frame is inflated through the hash in chunks, and never held whole during the pass.
  - Any other codec value: `Decode("unknown frame codec N")`.
- **The error precedence,** stated in the format page:
  - a frame that does not inflate is `Decode("<metadata|blob> frame N: …")`, because there is no logical stream
    left to hash;
  - otherwise today's rule holds: `DigestMismatch` before any structural error.
  - `from_bytes` and `read_from` must agree, variant for variant, as the mutation tests already require.

## 3. The store order (D-2)

- **`pub fn store_order(ir: &Ir) -> Vec<BlobId>`** in `brygge_ir::artifact`:
  - each blob's first use is the first `Add`, `Modify` or `Replace` naming it, in canonical atom order and then
    op path order;
  - sort by (that op's path, as bytes; that atom's index);
  - document it as the order that reads each frame once.
- **The writer** uses it, and so do the reader's checks:
  - the blob table's ids must be in exactly this order, or `NonCanonical`;
  - every op's blob is in the table, and every table entry is used (as today).

## 4. The reader and the writer

- **The version gate:**
  - `version::CURRENT` becomes `0.3.0`;
  - add `pub const READABLE: &[ContractVersion]` (0.2 and 0.3, any patch), used by `accepts`;
  - the error message names what is readable: "artifact contract 0.1 is not supported by this build (reads 0.2,
    0.3)".
- **`Decoded` gains `pub contract: ContractVersion`.** For a 0.2.0 artifact:
  - the old layout is read, and its digest is the old definition (stored bytes);
  - the IR is the same model.

  Document both digest definitions on `Decoded::digest`.
- **`ContentStore`'s artifact backing:**
  - the index maps a blob to (its frame, its offset in the frame's raw bytes, its length);
  - the frames are listed by (file offset, codec, `raw_len`, `stored_len`);
  - `get` reads the frame, inflates it (bounded as in §2), and re-hashes the blob. `ArtifactChanged` keeps its
    meaning;
  - a cache keeps the last 8 small-blob frames, so at most 8 MiB. A single-blob frame is never cached;
  - a 0.2.0 artifact's blobs keep today's `(offset, len)` backing.
- **The writer (`write_to`):**
  - the metadata, framed;
  - the blob table, from `store_order`;
  - the blob frames, one frame's raw bytes in memory at a time (at most 1 MiB, or one large blob, as today);
  - the digest at its offset, as today.

  `ContentStore` needs each blob's length without reading it: add a crate-private accessor, from the
  in-memory map or the spill index.
- **Writing a stored-only artifact** (every frame codec 0) is a crate-private test helper. It adds no public API.

## 5. The CLI and the documents

- **`inspect`:**
  - prints the artifact's own `contract` (it prints `CURRENT` today: `commands.rs` near lines 591 and 688);
  - a 0.2.0 artifact gets a note: "contract 0.2.0: an older contract; this build reads it, and writes 0.3.0";
  - the machine key keeps its name, `contract_version`, and its meaning becomes the artifact's own. Say so in
    `machine-output.md`.
- **`verify`:**
  - internal checks on both contracts;
  - `--against-source` on a 0.2.0 artifact compares the IR, as today. Test it on a fixture repository's 0.2.0
    artifact, made with the 0.6.0 code in §0.
- **`encode`:** hands either contract to the plugin, unchanged.
- **`examples/encode-git`:** writes every blob first, in `store_order`, then the commits. The output is
  unchanged: the round-trip test proves it.
- **`ir-artifact-format.md`,** rewritten as "contract 0.3.0":
  - the container, the frames, the packing, the logical stream, the digest, the store order, the bounds and the
    precedence;
  - **a section "Contract 0.2.0 (read only)"** with its layout and digest;
  - an updated worked example (§7). Write it so that a foreign reader can build a reader from it alone (PU-3).
- **`writing-an-encoder.md`:** `store_order` for bulk reads, and the digest's definition per contract.
- **Every other present-tense "contract 0.2.0"** (the list: `introduction.md`, `README.md`, the requirements and
  external-design pages, `HANDOFF.md`, `GOVERNANCE.md`, `commands.rs`'s module doc) becomes 0.3.0. Leave the
  historical mentions, and `brygge-03`.
- **The `--help` of `decode`:** one clause, "artifacts are compressed (contract 0.3.0)", if it fits. Otherwise
  nothing.

## 6. Proof

1. **The round trip:** `to_bytes` → `from_bytes` → equal IR, for every IR the tests already build, through both
   readers.
2. **Digest independence:** the same IR written with DEFLATE frames and stored-only gives two different files, one
   digest, and equal IRs.
3. **Golden bytes:**
   - commit `crates/brygge-ir/tests/data/contract-0.3.0.ir`, written from the §0 IR;
   - a test asserts that `to_bytes` reproduces it byte for byte;
   - its failure message says that a codec or writer change altered the file bytes, and that this must be noted in
     the CHANGELOG, with the file regenerated.
4. **0.2.0:**
   - the §0 fixture reads through both readers;
   - its digest matches the stored one, and `contract == 0.2.0`;
   - `inspect` shows the note, and `verify` passes.
5. **Refusals, each its own test:**
   - wrong packing (two frames that should be one; a blob split across frames);
   - a blob table out of store order;
   - codec 0 with `stored_len != raw_len`;
   - a DEFLATE frame claiming more than 1032× expansion (refused with no allocation: assert on a tiny input
     claiming 1 GiB);
   - an overlong and a short inflate;
   - trailing stored bytes;
   - an unknown codec;
   - a 0.1 and a 0.4 version.
6. **The mutation harness** (`artifact/mutation.rs`), with three strategies:
   - **raw:** as today;
   - **resigned, on the logical stream:** mutate the logical stream, re-frame it stored-only, recompute the
     digest. That reaches the structural checks without the codec in the way;
   - **frames:** mutate the stored bytes of DEFLATE frames, and re-sign nothing. That exercises the inflater's
     bounds.
   - **The "never overclaims" property, redefined:** content never exceeds the declared `raw_len`s, which are
     bounded as in §2, and no single allocation exceeds one frame's declared raw length.
   - Run the deep sweep before the push, under `mutation.yml`'s cap. Record the time and the findings.
7. **The corpus, against §0's baseline:** size, decode wall time and peak RSS, for eza, fish-shell and TortoiseHg.
   Also `verify`'s wall time and peak RSS on each 0.3.0 artifact, and on its 0.2.0 one. **The expectations:**
   - roughly 7× smaller, measured on eza by the architect;
   - decode time within 1.25× of 0.6.0;
   - peak memory not above 0.6.0's.

   A miss on either is a finding to report, with the cause, not a reason to stop.
8. **The usual gates:**
   - fmt; clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
   - `cargo test --workspace` on stable and 1.87;
   - deny; audit (only `RUSTSEC-2026-0306`); the isolation check on every target; the dev-dependency guard;
   - the links, and the book with no warnings.

## 7. CHANGELOG `[Unreleased]`

- **Breaking:** "Artifacts are IR contract 0.3.0: compressed, with blobs grouped by path (about 7× smaller on a
  measured repository). brygge 0.6.0 and earlier cannot read them. The artifact digest is now taken over the
  uncompressed content, so it does not depend on how the artifact was compressed (RFC 018)."
- **Breaking:** "`brygge-ir`: `version::CURRENT` is 0.3.0; `artifact::Decoded` gains `contract`; `brygge-ir`
  depends on `miniz_oxide`."
- **Added:** "`brygge_ir::artifact::store_order`: the blob order that reads each compressed frame once."
- **Changed:** "brygge still reads contract 0.2.0 artifacts (`inspect`, `verify`, `encode`), and writes only
  0.3.0. `inspect` shows the artifact's own contract version."

## 8. Review request

File `.git-exclude/review-requests/061-compact-artifacts.md`, with:
- the corpus table (§0 against §6.7);
- the closures;
- the sweep's time and findings;
- the list of documents changed.

Commit and push to `main` after approval, with CI (all seven jobs), Mutation (deep) and the docs run appended.

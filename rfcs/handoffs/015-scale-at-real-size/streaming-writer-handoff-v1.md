# Handoff — RFC 015 D-2: `finish` without a copy, and a streaming artifact writer (batch W)

**Governing:** RFC 015 D-2, and review 074's attribution. **Crates:** `brygge-ir`, and the CLI's write path. One
review request. **Target:** fish-shell's `brygge decode` peak (P3), from 18.7 GiB to **≤ 1.3× its content (about
6 GiB)**, with **byte-identical artifacts.**

## 1. `IrBuilder::finish` moves blobs; it does not copy them

- Today (`builder.rs:208–211`), `finish` inserts `bytes.to_vec()` of each referenced blob into a **new**
  `ContentStore` while the old one is alive. That is a second full copy of the content, and a second SHA-256 of
  every blob.
- **The fix:** prune the unreferenced blobs **in place** (for example a `ContentStore::retain` over the referenced
  set, `pub(crate)`). Each blob is hashed once, when it is added, as now.
- Nothing observable changes: the same blobs, the same ids, the same order.

## 2. A streaming writer

- **Add `artifact::write_to<W: Write + Seek>(ir: &Ir, out: &mut W) -> Result<…>`.** It writes:
  1. the magic, the format byte, and the version varints;
  2. **a 32-byte placeholder** at the digest's fixed offset (`0x0c`);
  3. the body (the metadata length and bytes, the blob count, then each blob's id, length and bytes in id
     order), **streamed**, while feeding a SHA-256 over `version ‖ body`, exactly what the format digests (§3,
     "excluding the 32 digest bytes themselves");
  4. then it seeks to the digest's offset, writes the digest, and seeks back to the end.
- **One encoder:** `to_bytes(ir)` becomes `write_to` into a `Cursor<Vec<u8>>`, so the two can never diverge.
- **The metadata** may still be encoded into one buffer first (`encode_metadata`): it is small (15 MiB for
  fish-shell). Only the blob section must stream. Write each blob straight from the store's own bytes, with no
  intermediate copy.
- **Errors:** an I/O error is a typed error, never a panic.

## 3. The CLI writes through it

- `decode`'s write path keeps C-7 exactly: a temporary file in the output's directory, renamed only when complete.
  But it writes **through `write_to`**, with a `BufWriter<File>` (seeking flushes it), instead of
  `to_bytes` plus a whole-buffer write.
- On any error, the temporary file is removed, as today.
- Check whether `verify --against-source` builds its re-decode's artifact through `to_bytes`. If it only compares
  `Ir` values, leave it for batch V, and say which.

## 4. Proof

1. **Byte-identity, everywhere:** every fixture, the bench corpora, and the **real corpus** (all of
   `tools/corpus/real.toml`). `cmp` the artifacts from `main` and from this tree.
2. **A property test:** for generated `Ir`s (several atoms, refs, drops, flags, empty and large blobs), `to_bytes`
   equals `write_to` into a `Vec`, and `from_bytes` accepts it.
3. **The A/B** with `brygge-bench profile`: P1, P2 and P3 before and after, for fish-shell, TortoiseHg and the
   synthetic targets. **Expected:** P1 falls by about one content-size (the `finish` copy), and P3 ≈ P1 (no
   serialization buffers).
4. **C-7:** an interrupted write leaves no artifact, or the previous one intact (the existing tests), and a write
   error removes the temporary file.
5. **The usual gates** (stable and 1.87, clippy on three targets, deny, audit, the isolation check, the dev-deps
   guard, the publish dry-run), and the mutation tests at both depths. `Mutation (deep)` runs, because
   `brygge-ir` changes.

## 5. CHANGELOG `[Unreleased]`

- **Added:** "`brygge-ir`: `artifact::write_to`, a streaming artifact writer; `to_bytes` is built on it."
- **Changed:** "`brygge decode` no longer holds extra copies of the content while finishing and writing an
  artifact: the peak falls from about 4× the content to about 1× (fish-shell: 18.7 → about N GiB). Artifacts are
  byte-identical."
- Put in the measured numbers.

## 6. Review request

`.git-exclude/review-requests/050-streaming-writer.md`. Commit with the batch the architect's corrections now in the tree: `docs/src/brygge-03-threat-model-v0.1.md` (RR-total-memory, and v0.8's summary: about 4× content, 18.7 GiB) and `rfcs/done/015-scale-at-real-size.md`, plus this handoff. A topic branch is not required (nothing is CI-only);
commit and push to `main` after approval. Then batch S.

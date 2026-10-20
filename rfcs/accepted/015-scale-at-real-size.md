# RFC 015 — Scale at real size (0.5.0): memory that does not grow with a history's total content

**Status.** **Accepted (2026-10-20)** by the owner, with two additions (recorded in OQ-1 and OQ-2). Handoffs:
`rfcs/handoffs/015-scale-at-real-size/`. Proposed (2026-10-20) by the architect.

## Summary

- RFC 014's real-world corpus measured what synthetic scenarios could not: **fish-shell** (23,496 commits)
  decodes at a **9.6 GiB** peak, and **TortoiseHg** at 3.5 GiB. That is about 2× their IR content (4.6 GiB and
  1.7 GiB): every unique file version, uncompressed.
- RFC 010 deferred streaming "until a real import is memory-bound". That trigger has fired: an ordinary 8–16
  GiB machine cannot decode a mid-size real project. The threat model records it as **RR-total-memory**.
- **The goal:** peak memory that tracks the history's *metadata*, not its total content. The **artifact format
  does not change,** so every artifact stays byte-identical.

## What the code shows (the architect's reading; batch A measures it)

1. **Decoding holds every blob in memory** (`ContentStore`: `BTreeMap<BlobId, Vec<u8>>`) until the end.
2. **`artifact::to_bytes` builds the whole artifact in memory, twice:** a `body` buffer, then a copy of it into
   `out`, while the `Ir` (and so every blob) is still alive. Only then does the CLI write the file.
3. **Reading (`verify`, `inspect`, `--against-source`) loads the whole file** (`from_bytes(&[u8])`) and decodes
   every blob into memory again.

**The format permits streaming on both sides:**
- the digest is written before the body, but it covers only `version ‖ body`;
- a writer can stream the body into the output file while hashing it, then write the 32 digest bytes at their
  fixed offset (`0x0c`). The artifact is already written to a temporary file and renamed (C-7), so it can seek;
- a reader can hash the file in one pass, then read the metadata (small), and each blob by its offset.

## Constraints

- **Byte-identical artifacts** for every input, before and after: the format and the contract (0.2.0) are
  unchanged.
- **Measure first** (RFC 010's method): batch A attributes the peak before any change, and every batch reports
  an A/B on the real corpus and the synthetic scenarios.
- **C-7 holds:** the write is atomic (a temporary file in the output's directory, renamed only when complete), and
  an interrupted run leaves no artifact or the previous one intact.
- **No new dependency.** No `unsafe` (so no memory mapping).
- **API changes to `brygge-ir` are allowed** (v0), and stated as Breaking. `IrBuilder::blob` cannot keep
  returning `&[u8]` once blobs live on disk.

## Decisions

### D-1 — Batch A: attribute the peak (no change)

- For fish-shell, TortoiseHg and the largest synthetic scenarios, measure the peak at the phase boundaries:
  - after decoding, before serializing;
  - during `to_bytes`;
  - during the file write;
  - during `verify` and `inspect`.
- Report how much each of the three holders (the content store, the serialization buffers, the reader's copy)
  and each decoder's working set contributes.
- **The later batches' targets are set from this table,** as RFC 010's were.

### D-2 — A streaming artifact writer

- `artifact::write_to(ir, …)` writes **directly to the output's temporary file:** the magic, the format, the
  version, a 32-byte placeholder, then the metadata and the blobs, in canonical order, hashing as it goes. Then it
  writes the digest at its offset. **No whole-artifact buffer.**
- `to_bytes` stays for small artifacts and the tests, built on the same code, so the two can never diverge. A
  property test asserts that they give the same bytes.
- **Expected:** the serialization copies disappear. At fish-shell scale that is about half the peak; batch A
  confirms the figure.

### D-3 — A disk-backed content store during decoding

- While a decoder runs, each new blob is **appended to a spill file** in the output's directory, and only an index
  (`BlobId` → offset, length) stays in memory. D-2's writer then copies the blobs from the spill file, in id
  order, into the artifact.
- **Disk:** the spill file temporarily doubles the disk the artifact needs. The artifact format requires blobs
  sorted by id, which cannot be written as they arrive. State this in the guide, and refuse with `ResourceLimit`
  if a write fails for lack of space, never a partial artifact.
- **The spill file** is private (created with mode 0600, in the output's directory), removed on success and on
  any error, and never read by anything but this decode.
- **Where a decoder needs a blob back** (`IrBuilder::blob`, which SVN delta bases use), it reads it from the spill
  file. That makes `IrBuilder::blob` return owned bytes (Breaking, v0).
- **Whether small decodes spill too** (one code path, versus a threshold) is decided from batch A's timings, by
  the architect.

### D-4 — A streaming reader for `verify`, `inspect` and `--against-source`

- **The digest is checked in one streaming pass.** The metadata is decoded. Blobs are read by offset only when a
  check needs their bytes (replay, content addresses), and dropped after.
- `from_bytes` stays for in-memory callers, on the same parser.
- Every existing reader check (canonical order, referential integrity, content addresses) keeps its exact
  semantics and errors. The mutation tests (C-2g) cover the new reader too.

### D-3a — `--spill-dir` (the owner's OQ-2 addition)

- **The location is a parameter from the start:** the library takes the spill directory. The default is the
  output's directory, as OQ-2 rules.
- **The CLI exposes it in 0.5.0** as `--spill-dir <DIR>`.
  - **Why it is sound:** only the *temporary artifact* must sit beside the output (C-7's atomic rename). The spill
    file is read back and deleted by the same decode, so it may live on any local filesystem.
  - **The use case is real:** an output on a small or network-mounted disk.
- **The guardrails:**
  - the directory must already exist, and be a directory. If not, it is a usage error (exit 2), never created
    silently;
  - the spill file has a unique name, is created with mode 0600 and `create_new`, and is removed on success, on
    error, and on interruption as far as the process can;
  - its location never reaches the IR or the provenance, since it cannot change the output;
  - the guide says plainly that a RAM-backed directory (`tmpfs`) moves the memory cost rather than removing it,
    and that the spill needs about the artifact's size in free space.
- **The help text and the guide** present it as an operational knob, never as something that changes the result.

### D-5 — What does not change

- The artifact bytes, the contract, the decoders' output, and the ceilings (C-2d).
- **RR-total-memory narrows, but does not close:** the metadata still grows with the atom count (about 5 KiB per
  atom for history-heavy repositories, RFC 010). The threat model is restated at the cut.

## Open questions (the owner's)

- **OQ-1 — The artifact's size.** **Ruled (2026-10-20): deferred, and *scheduled*, not only documented.** It is
  the planned theme **0.6.0 "Compact artifacts"** (`ROADMAP.md`), following no date, by its trigger:
  - its RFC opens when 0.5.0 is released;
  - or earlier, if prikk's import design needs smaller artifacts.

  That RFC designs a compressed blob encoding in a new contract version, with a pinned, deterministic codec and
  its reproducibility promise, and asks prikk's view by letter. Until then, the guide documents external
  compression for transport.

  The original question follows. fish-shell's artifact is 4.86 GB, because the IR stores every unique blob
  uncompressed.
  - **Recommended: defer, and document.**
    - Compressing the blob store is a **contract change**. Worse, **the digest covers the stored bytes**, so
      compressed output would have to be byte-identical across compressor versions forever, which no general
      compressor promises.
    - Artifacts compress well externally (for example `zstd file.brygge`) for transport.
    - Revisit if prikk's import design needs a smaller artifact.
  - **Alternative:** design a compressed blob encoding (contract 0.3.0), with a pinned, deterministic codec.
- **OQ-2 — Where the spill file lives.** **Ruled (2026-10-20): in the output's directory by default; the owner
  asked for the capacity to change it, which D-3a provides (`--spill-dir`).** The original question follows.
  - **Recommended: in the output's directory,** next to the temporary artifact. It is the same filesystem, so
    the space accounting is honest, and the cleanup sits with the existing atomic-write path (C-7).
  - **Alternative:** the system temporary directory, which may be a small RAM-backed `tmpfs`, and so would move
    the memory problem rather than solve it.

## Order and proof

1. **Batch A:** the attribution table (D-1).
2. **Batch W:** the streaming writer (D-2): byte-identity on every fixture, the bench and the real corpus, plus an
   A/B.
3. **Batch S:** the spill store (D-3), with the same proof, and the disk and cleanup tests (an interrupted decode,
   a full disk).
4. **Batch V:** the streaming reader (D-4), with the same proof, plus the reader mutation tests.

Each batch is shippable on its own; 0.5.0 ships when W, S and V are in. The threat model is revised at the cut.

## Consequences

- An ordinary machine decodes and verifies mid-size real projects.
- Artifacts stay byte-identical.
- `brygge-ir`'s blob access becomes owned (Breaking, v0).

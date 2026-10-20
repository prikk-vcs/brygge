# Handoff — RFC 015 D-3 and D-3a: a disk-backed content store during decoding, and `--spill-dir` (batch S)

**Governing:** RFC 015 D-3 and D-3a, and reviews 074 and 075. **Crates:** `brygge-ir`, the four decoders, and the
CLI. One review request (two if it grows: S-1 the store, S-2 the decoders and the CLI). **Target:** fish-shell's
`brygge decode` peak (P3) from 4.9 GiB to **under 1 GiB**, with **byte-identical artifacts.**

## 1. The store: one abstraction, two backings

- **`ContentStore` gets a file backing** beside today's in-memory one:
  - **memory:** as today. It remains the library default, for embedders, the tests and small uses;
  - **file:** blobs are appended to a spill file as they are added. Only an index, `BlobId` → (offset,
    length), stays in memory. A blob already present (the same id) is not written again.
- **The read API becomes owned and fallible,** because a file-backed blob cannot be lent as `&[u8]`:
  - `get` → `Result<Option<Vec<u8>>>`;
  - `IrBuilder::blob` → `Result<Option<Vec<u8>>>`;
  - `iter_sorted` → an iterator over ids, plus a read per id.
  - Keep one code path: the memory backing implements the same calls.
  - **This is Breaking for `brygge-ir`** (v0). State it in the CHANGELOG.
- **`write_to`** (batch W) reads each blob from the backing in id order, and streams it into the artifact. Never
  more than one blob is in memory at a time.
- **`Ir` equality** (used by `verify --against-source`): blob ids are SHA-256 of the content, computed when a blob
  is added (and checked on read, in V). So two stores with the same id set are equal. Keep `PartialEq`'s meaning
  exactly, and say how it is computed.
- **Design it so batch V can reuse the file backing with the *artifact itself* as the file:** the index then
  points into the artifact's blob section, and nothing is copied. You are not building V here; do not close
  that door.

## 2. The spill file: safe, private, never left behind

- **Placement:** the directory given (D-3a), by default the output's directory (OQ-2).
- **Creation:** a unique name (`.brygge-spill-` followed by the pid, the time and a counter; retry on
  collision), with `create_new` and **mode 0600** on Unix.
- **Never left behind, even when killed:**
  - **Unix:** remove the directory entry right after opening, and keep using the open handle (the file vanishes
    when the process ends, however it ends);
  - **Windows:** open with `FILE_FLAG_DELETE_ON_CLOSE` (`OpenOptionsExt::custom_flags(0x04000000)`, std only,
    no `unsafe`).
  - Prove both in tests: the directory holds no spill file during the decode on Unix, or after a killed decode
    on Windows (CI). If either cannot be made to hold on a platform, stop and report.
- **Disk full,** or any write error: a typed `ResourceLimit` or `Io` error, naming the spill directory. The
  temporary artifact is removed, as C-7 requires. Never a partial artifact.
- **Its location never reaches the IR or the provenance.**

## 3. Decoders: never materialize the content

- **Every decoder hands each blob to the builder as soon as it has it,** and keeps only the `BlobId`.
- **CVS especially:** today `FileRev` carries `content: Vec<u8>` through clustering. That is review 074's extra
  CVS holder (`cvs-revs`: 3.3× content after W). **Carry the `BlobId` instead**: add the blob when the revision's
  content is produced, and let clustering see only ids.
- The output must stay byte-identical: the same blobs, the same ids, the same ops. Check the dead-revision and
  vendor-skip paths especially.
- **SVN delta bases** read back through `IrBuilder::blob`, which is now owned. Unchanged in effect.
- **Git and hg:** confirm that no decoder-side cache holds blob bytes beyond the current commit or revision.
  Report what each one keeps.
- Each decoder's `Options` gains the spill location (`Option<PathBuf>`), or a builder parameter: choose one and
  say which. `None` means memory.

## 4. The CLI and `--spill-dir`

- **`brygge decode` always spills** (one code path), into `--spill-dir <DIR>` if given, otherwise the output's
  directory.
- **`--spill-dir`:** the directory must already exist and be a directory; otherwise it is a usage error (exit 2),
  never created silently.
- Add it to the help text, and to `docs/src/guide/` (the decode page). The guide says:
  - the spill needs about the artifact's size in free space;
  - a RAM-backed directory (`tmpfs`) moves the memory cost rather than removing it;
  - the option never changes the artifact.
- **Small decodes:** measure the cost of always spilling (the A/B below). **If any bench or corpus row's decode
  time rises by more than 10%,** report it, and the architect decides on a threshold. Do not add one yourself.

## 5. Proof

1. **Byte-identity:** every fixture, the bench corpora, and **all real-corpus entries** (`cmp` against `main`).
2. **The A/B** with `brygge-bench profile`: P1 and P3 (peak and time) for fish-shell, TortoiseHg, `cvs-revs`,
   `svn-dump` and `git-commits`. **The targets:** fish-shell P3 under 1 GiB; `cvs-revs` well below its 3.3×.
   Report what remains, holder by holder, as best you can.
3. **The spill-file tests** (§2): mode 0600, `create_new`, unique; gone during the decode (Unix) and after a
   killed decode (Windows CI); disk-full → typed error, and no artifact (simulate with a small `tmpfs` or a
   size-limited file system on Linux, or skip with a message where impossible).
4. **`--spill-dir`:** a missing directory, or a file, gives exit 2; a good directory is used; the artifact is
   byte-identical to the default's.
5. **The mutation tests,** both depths. The store's file backing is fed mutated index and offset cases through
   its read path (a truncated spill file must give a typed error).
6. **The usual gates** (stable and 1.87, clippy on three targets, deny, audit, the isolation check, the
   dev-dependency guard). **Not** the publish dry-run (review 075). **Windows matters here:** this batch touches
   platform file APIs, so use a **topic branch**, with CI's Windows job showing the spill tests run.

## 6. CHANGELOG `[Unreleased]`

- **Added:** `--spill-dir`.
- **Changed:** `brygge decode` keeps file contents on disk while decoding (the peak is now about the metadata,
  not the content: fish-shell 4.9 → N GiB; artifacts byte-identical).
- **Breaking:** `brygge-ir`'s blob access is owned and fallible (`ContentStore::get`, `IrBuilder::blob`), and
  the decoders' `Options` gain the spill location.

## 7. Review request

`.git-exclude/review-requests/051-spill-store.md` (or `051-…-s1` and `052-…-s2`), with the topic-branch routine.

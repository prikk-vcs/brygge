# Handoff — 0.2.0, batch C: maintenance (small, independent)

**Governing:**
- RFC 007 (CVS) and `brygge-03` v0.4, for items 1 and 2;
- RFC 012 D-11 (the book), for item 3.

**Order:** independent of batch A (measurement); build it in parallel. One review request.

---

## 1. The CVS drop reason names a version (RFC 007 §2.1)

- `crates/brygge-decode-cvs/src/decode.rs:161`, `BRANCH_REASON`, begins "brygge 0.1.0 imports the CVS
  main line only; …". It appears in every CVS artifact with branch revisions, and it names a version that
  is no longer running.
- **Change it** to: *"brygge imports the CVS main line only; branch history is planned for a later release
  (0.3.0). Keep the source repository."* Keep the rest as it is.
- Update the tests that assert the text, and `docs/src/guide/cvs.md` if it quotes it.
- **This is the only version-named string in product code** (the architect searched), so nothing else
  changes.
- **CHANGELOG `[Unreleased]`:** a **Changed** line noting that CVS artifacts' drop reason text changes.

## 2. Close `RR-cvs-read-toctou` (threat model v0.4)

- **The residual:** the CVS scanner examines each entry without following links, then opens `,v` files by
  path. A symlink swapped in between would be followed.
- **Unix.** After opening a `,v` file, compare the opened file's identity (`File::metadata()`, then
  `std::os::unix::fs::MetadataExt::dev()` and `ino()`) with the `lstat` taken during the walk
  (`symlink_metadata`, kept with the entry). A mismatch is `Error::Read("<path> changed while being
  read")`, with the path shown losslessly.
- **Windows.** `std`'s file-index API is unstable, so instead open with `FILE_FLAG_OPEN_REPARSE_POINT`
  (`std::os::windows::fs::OpenOptionsExt::custom_flags(0x0020_0000)`), which opens a link itself rather
  than its target. Then refuse if the opened handle's metadata is a symlink or a reparse point; that is the
  same `Read` error.
- **The order is unchanged:** the refusal of a symlink found during the walk (`symlink-in-repository`)
  stays. This only closes the window between the walk and the open.
- **Tests:**
  - **Unix:** take the walk's `lstat` of a regular `,v`, replace the file with a symlink to another valid
    `,v`, and then run the read step. It is refused.
  - **Windows:** the same, with a file symlink (the privilege is available on CI runners; skip only off
    CI, as the existing tests do).
  - **A normal repository** decodes byte-identically.
- **Put the check in one place,** the scanner's `read_bounded` path. No other reader in the CVS decoder
  opens source files.

## 3. Cache the pinned mdBook in CI (RFC 012 D-11)

- `cargo install mdbook --version 0.5.4 --locked` compiles mdBook on every run of `ci.yml`'s `repository`
  job and of `docs.yml`, a minute or two each time.
- **Cache the installed binary with `actions/cache`,** keyed on the runner OS and the pinned version, read
  from `tools/install-mdbook.sh`. For example, the key can include a hash of that file, so changing the
  pin invalidates it. `install-mdbook.sh` already skips the install when the exact version is present.
- Only the binary is cached, not the cargo registry. No other workflow change.

## 4. Proof

- The usual gates, default toolchain and 1.85.
- The tests of item 2 on Linux locally. The Windows test runs in CI; report its result after the push.
- For item 3: after the push, a second run shows the cache hit and the saved time. Report both runs.

## 5. Review request

File `.git-exclude/review-request/023-maintenance-0.2.0.md` with the standard sections. After approval,
commit and push, and append the CI results (the Windows item-2 test; the cache hit).

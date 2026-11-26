# Handoff — RFC 021 batch G: the Git decoder carries submodules; the reports; `encode git` writes them (request 090)

**Governing:** RFC 021 D-3 to D-6, as refined at batch K (the flat wire). **After:** batch K, on `main` at `81f8b43`.
**On a topic branch,** `batch-g-submodules`: the tests run `git`, so the Windows and macOS logs must show them
running. Push only after approval, and name the branch.

## 0. Evidence plan first (N-3), with `uptime` for any timing

- **Level 1:**
  - the Git decoder's op for each change of a submodule entry: added, moved (a new commit), replaced by a file, a
    file replaced by one, removed;
  - D-4's report lines and machine keys;
  - `encode git`'s stream line.
- **Level 2:**
  - a repository built in the test with `git` (`git update-index --add --cacheinfo 160000,<id>,<path>`, so no
    second repository is needed), with every change above;
  - then decode, `verify`, `--against-source`, `encode git` with the check, and `git ls-tree` on the result.
- **Level 3:**
  - **jq** (`real.toml`, pinned): decode with its submodule carried, then `verify`, `--against-source`
    (`corresponds`), `encode git` with the check (`corresponds`) and `git fsck --strict`;
  - in the result, `git ls-tree -r HEAD` shows the `160000` entry at **the same path and commit** as the source's;
  - **every other Git corpus entry,** `ir-equal` against batch K's build (§5).

## 1. The decoder (D-3)

- **The tree entry:** `walk_tree` and `diff_to_ops`. A tree entry of kind `Commit` becomes
  `Content::Submodule { commit }`, its 20 bytes as stored (the repository is SHA-1; SHA-256 repositories stay
  refused).
- **Never opened:** it is never followed, fetched or recursed into. The test proves no object lookup is made for
  the id. A commit id that no object in the repository has is normal for a submodule.
- **The ops:**
  - **added:** `Add`;
  - **a new commit at the same path:** `Modify` (same kind);
  - **file or directory ↔ submodule at one path:** `Replace`, and for a directory, its files' `Delete`s plus the
    `Replace` or `Add` that the existing diff rules give. Say exactly which in the request;
  - **removed:** `Delete`.
- **The path checks** (C-2c) apply as to any entry.
- **The floor:** remove `SUBMODULE` from `floor.rs`'s list, and its refusal, its message and its test. The new list is
  what `params["floor"]` records. The guide's floor table (`git.md`) loses its row.
- **`.gitmodules`:** no change. It is a file, carried as content.

## 2. The reports (D-4, condition A)

**The sentence:** wherever a submodule appears, the user reads one sentence, in exactly these words:

`submodules: P path(s) in the history, T at the branch and tag tips; their own repositories were not read, imported or verified`

- **`P`** counts the distinct paths that ever held a submodule;
- **`T`** counts the distinct paths that hold one at any ref's target. Define both in `machine-output.md`.

**Where it appears:**
- **`decode`:**
  - the sentence, after the fidelity lines, when `P > 0`;
  - machine: `decode_submodules_version=1`, `decode.submodules.paths=P`, `decode.submodules.at_tips=T`, absent when
    `P = 0`;
- **`inspect`:**
  - the same sentence;
  - **`inspect --atoms`:** an atom's op count gains `, K submodule` when K of its ops are submodule entries. That
    replaces the RFC's "`submodule <hex>`", because `--atoms` prints counts, not ops (request 089). The architect
    records this refinement in RFC 021;
- **`encode git`'s report:**
  - `encode_git.submodules.paths=P` and `encode_git.submodules.at_tips=T`;
  - the human `wrote …` line gains the same sentence.

## 3. `encode git` and the check (D-5, D-6)

- **The stream:** `M 160000 <40-hex> <path>`, `fast-import`'s gitlink form, an object id, not a mark.
- **The pre-flight:** remove batch K's temporary `Refusal::Submodule`. `160000` leaves the refused modes.
- **The check (G2):** the result is decoded by this batch's Git decoder, which now carries submodules. Compare a
  submodule entry like any other: path, kind and commit id. A blob in one, and a submodule in the other, at a path is
  a difference.
- **`verify --against-target git`:** the same.
- **Level 2:** tamper a result's submodule pointer (`git update-index --cacheinfo` in a copy, then a commit). The check
  gives `does-not-correspond`, naming the path.

## 4. The guides and CHANGELOG

- **`docs/src/guide/git.md`, a "Submodules" section:**
  - what is carried, and what is not;
  - **the sentence;**
  - that `.gitmodules` is ordinary content;
  - that a submodule pointing at a commit its repository no longer has is carried as the source holds it.
- **`docs/src/guide/git-target.md`, a "Submodules" section**, with RFC 021 D-4's four points:
  - the result's submodules point at the same commits;
  - `git submodule update` fetches from the source's own `.gitmodules` URLs, so it contacts those hosts, and brygge
    contacts none;
  - brygge checked none of the submodules;
  - a path missing from `.gitmodules` cannot be fetched.
- **`writing-an-encoder.md`:** the example of holding one (as `encode git` does), and of refusing one by name.
- **`CHANGELOG.md` `[Unreleased]`:**
  - **`### Added`:** "Git repositories with submodules decode: each submodule is carried as a pinned reference (its
    path and the commit it pins), and its own repository is never read, imported or verified. `encode git` writes it
    back as a submodule (RFC 021)";
  - **`### Changed`:** the Git floor no longer refuses `submodule`.

## 5. The same history elsewhere

**Every Git corpus entry other than jq** (fish-shell and eza, default and `--infer-renames`), `ir-equal` against
batch K's build. **Expected:** equal, except `params["floor"]`, which differs by `submodule` alone.
- Make `ir-equal` report a provenance difference by key, so that this one is visible and nothing else hides
  behind it.
- **Any other difference stops the batch.**

`--against-source` with this build on a K-built artifact gives `corresponds`, with the floor note (review 140).

## 6. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87. All are installed
  on this machine;
- `cargo test --workspace` summed, on stable and 1.87, plus the no-identity run;
- the deep mutation sweep (the Git decoder changes);
- the isolation check, links, the book, the release tools, `cargo deny`.

## 7. Request

- **File** `.git-exclude/review-requests/090-git-submodules.md`.
- **Push the branch only after approval.**
- **Then:** the 0.10.0 cut. **Letter 008** (in `send/draft/`, authorized) must have been sent by the owner before
  the tag.

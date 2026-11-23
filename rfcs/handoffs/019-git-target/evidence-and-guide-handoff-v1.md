# Handoff — RFC 019 D-8: the evidence, and the guide (batch G4, request 075)

**Governing:** RFC 019 D-8 and Amendment A. **After:** G5 (on `main` at `4ffb438`). **On `main`.** This is the last
batch before the 0.8.0 cut prep, which is a separate handoff.

## 0. Evidence plan first (N-3), and a standing rule

- **Open the request with the plan.** Most of this batch is level 3 by nature. Say which runs are level 3 and which
  findings, if any, got a cheaper test.
- **A standing rule from now on (a missing git identity has now broken CI twice: 068, then 074):**
  - before filing any request, run the workspace tests once with `GIT_CONFIG_GLOBAL=/dev/null
    GIT_CONFIG_SYSTEM=/dev/null HOME=$(mktemp -d)`, the CI runners' condition. Report the sum;
  - add the same to `docs/src/development/` where the local checks are listed.
- **On 074's `4ffb438`:** it was pushed after a CI failure, without a review. 114 says a change made after an
  approval is reviewed first, as 068's CI fix was. It was test-only and correct, so nothing is reverted. Hold the
  next one.

## 1. One behaviour to settle first: `--branch-lines` with no layout

- **Decode a Subversion source with no trunk/branches/tags layout,** using `--branch-lines`. The existing level-2
  fixture `an_svn_fixture_with_no_trunk_layout…` has its file at the root.
- **If it yields no atoms** (every revision touches no root), it must **refuse** instead: exit 20, "the
  trunk/branches/tags layout was not found, so `--branch-lines` has no branch to follow; decode without it". A
  decode that turns a whole history into nothing is never a success.
- **A level-1 test** for whichever behaviour holds, and the fix if needed.

## 2. The measurement method

- **Extend `tools/bench/release-corpus.py`, without changing its existing output:**
  - `--decode-args "<args>"`, passed through to `decode` (for example `--branch-lines`);
  - `--encode-git`: after `verify`, run `encode git` (with its check) into a temporary directory, as one more child
    under `os.wait4`, and print `encode_time_ms`, `encode_peak_kib`, and the report's `encode_git.check` and
    commit count.
- **Same rules as before:** one child per measurement, one session, one machine.

## 3. The runs (level 3)

**One table in `tools/bench/README.md`,** "Result — Git target (RFC 019 D-8)". One row per run: commits, refs, the
check result, decode time and peak, encode-with-check time and peak, and `git fsck --strict`.

| Source | Repository | Decode | Before encoding |
|---|---|---|---|
| Git → Git | `eza`, `fish-shell` | default | none |
| Mercurial → Git | `tortoisehg-thg` | default | none |
| Subversion → Git | `wordpress-core` | `--branch-lines` | `transform authors --require-all` with a placeholder map |
| Subversion → Git, deleted branch | a public repository you propose (below) | `--branch-lines` | as above |
| CVS → Git | `openbsd-games`, `openbsd-cpio` | `--reconstruct-refs` | as above |
| CVS without `--reconstruct-refs` | `openbsd-cpio` | default | none: show the refusal and its message |

**Also:**
- **Git → Git, the strongest check:** for eza and fish-shell, compare every commit's tree, and every author,
  committer and time, with the source commit named by `Brygge-Source-Id`, using `git log --all --format`. That is
  the architect's method from review 136. Report the counts, and any difference.
- **Subversion → Git:** for wordpress-core, compare `trunk`'s and `1.5`'s tips with `svn export`, as 074 did. The
  only expected difference is `svn:keywords`.
- **The deleted-branch repository:**
  - propose a public Subversion repository, small enough to dump, under an open license, that really has a
    deleted branch (or a deleted then re-created one);
  - give the URL, the license, the revision pin, and which revision deleted the branch;
  - add it to `real.toml` with the S-3 canonical hash, fetched with `tools/fixtures.sh`;
  - its Git result must show the line under `refs/brygge/other/…@r<N>`, and the check must pass.
  - **Search properly this time:** SourceForge and Apache projects' public Subversion repositories are the likely
    places. If, after a real search, there is none, report what you searched.

**Stop and report** if any run's check fails, a peak exceeds 1.5× its decode, or `fsck` complains.

## 4. The guide: `docs/src/guide/git-target.md`, finished

- **"Migrating from each source":** the exact commands for Subversion (`--branch-lines`, authors, encode), CVS
  (`--reconstruct-refs`, authors, encode), Mercurial and Git.
- **"What the result is":**
  - the trailers;
  - `refs/brygge/other/`;
  - the report beside the repository;
  - that the commit ids are new;
  - that the repository is bare (`git clone repo.git`).
- **"How brygge checks it":** what is compared. **The cost:** the check about doubles the time of the write,
  measured on fish-shell and named.
- **"Limits in 0.8.0":**
  - **a Subversion repository with no trunk/branches/tags layout:** under `--branch-lines`, whatever §1 settles.
    Without it, the whole tree is in every commit, which is still a faithful result;
  - **merges:** not inferred from `svn:mergeinfo`;
  - **not supported:** submodules, LFS, SHA-256 repositories;
  - non-UTF-8 annotated-tag messages are refused.
- **"Evidence":** a short summary of §3's table, with a link to the bench README.
- **`README.md` (the repository's)** and the book's introduction: one sentence each, saying that brygge also
  writes Git repositories (`encode git`).

## 5. Checks

- the usual gates on stable and 1.87, **plus the §0 no-identity run**;
- links and the book, with no warnings;
- the deep mutation depth, if `src/` changes (§1).

## 6. Request

- **File** `.git-exclude/review-requests/075-git-target-evidence.md`.
- **Push after approval only** (114).
- **Then:** the 0.8.0 cut prep, handed off separately.

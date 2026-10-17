# Handoff — RFC 014 D-4: real-world validation on public repositories (batch R)

**Governing:** RFC 014 D-4 (accepted 2026-10-17). **After batch A;** independent of batch M. **Two review
requests:**
- **R-1, the list (short, first):** the proposed repositories, before anything large is downloaded. The
  architect approves or amends it.
- **R-2, the run:** the tooling, the results and the shapes.

## Why

Every input brygge has been tested on is synthetic: repositories our scripts build. Real projects carry what
we did not think of: decades-old tool versions, odd encodings, huge or messy histories. This batch decodes
**existing public open-source repositories,** not dummy ones. Their **shapes** are also what brygge promised
prikk (letter 003).

## R-1 — Propose the list

**Two or three repositories per source kind,** each with:
- the URL;
- the license;
- the pinned revision (a commit, changeset, revision number or date for CVS);
- the expected size (commits, files, MB);
- **which brygge features it exercises.**

**Criteria:**
- publicly reachable;
- a clear open-source license;
- **decodable in minutes** on a laptop (mid-size: well beyond our fixtures, nowhere near the largest
  projects);
- **together, per kind, they cover what brygge claims:**

  | Kind | What the set must exercise |
  |---|---|
  | Git | renames, merges, tags (annotated and lightweight), at least one non-UTF-8 or unusual-encoding message if one can be found; a repository where `--infer-renames` matters |
  | hg | named branches, **a branch with several heads**, bookmarks, **zstd revlogs** (a repository created or upgraded with `format.revlog-compression=zstd`, or note that you converted one locally with `hg debugupgraderepo`), long paths if possible; Mercurial's own repository is a natural candidate if it fits the size |
  | SVN | the standard layout with branches and tags, copies, property changes; **dumped with `svnrdump`** from a public server (the remote case D-2 exists for), up to a pinned revision |
  | CVS | branches (nested if one can be found), tags, a vendor import; the repository's `,v` files are needed, typically over rsync from a project that still publishes its CVS tree |

- **Say how each is fetched,** and where it is stored: **outside the repository,** in a cache directory the
  script takes as an argument. Nothing downloaded is committed.
- **Downloaded data is untrusted:** fetch it into a fresh directory, and never run anything from inside it.

## R-2 — The tooling and the run

- **`tools/corpus/real.toml`:** the approved list (kind, URL, license, pinned revision, fetch method).
- **A script** (a `tools/bench` subcommand, or `tools/corpus/run.sh`; say which), taking the cache
  directory. For each repository, it:
  1. fetches it at the pinned revision, if it is not already cached;
  2. decodes it, with and without `--reconstruct-refs` (and `--infer-renames` for Git);
  3. runs `verify`, and `verify --against-source`;
  4. records the time and peak (the bench harness's method);
  5. **computes the shape** from the artifact: the commit (atom) count; the files per commit (min, median,
     p90, max); **the tree size over depth** (the files at each atom, summarized: start, median, max, end).
     Use `brygge-ir`'s public reader, in the `publish = false` bench crate, and add nothing to the published
     crates;
  6. writes one row per repository.
- **The results go in `tools/bench/README.md`,** in a section "RFC 014 D-4: real repositories". It holds:
  - the repositories and revisions;
  - the brygge version;
  - the machine;
  - the exit codes;
  - the verify verdicts;
  - the time and peak;
  - the drop and flag counts (from the report);
  - the shapes.
- **Every mismatch, refusal or surprise is a finding.** That includes a `verify --against-source` that does not
  answer `corresponds` / `reproduces`, a refusal by name, an unexpected drop, a crash, or a ceiling hit. Report
  each one with what you found. **Do not fix them in this batch:** the architect hands each one on.
- **The shapes are raw data for prikk.** Leave them as numbers; the architect writes the letter when prikk
  schedules its import work.

## Proof

- **R-1:** the list, against the criteria.
- **R-2:** the tooling, the full table, and the findings. A second run from the cache reproduces the same
  artifacts (`cmp`).
- The usual gates for any code touched, and the publish dry-run unchanged.

## CHANGELOG `[Unreleased]`

- **Added:** "Real-world validation: brygge decodes and verifies pinned public repositories of each source kind
  (`tools/corpus/`); the results and history shapes are in `tools/bench/README.md` (RFC 014 D-4)."
- Plus one Fixed line for each finding fixed later through its own handoff.

## Review requests

- `.git-exclude/review-requests/042-real-corpus-list.md` (R-1);
- `…/043-real-corpus-run.md` (R-2).

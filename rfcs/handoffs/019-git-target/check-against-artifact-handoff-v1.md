# Handoff — RFC 019 D-6: brygge checks the Git result against the artifact (batch G2, request 072)

**Governing:** RFC 019 D-6, with the G1 refinements (the reachability refusal, and F-7's `Brygge-Source-Id` forms).
**After:** G1 and 071 (on `main` at `b130d1e`).
**Branch:** a topic branch, `batch-g2-check`. It runs `git` and the Git decoder over a repository `git` wrote, so the
Windows and macOS logs must show its tests running.

## 0. Evidence plan first (N-3)

Open the request with the questions, the level each is checked at, and what remains. In particular:
- every kind of difference in §2 is **detected** at level 1, from a hand-built pair;
- and at level 2, from a real repository tampered after the encode;
- the memory question of §4 is answered with a measurement, not an estimate.

## 1. What runs, and where

- **`encode git`:** after `fast-import` and **before** the partial directory is renamed (G1 §1, step 5), brygge
  decodes the partial repository in-process with `brygge-decode-git`. It uses the CLI's decode path and the same
  spill rules. Then it compares the result with the artifact (§2):
  - **a match:** the rename and the report proceed, and the report gains `encode_git.check=corresponds`;
  - **any difference:** exit 1, the first differences on stderr (at most 20, then a count), the partial directory
    removed, and nothing at `<dir>` or at the report path.
- **`brygge verify <artifact> --against-target git <repository>`:** the same comparison, standalone, over a
  repository someone kept, bare or not.
  - The repository is read only. Its own refs are listed with the §1 environment of G1 (`git for-each-ref`, no user
    config).
  - **Machine keys:** `verify.against_target=corresponds | does-not-correspond | not-checked`, plus
    `verify.against_target.detail`.
  - **Exit codes:** `does-not-correspond` is exit 50 (the `verify` fail code). `not-checked` follows
    `--against-source`'s rule.
  - `--against-source` and `--against-target` may not be given together (a usage error). One question per run.
- **The comparison itself** is one function, `pub(crate) fn compare_git_target(ir: &Ir, digest: &Digest, result:
  &Ir, result_refs: &[GitRef]) -> Comparison`, in a new module `crates/brygge/src/encode_git/check.rs`.
  - It **may use `brygge_ir` only**, so the isolation rule of G1 §2 covers it too.
  - The CLI does the decoding and hands it both IRs.

## 2. What must be equal

**Matching.**
- Each commit of the result is matched to an artifact atom by its `Brygge-Source-Id` trailer, parsed back with
  F-7's forms. The parser is the inverse of the writer, and level 1 shows they round-trip for every form.
- **Not checked:** two atoms in the artifact with one source id. The outcome is `not-checked`, naming them.
- **A difference:**
  - a result commit with no trailer, or one that names no atom;
  - an artifact atom no commit names.

**Per matched pair:**

| What | Rule |
|---|---|
| parents | the result commit's parents, mapped back through the matching, equal the atom's parents in order |
| tree | the full path → (mode, content id) state after the commit equals the state after the atom. The two decoders may record different ops for the same change: an SVN replace against a Git modify, or a no-op modify Git never records. So compare **states**, never ops. Content ids are the IR's own SHA-256 addresses, so equal bytes give equal ids in both. |
| author, committer | name, email, time and offset equal, except fields a `Brygge-Supplied` or `Brygge-Altered` trailer names. Those must equal exactly what G1's rules write (for example `<>`, the author's own claim, `+0000`, the documented replacement). |
| message | the result's message bytes equal what G1's writer renders for that atom: the source message, the blank line, the trailer block. Re-render it with the same functions and compare the bytes, including `encoding`. |

**Refs:**
- the set of the result's refs equals D-3's mapping of the artifact's refs: names, kinds and targets (through the
  matching);
- an annotated tag's tagger, time and message follow the same rules as a commit's;
- `refs/brygge/other/*` is read from the listed refs, since the Git decoder does not carry that namespace;
- **`HEAD`** names the default branch G1 chose.

**Nothing extra:** no commit, ref or tag in the result beyond the mapping. A kept repository someone has added a
branch to does not correspond. That is the intended answer, so state it in the guide.

## 3. Level 1 and level 2

- **Level 1, in `check.rs`'s tests:** one hand-built pair per row of §2 with exactly one difference, each
  detected and named. Also a matching pair (corresponds), and the duplicate-source-id case (not checked).
- **Level 2, real `git`:** encode a fixture from each of the four decoders and check it: corresponds. Then tamper
  with a copy of the repository, one change per test:
  - amend a commit's message;
  - rewrite a blob in a commit with `git replace` and `filter-branch`, or with `fast-import` from a modified
    stream;
  - add a branch;
  - move a tag;
  - change `HEAD`.

  `verify --against-target` must give `does-not-correspond`, naming what changed.
- **`encode git`'s refusal path:**
  - prove that a failed comparison removes the partial directory and writes nothing;
  - do it through an internal seam (for example the comparison passed in as a function in a unit test of the CLI
    path). **Never** an environment variable or a flag in the shipped product.

## 4. Memory and time

- **The bound:** a tree **state** per atom must not be held for the whole history.
  - Compute the states in canonical order, keeping each atom's state only until its last child has used it, as
    the bench's shape summary does with its tree sizes.
  - Or compare per-commit digests of the state, folded incrementally. Choose one and say why.
- **The bound to meet:** the encode-with-check peak stays within **1.5×** the decode peak of the same repository.
- **The proof (level 3, in this batch):**
  - `release-corpus.py`'s method on fish-shell: decode it, then `encode git` with the check;
  - record time and peak for the decode, the encode alone (with the check bypassed through the internal seam, in a
    test build, not the product), and the encode with the check.

  If the bound fails, stop and report. Do not tune silently.

## 5. Documentation and CHANGELOG

- **`docs/src/guide/git-target.md`:**
  - a "How brygge checks the result" section: what is compared, the state-not-ops rule, the nothing-extra rule;
  - `verify --against-target`;
  - remove "G2" from "what this does not do yet".
- **`machine-output.md`:** the `verify.against_target*` keys, `encode_git.check`, and the exit codes.
- **`CHANGELOG.md` `[Unreleased]`, `### Added`:** "`encode git` checks its result against the artifact before
  writing it, and `verify --against-target git` checks a kept repository the same way."

## 6. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- `cargo test --workspace --locked` on stable and 1.87;
- the isolation check, covering `check.rs`;
- links, the book, the release tools, `cargo deny`;
- the deep mutation depth: `verify`'s own checks change, which is in `mutation.yml`'s path filter.

## 7. Request

- **File** `.git-exclude/review-requests/072-check-against-artifact.md`.
- **Push the topic branch only after approval,** and name it in the request.

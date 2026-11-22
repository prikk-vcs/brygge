# Handoff — RFC 019 Amendment A, D-9: `decode svn --branch-lines` (batch G5, request 074)

**Governing:** RFC 019 Amendment A (D-9), accepted by the owner on 2026-11-22.
**One refinement at handoff (the architect):** no new `DerivationKind` variant. An enum variant an older reader
does not know fails its read, because the critical-bit rule covers fields, not variants. The existing
`ReconstructedBranch` ("a branch reconstructed by convention (SVN path copies)") already names this derivation. A
line atom uses it with `params = {layout, root, line: "true"}`. **The contract is unchanged.**
**After:** G3 (on `main` at `c3a70ea`). **On `main`, no topic branch:** a decoder change, with no new process or
platform code.

## 0. Evidence plan first (N-3)

- **Level 1:** hand-built dump streams, one per rule of §2.
- **Level 2:** real `svnadmin` and `svn` repositories built in the tests, then decode, then `encode git` with G2's
  check, then `git ls-tree` and `git log`.
- **Level 3, in this batch:**
  - wordpress-core with `--branch-lines`: encode and check, and compare `trunk`'s and `1.5`'s trees with an `svn
    export` of the same paths at the same revisions;
  - a public Subversion repository with a **deleted branch**. Propose one, with its URL, license and a pin, in the
    request, and add it to `real.toml` (fetched with `tools/fixtures.sh`). If none fits, a level-2 repository with a
    deleted and re-created branch stands in, and say so.

## 1. The option

- `brygge decode svn <source> --out <artifact> --branch-lines`. It implies `--reconstruct-refs`, and uses the same
  layout policy.
- It is recorded in `params` (`branch_lines = "true"`), so `--against-source` re-derives with it.
- **Without it,** nothing changes, byte for byte. Prove that on wordpress-core: `cmp` of the artifacts before and
  after this batch, with and without `--reconstruct-refs`.

## 2. The rules (D-9, made exact)

**Roots.** The layout's directories: `trunk`, each `branches/<name>` and each `tags/<name>`, as `layout::classify`
already names them.

**Atoms.**
- **One per (revision, root it touches),** in revision order, and within a revision in canonical root order:
  `trunk`, then branches, then tags, each by bytes.
- **Its ops** are the root's ops with the root's prefix removed. A copy that **creates** the root gives no ops: the
  tree comes from the parent (below).
- **A copy into a root from a path inside another root,** at a revision (a file copied across branches), becomes the
  file's content as an `Add` at the new path, plus a marked `CopyRecord` naming the source atom on that line, the
  same way the stated layer records copies.

**Parents.**
- the previous atom on the same root's line;
- **the root's first atom, when the root was created by `svn cp <src>@<rev> <root>`:**
  - its parent is the newest atom on `<src>`'s line at or before `<rev>`;
  - and the starting tree is that atom's tree, with `<src>`'s subtree if `<src>` is a subdirectory of a root. In
    that case the parent still is that root's atom, and the ops make up the difference;
- a root created without a copy starts a new line, with no parent;
- **no merges** are inferred.

**A root deleted, then created again:** each life is its own line. Lines join only through the copy rule above.

**Outside every root:**
- ops on paths outside every root keep the existing flag (`paths outside the trunk/branches/tags layout`);
- they are recorded as a drop of class `Other`, with `what` naming the revision and the path;
- they are on no line.

**A revision that touches no root** gives no atom. It is counted as a representation-class drop, "revisions with no
branch change (N)". Revision 0 is the usual case.

**Refs:**
- **each live root's last line:** `trunk` and branches as `Branch`, tags as `Tag`, targeting that line's last atom.
  `Derived(ReconstructedBranch)`, as today;
- **each earlier life of a root, and every deleted root:** `RefKind::Other`, named `<root>@r<last revision it
  existed in>` (for example `branches/foo@r812`), `Derived(ReconstructedBranch)` with
  `{layout, root, state: "deleted"}`. `encode git` writes it to `refs/brygge/other/branches/foo@r812`. Check that
  the name passes `git check-ref-format`, and if `@` causes trouble, report it rather than choosing another
  character silently;
- **"deleted or moved branches/tags not represented (N)"** no longer applies with `--branch-lines`: every line has a
  ref. That count is 0, and the guide says why.

**Marks and ids:**
- every line atom is `Derived(ReconstructedBranch)` with `{layout, root, line: "true"}`;
- **`source.atom_id`** = the revision's 8 big-endian bytes, followed by the root's bytes;
- **F-7's Subversion form gains the root:** `r<N> <root>` (for example `r1234 branches/1.5`). Use plain `r<N>` for an
  8-byte id, as today, and `hex:` if the root is not printable UTF-8.
- **G2's parser reads both forms.**

## 3. Memory

- **What to keep:** a tree state per **live line**, not one per revision. The decoder already keeps only the
  snapshots a later `copyfrom` names (RFC 010 increment 1). Keep that rule per line.
- **Measure** wordpress-core's peak with and without `--branch-lines`, with `release-corpus.py`'s method. Report it.
  **Stop if it is more than 1.5×** the plain decode.

## 4. Documentation and CHANGELOG

- **`docs/src/guide/svn.md`:**
  - a `--branch-lines` section, with the rules of §2 in user terms;
  - the deleted-branch refs;
  - the drop and flag names.
- **`docs/src/guide/git-target.md`:**
  - for Subversion, the documented path is `decode svn --branch-lines`, then `inspect --authors`, then
    `transform authors`, then `encode git`;
  - **without `--branch-lines`,** the result keeps the whole repository tree in every commit. Say so in one plain
    sentence;
  - `Brygge-Source-Id`'s `r<N> <root>` form.
- **`docs/src/reference/ir-artifact-format.md`:** nothing changes, because no field and no variant is added.
- **`CHANGELOG.md` `[Unreleased]`, `### Added`:** "`decode svn --branch-lines`: each branch and tag becomes its own
  line of history, with its own files at the root of its tree, as a marked derivation. Deleted branches keep their
  history under `refs/brygge/other/` in Git (RFC 019 Amendment A)."

## 5. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- `cargo test --workspace --locked` on stable and 1.87, **summed**;
- the deep mutation depth: `brygge-decode-svn` changes;
- the isolation check, links, the book, the release tools, `cargo deny`;
- `verify --against-source` on wordpress-core's `--branch-lines` artifact: `reproduces`.

## 6. Request

- **File** `.git-exclude/review-requests/074-svn-branch-lines.md`.
- **Push after approval only** (114).
- **G4** (the evidence, the guide, and then the cut prep) is handed off after this.

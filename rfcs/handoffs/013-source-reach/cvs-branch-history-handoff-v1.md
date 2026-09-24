# Handoff — RFC 013 D-3: CVS branch history

**Governing:**
- RFC 013 D-3 as amended 2026-09-24: the earliest covering changeset, the branch tree, nested branches;
- OQ-1: with `--reconstruct-refs`;
- OQ-2: unnamed branches are still dropped;
- **OQ-6** (vendor branches, pending the owner; §8).

**Crates:** `brygge-decode-cvs`, and CI (`cvs` installed). **Batch C** of 0.3.0, independent of H and S.

**The size:** this is the largest item in 0.3.0. **File two review requests:**
- **030 (C-1):** the model, the parent rule, and single-level branches off the main line;
- **031 (C-2):** nested branches, tags on branch revisions, and the real-`cvs` fixtures at scale.

C-1 must stand on its own: gates green, and fixtures for what it covers.

---

## 0. Words used here

- **A line:** the main line (as today), or **one named branch**: a branch symbol, identified by **name** across
  files, never by number.
- **A file on a branch:** a file whose symbols give that name a **magic** branch number (`1.2.0.4`, so the
  branch is `1.2.4`, from branch-point revision `1.2`).
  - Literal (odd-length) symbols are vendor branches (§8).
  - A file without the symbol is **not** on the branch.
- **The line of a revision:**
  - the main line if `mainline::is_main_line` admits it (unchanged: trunk, plus the vendor branch while it is
    the default);
  - otherwise the branch named by `branch_symbol_name` for `branch_of(rev)` in that file;
  - otherwise **unnamed** (OQ-2: dropped and counted, as today).
- **A file's state on a line after a changeset `c_k`:** its latest revision in `c_0..c_k` of that line. It is
  absent if there is none, or if that revision is `dead`.

## 1. When branches are imported (OQ-1)

- **Without `--reconstruct-refs`: the main line only, exactly as today.** The one exception is the drop-reason
  text: it said "planned for a later release (0.3.0)", which is no longer true. The new text is:

  > *"CVS branches are imported only with `--reconstruct-refs`: a branch is identified by its symbol name.
  > Keep the source repository."*

  Every CVS artifact without branch revisions stays byte-identical.
- **With `--reconstruct-refs`:** everything below.

## 2. Each branch's changesets

- **Collect** each branch's revisions: every file's revisions on that branch number, directly on it. Revisions
  on a branch of that branch belong to the nested branch (§6).
- **Cluster them with `cluster::reconstruct`,** unchanged, **per branch**: the same `(author, log)` keys,
  window, per-file order, one revision per path, and `span-overlap-v1` confidence, computed within that
  branch only. Overlap never spans lines.
- **Branch changesets are `Derived(ReconstructedChangeset)`** with today's params, plus `line = <branch
  name>`. Their ops are `Stated` per file, as on the main line.
- **Within a branch,** changesets chain linearly in cluster order (as the main line does).
- **The confidence floor:**
  - the **whole-import** refusal still considers **main-line** changesets only, so its behaviour is unchanged;
  - a branch changeset below the floor is imported and counted in the existing `BelowConfidenceFloor` flag.
- **Content:** `contents_of_many` already rebuilds branch revisions from their branch point in one pass. Add
  the branch revisions (and their branch points) to `wanted`.
  - For nested branches, see §6.
  - The per-file one-pass cost stays linear: no per-revision `content_of` loop.

## 3. The parent: the earliest covering changeset (D-3, as amended)

For branch `B`, with **parent line** `L` (the main line in C-1; §6 in general), and **its covered files**
(the files on `B` whose branch-point revision is live and on `L`):

- **File `f` is at its branch point at `c_k`** when its state on `L` after `c_k` is exactly its branch-point
  revision.
  - That holds on a contiguous run of `L`'s changesets `[in_f, out_f)`.
  - `out_f` is where `f`'s next revision on `L` arrives, or the end of the line.
- **Exact:** if `max(in_f) < min(out_f)`, the parent is **`c_{max(in_f)}`**: the earliest changeset at which
  every covered file is at its branch point.
- **Approximate:** otherwise, the parent is the changeset of `L` with the most covered files at their branch
  points, **the earliest** on ties.
  - Count the branch in a `ConventionViolation` flag: what = "CVS branch point spans reconstructed
    changesets", count = the number of such branches. The reason names the rule.
  - Its **tree is still exact** (§4); only the parent edge is approximate.
- **Excluded from coverage:**
  - **A branched file whose branch-point revision is `dead`** (the file was added on the branch: CVS's dead
    `1.1` "initially added on branch" revision). It does not constrain the parent; its branch revisions add
    it.
  - **A file whose symbol names a revision that does not exist** (outdated with `cvs admin -o`). The file
    leaves the branch, and is counted in a drop record, "CVS branch symbols naming a missing revision (N
    files)". It is never a refusal of the import.
- **The vendor-import skip:** the skipped branch-point revision (`vendor_import_branch_point`) counts as
  present wherever that file's first vendor revision is, since they are identical and dated alike.
- **Why earliest, not latest** (the amendment):
  - every covered file's branch point is present at `c_{max(in_f)}`, so the cut is no earlier;
  - a later trunk changeset that only adds a file the branch does not carry is **evidence the cut preceded
    it**: CVS tags every live file in a full branch;
  - "latest" could also place a parent after the branch's first commit.
- **The record:** the first changeset of `B` (or its branch-point atom, §4) carries the param
  `branch_point_rule = earliest-covering-changeset`, and `branch_point = exact` or `approximate`.

## 4. The branch tree: what `cvs checkout -r B` gives

- **At the cut,** `B`'s tree has **exactly the files on `B`**, each at its branch-point content. Files not on
  `B` are absent, and so are those whose branch point is dead.
- **The parent's tree can differ,** because:
  - the parent is approximate;
  - or the branch covers only part of the tree (a subdirectory `cvs tag -b`, a common practice);
  - or the parent line holds files that `B` does not carry.
- **When it differs,** emit a **branch-point atom** between the parent and `B`'s first changeset:
  - its ops make the parent's tree into `B`'s tree at the cut (`Delete` for files not on `B`;
    `Add`/`Modify` to the branch-point content);
  - **each op is `Derived(ReconstructedBranch)`**, rule `branch-point-tree`;
  - the atom's status is `Derived(ReconstructedBranch)`, with `branch_point_rule` and `line`;
  - it has **no author, log or time**; it asserts nothing the source did not say;
  - its source atom id is `branch-point:<name>`.
- **When it does not differ,** there is no such atom (the common full-tree, exact case).
- **This is cvs2git's semantics too**: a branch holds only the files tagged on it.
- **After the cut:** `B`'s changesets apply their `Stated` ops to `B`'s own tree (a `dead` revision deletes).
  Trunk changes after the cut never reach `B`: CVS records no merges, and **no merge parents are ever
  inferred** (D-3).

## 5. Refs (`--reconstruct-refs`)

- **A branch ref:** `RefKind::Branch`, `Derived(ReconstructedBranch)`, `source = cvs-symbol`, at the
  branch's **last** atom:
  - its last changeset;
  - or its branch-point atom, when it has no changesets;
  - or the parent, when it has neither (a branch tag with no commits and an identical tree).

  A branch is therefore never "not reconstructed" for lack of revisions.
- **Tags:** feed branch changesets into `rev_to_atom`, so that a tag naming branch revisions resolves by
  today's rule (the changeset of its latest-dated named revision, with `straddle` recorded).
  - "CVS tags on branch revisions not reconstructed" now counts only tags on unnamed or dropped lines.
- **Drop records:** keep exact counts for:
  - unnamed branch revisions (OQ-2);
  - branches whose parent line is not imported (§6);
  - symbols naming a missing revision (§3);
  - vendor-branch revisions (§8).

  Use one record per kind, each with a reason that is true today (none says "planned").

## 6. Nested branches (C-2)

- **A branch cut from a branch revision** (`1.2.4.3` → branch `1.2.4.3.2`) has as its parent line **the line
  of its branch-point revisions**.
- **When the branch points lie on several lines** (a branch cut from a mixed working copy):
  - the parent line is the one holding **most** of them, with ties going to the main line, then to the lower
    symbol name;
  - the files whose branch point lies elsewhere count as not covered, so the branch point is
    **approximate**;
  - the branch-point atom sets their content (§4).
- **Order:** import lines in dependency order: the main line, then branches whose parent line is already
  imported, by symbol name. A branch whose parent line is not imported (unnamed, or itself dropped) is
  dropped and counted, never guessed.
- **`contents_of_many`:**
  - capture branch-point content during **branch** walks too, not only the trunk walk (a nested branch
    point is a branch revision);
  - process branch groups in nesting order;
  - today a non-trunk branch point makes the pass fail over to per-revision `content_of`. That stays correct,
    but it is quadratic. Keep the fallback for malformed files only;
  - extend the existing one-pass vs `content_of` unit test to nested branches.

## 7. Tests

**Real `cvs` fixtures** (RFC 013 "Order and proof"):
- Add `cvs` to CI's Linux x86_64 `apt-get install` line.
- Write the tests with the guard the hg and svn tests use (they skip where the tool is absent, and run on
  ubuntu-latest).
- Build repositories with `cvs init`, `import`, `checkout`, `commit`, `tag -b`, `update -r`, `add` and
  `remove`, driven like the hg helpers.
- Keep the hand-written `,v` tests for the malformed and edge shapes.

| # | Test | C-1 / C-2 |
|---|---|---|
| 1 | Without `--reconstruct-refs`, every existing CVS fixture decodes byte-identically, except the drop-reason text in repositories with branch revisions (show that diff) | C-1 |
| 2 | A full-tree branch with commits: the exact parent, no branch-point atom; each branch atom's tree equals `cvs checkout -r BR` at that point (compare the files) | C-1 |
| 3 | A **subdirectory** branch: a branch-point atom deleting the rest; the tree equals `cvs checkout -r BR` | C-1 |
| 4 | A trunk file added after the cut (not tagged): the parent is before it (earliest covering) | C-1 |
| 5 | A file added on the branch (a dead `1.1` on trunk): not in coverage; added by its branch revision | C-1 |
| 6 | An approximate branch point (tag files one by one with trunk commits between): `ConventionViolation`, a branch-point atom, `branch_point = approximate`, and the tree exact | C-1 |
| 7 | A branch tag with no commits: the ref at the parent (or at the branch-point atom) | C-1 |
| 8 | Unnamed branch revisions (delete the symbol with `cvs tag -d -B`): dropped and counted, as today | C-1 |
| 9 | Branch clustering: two commits within the window on the branch, with the same `(author, log)` as a trunk commit, stay separate from the trunk | C-1 |
| 10 | A nested branch; a branch from a mixed working copy (the branch points on two lines) | C-2 |
| 11 | Tags on branch revisions resolve to branch changesets | C-2 |
| 12 | `contents_of_many` equals `content_of` for every revision, nested branches included (a generated `,v`) | C-2 |
| 13 | The bench: `cvs-revs` and `cvs` corpora with branches added (the generator gains a branch knob). Time and peak before and after, with and without the flag. Time stays near-linear. | C-2 |

**Determinism:**
- Run the decode twice for tests 2, 3, 6 and 10; the outputs are `cmp`-identical.
- `brygge verify --against-source` on each of those artifacts answers `reproduces`.

## 8. Vendor branches — OQ-6 (pending the owner)

- **The design question:** a **literal** branch symbol (`cvs import`'s `1.1.1`, or `-b 1.1.3`) is a vendor
  branch. While it is a file's default branch, its revisions are **main-line** (RFC 007, unchanged).
  - Once cleared in some files but not others, the same symbol's revisions are main-line in some files and
    branch revisions in others.
  - The vendor-import skip also removes its branch point from the op spine.
  - D-3's "once cleared, an ordinary branch" does not settle which line owns those revisions.
- **Recommended for 0.3.0:** vendor branches are **not** reconstructed as lines.
  - Their non-main-line revisions stay dropped, with a precise record: "CVS vendor-branch revisions after
    the vendor branch was cleared (N revisions)", reason "vendor branches are imported as the main line
    while they are the default; their later imports are not reconstructed as a branch".
  - It is revisited when a real repository needs it.
- **Build C-1 on the recommendation.** It touches no main-line behaviour. If the owner rules otherwise, the
  architect amends this section before C-2.

## 9. Docs and CHANGELOG

- **`docs/src/guide/cvs.md`:**
  - branches with `--reconstruct-refs`;
  - the earliest-covering rule and what "approximate" means;
  - the branch tree (only the files tagged on it) and the branch-point atom;
  - no inferred merges;
  - what is still dropped: unnamed branches, vendor branches after clearing, missing-revision symbols.
- **CHANGELOG `[Unreleased]`:**
  - **Added:** CVS branch history with `--reconstruct-refs`.
  - **Breaking:** CVS artifacts decoded with `--reconstruct-refs` from repositories with branches change;
    the drop-reason text changes.
- **Machine output** (`docs/src/reference/machine-output.md`): new flag or drop wording only; no new keys.
  State any change in the request.

## 10. Review requests

- **`.git-exclude/review-request/030-cvs-branches-c1.md`** (§1–§5, tests 1–9).
- **`…/031-cvs-branches-c2.md`** (§6, tests 10–13, the bench).

Commit and push each after its approval, and append CI.

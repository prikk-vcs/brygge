# Handoff — RFC 020 batch S: the shared tree (D-1), and the ceilings re-expressed (D-2) (request 083)

**Governing:** RFC 020 D-1 and D-2. **After:** batch M (request 082), which confirmed that the retained snapshots and
the working tree are the peak: about 96% on codeblocks (P3), and about the whole of it on `svn-tags`.
**On a topic branch,** `batch-s-shared-tree`: it restructures the Subversion decoder's core. The branch is pushed
only after approval, and named.

## 0. Evidence plan first (N-3)

**This is a pure restructuring, so byte-identity is the proof.**
- **Level 1:**
  - the new tree type's operations (insert, remove a subtree, copy a subtree, lookup), each against the flat map's
    answer on the same sequence;
  - and a property test: random operation sequences, with **the iteration of both structures equal at every step**.
- **Level 2:** every existing Subversion test, unchanged.
- **Level 3:**
  - every Subversion corpus entry (wordpress-core, codeblocks), in all three modes, plus every test fixture:
    **byte-identical** to `fixtures/previous-release/0.8.1/brygge`;
  - the scale and time figures of §5.

## 1. The structure (D-1)

- **The types:** `Dir { files: BTreeMap<String, FileEntry>, dirs: BTreeMap<String, Rc<Dir>> }`, and a tree is an
  `Rc<Dir>`.
  - **A snapshot** is `Rc::clone` of the root;
  - **a change** copies the directories on its path with `Rc::make_mut`;
  - **`std` only, with no `unsafe`, no new dependency, and no `Arc`.**
- **The ordering pitfall:** the flat `BTreeMap<String, _>` orders **whole paths** by bytes, but a trie walked by
  component orders `a/…` before `a.c`, while bytes order `a.c` (`.` = 0x2E) before `a/b` (`/` = 0x2F).
  - Anywhere the output depends on iteration order, the result must be the flat order. `IrBuilder` sorts ops and
    copies itself, so find and list every other consumer of tree order in the request (diffs, the copy-record
    order before the builder, `layout_honesty`, `reconstruct_refs`, `--branch-lines`' partition).
  - Then either prove each one order-independent, or iterate in flat order where needed.
  - The level-1 property test must include names around `.`, `-` and `/`.
- **A directory copy** (`apply_dir` with `copyfrom`) inserts the source subtree's `Rc`.
  - **The stated layer still records one `Add` and one `CopyRecord` per file,** as today: the artifact must not
    change. That expansion still walks the copied subtree once, and stays under `max_copy_expansion`.
- **`kept`** holds root `Rc`s. **Also release a kept snapshot after its last use.** The pre-scan already knows every
  `copyfrom` revision. Count the uses, and drop the entry after the last copy that names it, so retention follows
  the copies still to come, not all of history.
- **The diff** that gives a revision's ops walks both trees together and skips `Rc::ptr_eq` subtrees. It produces
  the same ops as today, which the byte-identity proves.
- **`--branch-lines`' per-root trees** use the same type.

## 2. The ceilings (D-2)

- **`max_retained_entries` goes.** It counts full copies, which no longer exist.
- **`max_tree_nodes` replaces it:** the **cumulative** count of directory nodes and file entries allocated by the
  tree, with every `make_mut` copy and every new entry counted. Checked at each allocation.
  - Cumulative, not live, so that no `Drop` accounting is needed. Live is at most cumulative, so the bound holds.
  - **Its default comes from measurement:** at least 10× codeblocks' cumulative count, and not above the memory a
    proportionate default would allow. Report both figures.
- **`max_tree_entries`** is checked where a tree is **expanded into entries**: the stated layer's copy expansion,
  and the per-file walks of `--branch-lines`' diffs. A shared tree's logical size is not counted.
- **`max_copy_expansion` and `max_ir_ops`:** unchanged.
- **The 0.8.1 level-1 and level-2 tests** of the replaced ceiling become `max_tree_nodes` tests, with the same
  discriminating proof.

## 3. The user-facing text

- **`docs/src/guide/svn.md`, "Memory while decoding":**
  - the memory now follows the history's size;
  - the ceilings' new names and their measured defaults;
  - **for a history with many tags,** `--branch-lines` keeps the artifact small, because a tag's creation has no
    ops.
- **`CHANGELOG.md` `[Unreleased]`:**
  - `### Changed`: "The Subversion decoder shares unchanged directories between revisions: a branch or tag copy no
    longer copies the tree, so memory follows the size of the history, not the number of tags times the number of
    files. Every artifact is byte-identical.";
  - `### Breaking`: the ceiling `max_retained_entries` is replaced by `max_tree_nodes` (its message and exit code
    are as before).

## 4. Memory and correctness traps to check, and report

- **No `Rc` cycle** is possible: a copy inserts an older subtree into a newer path, so it is a DAG. Say why in the
  type's doc comment.
- **`Rc::make_mut` on a node still shared with a kept snapshot** must copy, never mutate the snapshot. The level-1
  test: a snapshot's iteration is unchanged after any later change.
- **The deep mutation sweep** covers the decoder. Run it.

## 5. Scale and time (level 3)

- **`svn-tags`:** N = 10, 40, 160, 640 and 2,560, plain and `--branch-lines`. **The peak must grow about linearly in
  N.** Fit it and report the slope. Every N must now complete.
- **codeblocks,** all three modes: the peak and the decode time, each the median of three, against 0.8.1's.
- **The 0.8.1 bombs, re-measured:** each is either refused or decoded, with its peak. Becoming cheap is the intended
  outcome.
- **Stop and report** if:
  - any artifact differs by a byte;
  - codeblocks' median peak or time is worse than 0.8.1's;
  - `svn-tags` is not about linear.

## 6. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- `cargo test --workspace` summed, on stable and 1.87, plus the no-identity run;
- the deep mutation sweep;
- the isolation check, links, the book, the release tools, `cargo deny`.

## 7. Request

- **File** `.git-exclude/review-requests/083-shared-tree.md`, opening with the evidence plan and the order-consumer
  list (§1).
- **Push the topic branch only after approval.** `main` is fast-forwarded on my approval of its CI report.

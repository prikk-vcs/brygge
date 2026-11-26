# RFC 020 — Subversion at real size and shape (0.9.0)

**Status.** **Accepted (2026-11-25)** by the owner: "Yes. Accepted." OQ-1 (as revised after the check against the
owner's philosophy: `--layout-trunk`, `--layout-branches`, `--layout-tags`) and OQ-2 (one container each) are ruled
as recommended. Handoffs: `rfcs/handoffs/020-subversion-at-real-size-and-shape/`. Proposed (2026-11-25) by the
architect. It is scoped in the architect's
review 157. It follows 0.8.1, which bounded the Subversion decoder's memory with ceilings; this RFC removes the cause.

**Done (2026-11-26): implemented in 0.9.0.**

## Summary

Two changes to the Subversion decoder, so that real Subversion repositories reach Git (RFC 019):

- **Size (D-1, D-2):** the decoder's tree shares unchanged directories between revisions. A tag or branch copy then
  costs O(1) and a revision costs O(the paths it changes × depth), instead of a full copy of the repository tree per
  retained revision. **Today, memory grows as tags × files** (157: 513 KB of input reaches 4.4 GB). After this RFC
  it follows the size of the history.
- **Shape (D-3):** the layout the decoder already has becomes three options, `--layout-trunk`, `--layout-branches` and
  `--layout-tags`, so a
  project in a subdirectory, a repository with other names, and a repository with **no** layout at all can be
  decoded with branch lines and encoded to Git. **Today, the last of these cannot reach Git under any option**
  (RFC 019's guide, "Limits in 0.8.0").

The artifacts do not change. Every existing decode is byte-identical, which is the release's main proof.

## Motivation, measured

| What | Today | Where |
|---|---|---|
| a synthetic dump with 2,000 files and N tags | the peak grows ~quadratically: 49 MB at N=10, 366 MB at N=40, 4.4 GB at N=160 | 157 |
| codeblocks (6,221 revisions, 316 copy-source revisions) | ~1.5 GB peak for 640 MB of content; 160 s decode | 152, 076 |
| `kept.insert(number, applied.tree.clone())` | a full clone of the whole tree per copy-source revision | `brygge-decode-svn` `decode.rs:179` |
| `apply_revision(&prev_tree, …)` | returns a new tree each revision, built from the previous one | `tree.rs` |
| the layout policy (`trunk`, `branches`, `tags`) | exists and is recorded in `params["layout"]`, but no option sets it | `options.rs`, `commands.rs:246` |

**Real Subversion repositories of public projects often have hundreds to thousands of tags.** With 0.8.1's ceiling
they are refused. Without it they would exhaust memory. Either way, Subversion → Git, 0.8.0's headline, stops at
the repositories that need it most.

## Decisions

### D-1 — A tree with shared directories

- **The structure:**
  - a directory node, `Dir { files: BTreeMap<name, FileEntry>, dirs: BTreeMap<name, Rc<Dir>> }`;
  - a tree is an `Rc<Dir>` root;
  - **a snapshot is a clone of the root's `Rc`;**
  - a change copies only the directories on its path (`Rc::make_mut`), so every untouched subtree stays shared.
- **A directory copy** (`svn cp <src>@<rev> <dst>`) inserts the source subtree's `Rc` at `<dst>`. That is O(depth),
  not O(files).
- **The diff** a revision's ops come from walks both trees together, and skips any pair of subtrees that are the
  same node (`Rc::ptr_eq`). The cost then follows what changed, not the tree's size.
- **The order and every byte of output are unchanged:**
  - paths are produced in the same canonical order as the flat map gives today;
  - **the stated layer still writes a directory copy as one `Add` per file**, as the IR has always recorded it, so
    artifacts do not change. That expansion still costs one op per file. It stays bounded by `max_copy_expansion`
    and `max_ir_ops`, and the guide says that a tag-heavy repository should use `--branch-lines`, where a copy that
    creates a root has no ops (RFC 019 D-9).
- **No new dependency, and no `unsafe`:** `Rc` and `BTreeMap` from `std`. The decoder stays single-threaded, so there
  is no `Arc`.
- **`--branch-lines`'s per-root trees** use the same structure.

### D-2 — The ceilings, re-expressed in what now costs memory

- **What still costs:** after D-1, a retained snapshot costs one pointer plus the directories that were copied on
  write since. **`max_retained_entries`** (0.8.1) counts full copies and becomes meaningless. It is replaced by
  **`max_tree_nodes`**: the live, distinct directory nodes and file entries the decoder has allocated, checked at each
  allocation.
- **`max_tree_entries`** (one materialized tree) is checked where a tree is **expanded**: the stated layer's copy
  expansion, and the `--branch-lines` diffs. It is not checked on a shared tree's logical size, which no longer costs
  anything to hold.
- **`max_copy_expansion`** and **`max_ir_ops`** stay as they are.
- **The defaults are set by measurement:**
  - the cost per node, and codeblocks' and the corpus's maxima, each with at least 10× margin;
  - **the 0.8.1 bombs:** each is either refused **or decoded within a bounded, measured peak**. Becoming cheap is the
    intended outcome of D-1.

  The threat model (C-2d) is updated.

### D-3 — The layout as options: `--layout-trunk`, `--layout-branches`, `--layout-tags`

- **The concept already has a name in brygge:** the **layout**. It is `params["layout"]`, the layout policy, and the
  refusal "the trunk/branches/tags layout was not found". The options carry that name, one per container:
  - `--layout-trunk <path>`: the trunk directory;
  - `--layout-branches <path>`: the directory whose children are branches;
  - `--layout-tags <path>`: the directory whose children are tags.
- **Each option changes only its own container.** The others keep their standard names (`trunk`, `branches`,
  `tags`).
  - There is no mode in which giving one option silently removes the others. A repository with no `branches`
    directory simply has no branches.
  - So an option can only add what it names. It never drops history the user did not mention.
- **`--layout-trunk .` means the whole repository is the trunk:** one line, `trunk`. The whole repository contains
  every path, so there are no branch or tag containers left. This is part of `.`'s definition, not something
  implied by an omission, and giving `--layout-branches` or `--layout-tags` with it is a usage error. With
  `--branch-lines`, a repository with no layout (the case that refuses today) becomes one line with a ref, and can
  be encoded to Git. The 0.8.0 refusal's advice ("decode without it") becomes "or give `--layout-trunk .`".
- **Refused as a usage error:**
  - an option without `--reconstruct-refs` or `--branch-lines`, since the stated layer has no layout;
  - an absolute path, `..`, an empty segment, or a trailing `/`;
  - any two containers where one is inside another, or two that are the same.
- **What is stated, never implied:** the decode's human report always names the layout it used, all three
  containers, whether given or standard. The artifact records it in `params["layout"]` (extended for the root), and
  `--against-source` re-derives with it. As today, paths outside the layout are flagged (exit 30) and recorded as
  dropped, never silently lost.
- **A project in a subdirectory:** `--layout-trunk oro/trunk --layout-branches oro/branches --layout-tags oro/tags`.
  One project of a multi-project repository is decoded per run.
- **For `git svn` users:** the guide maps `-T`/`--trunk` → `--layout-trunk`, `-b` → `--layout-branches`, `-t` →
  `--layout-tags`, and `-s` → no option (the standard layout is the default). It names the one difference: in
  brygge, an option you do not give keeps its standard name.
- **Branch-lines source ids** (`r<N> <root>`, RFC 019 F-7) use the root's path. For `--layout-trunk .` the root is
  the repository itself, so the id is the plain `r<N>`.

## Non-goals (0.9.0)

- **Merges from `svn:mergeinfo`** (Git merge commits): a new derived layer, and the first candidate for 0.10, with
  its own RFC.
- **Several branches or tags containers** (`git svn` allows repeating `-b` and `-t`): one each in 0.9.0. Only
  raised if a real repository needs it (OQ-2).
- **Any change to what the stated layer records.**

## Evidence (the release's proof)

- **Byte-identity:** every corpus entry, in every Subversion mode, and every test fixture decode to the same bytes
  as 0.8.1 (`fixtures/previous-release/0.8.1/brygge`). For a pure restructuring, this is the proof.
- **Scale:**
  - the synthetic N-tags benchmark grows linearly in N (N = 10, 40, 160, 640, and 2,560), in a new bench scenario;
  - codeblocks' peak is reported against 0.8.1's;
  - the 0.8.1 bombs, re-measured.
- **Time:** codeblocks' decode is reported, using the median of three runs.
- **Layout:**
  - level 1: every rule of D-3;
  - level 3:
    - wordpress-core with explicit standard flags gives bytes identical to the default;
    - a flat repository with `--layout-trunk . --branch-lines`, then `encode git` with the check;
    - a project in a subdirectory: a real multi-project dump, or a level-2 one, said so.

## Threat model

`brygge-03` is updated:
- **C-2d:** the new ceiling and the re-expressed ones;
- **RR-total-memory:** 0.8.1's residual (about 3.3 GB before refusal) closes, or narrows to the measured figure.

## The release plan

| Batch | What |
|---|---|
| **0** | `fixtures.sha256` to 0.8.1 (done, 081) |
| **M** | measure first: where codeblocks' and the synthetic dump's memory and time go, by phase, with `brygge-bench`'s profile mode. It confirms D-1's target before any change |
| **S** | D-1 and D-2 |
| **L** | D-3 |
| **cut** | — |

## Open questions for the owner

- **OQ-1 — The option names (revised 2026-11-25, after checking against the owner's philosophy).**
  - **Recommended:** `--layout-trunk`, `--layout-branches` and `--layout-tags`, each changing only its own container
    (D-3).
  - **Why the first recommendation, `git svn`'s `--trunk`, `--branches` and `--tags`, was withdrawn:**
    - **one name per concept:** brygge already calls this the layout. Three bare container names leave the concept
      unnamed on the command line;
    - **confusion:** `--branches <path>` would sit beside the existing `--branch-lines` mode, two near-identical
      names for different things;
    - **something implied:** `git svn`'s rule ("give any, and the others are absent") turns one option into a
      silent removal of the others.
  - **Why not one `--layout trunk=…,branches=…,tags=…`:** a Subversion path may contain `,` or `=`, so that syntax is
    ambiguous without escaping. That is a new thing to learn, and a new way to be wrong.
  - **The cost of the recommendation:** it is not `git svn`'s spelling, so the guide carries a one-line mapping.
- **OQ-2 — Several branches or tags containers.**
  - **Recommended:** not in 0.9.0. One each covers the standard layout, a renamed one, and a project in a
    subdirectory, and no corpus repository needs more;
  - **the cost of yes:** repeated options, and ref names that must stay distinct across containers.
  - **Checked against the owner's philosophy (2026-11-25):** it holds. A second container's content is outside the
    layout, so it is flagged (exit 30) and recorded as dropped, never silently lost, and deferring is preferred to
    approximating.

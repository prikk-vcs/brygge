# Handoff — 0.2.0, batch B: RFC 010 increments 5 (Git) and 3 (CVS)

**Governing:** RFC 010 as amended 2026-09-24 (increment 5; OQ-B resolved; the plan and its re-ranking by the
batch A baseline). **The baseline:** `tools/bench/README.md`, "0.2.0 baseline (0.1.1)" (review 022).

**The rules for both parts** (RFC 010 D-1, D-2):
- **byte-identical artifacts** for every input;
- no IR, format or dependency change;
- an A/B on the batch A scenarios, same machine, the two builds back to back.

**The parts are independent.** File one review request per part, as each is ready. Part 1 goes first if
you must order them.

---

## Part 1 — increment 5: the Git snapshot retention bound (`brygge-decode-git`)

**The problem, as measured:** `git-commits` peaks at **1.16 GiB for 341 KiB of content** at 20,000 commits,
about 60 KiB per commit, which is one 500-file snapshot per root tree. `decode.rs` keeps
`snap_cache: HashMap<ObjectId, Snapshot>`, keyed by root tree id, and never evicts from it.

**What each snapshot is used for** (`decode.rs:226-262`): for each commit, in parent-first order, the
snapshot of the commit's own tree, and the snapshot of its **first** parent's tree, are diffed. Nothing
else reads the cache.

**Change:**
- **Before the loop, count each root tree's remaining uses.** For every commit in `order`, add +1 for its
  own tree, and +1 for its first parent's tree when it has one. Identical trees share a count: the key is
  the tree id, as the cache's is.
- **In the loop,** after a commit's diff is done, decrement the count of the two trees it used. Remove a
  snapshot from the cache when its count reaches zero.
- A snapshot that is needed again later stays cached, so no tree is walked twice more than today.
- **Nothing else changes:** the traversal order, the diff, rename inference and the output.
- **The bound:** snapshots live only for trees whose remaining users are not all processed. For ordinary
  histories that is the frontier of the parent-first order, so memory tracks O(frontier × tree), not
  O(commits × tree).

**Acceptance:**
- the **`git-commits` A/B** shows the peak no longer growing per commit (a flat, or near-flat, peak
  across 1k, 5k and 20k, near `git-content`'s ~3× content floor plus one snapshot);
- **`git-content` does not rise;**
- **byte-identical** artifacts on every existing Git test fixture, and on the bench corpora (compare the
  artifact bytes from both builds with `cmp`).
- **A unit test:** a history where a tree is reused later (a revert back to an old tree) is diffed
  correctly. The re-reference must not have been evicted early, or must be rebuilt identically.

## Part 2 — increment 3: CVS trunk reconstruction in one pass (`brygge-decode-cvs`)

**The problem, as measured:** `cvs-revs` time is **quadratic** in chain length: 1.4 s, then 34.6 s, then
**920 s** for 200, 1,000 and 5,000 revisions per file. The peak is a constant ~4.5× content. `decode.rs`
calls `f.rcs.content_of(num)` for each main-line revision (`decode.rs:77`), and `content_of` walks the
delta chain from `head` every time (`rcs.rs:97`).

**Change:**
- **Add to `RcsFile`** one method that yields the content of **every main-line revision of the file** in
  one pass (see below). `decode.rs` uses it in place of per-revision `content_of`.
  - **The trunk:** start at `head` (full text), follow `next`, and apply each revision's reverse delta
    once. Each trunk revision's content is produced in turn.
  - **The vendor branch** (while `branch` is set): reconstruct the branch point from the trunk pass's
    result, then apply the branch revisions' forward deltas in order.
  - It yields only the revisions `mainline::is_main_line` admits, and skips none it admits.
  - It keeps the `MAX_CHAIN` bound and the same errors for a malformed chain.
- **Dead revisions** keep today's empty content. The `cvs import` branch-point skip
  (`vendor_import_branch_point`) keeps its comparison, fed from the pass's results rather than two more
  full reconstructions.
- **Memory:** the peak must **not** rise. Either reuse the running line vector and hand out each
  revision's bytes as today (all `FileRev`s are still held for clustering, as now), or measure and state
  what the peak did.
- **`content_of` may stay** for any single-revision use that remains. If none remains, remove it.

**Acceptance:**
- the **`cvs-revs` A/B:** time grows **linearly** in revisions (5,000 per file in seconds, not
  minutes), and the peak is **not higher** at any scale;
- **byte-identical** artifacts on every existing CVS test fixture (vendor import, cleared vendor branch,
  order splits, dead revisions) and on the bench corpora;
- **a unit test** comparing, for a generated `,v` with a long trunk and a vendor branch, the one-pass
  contents against per-revision `content_of`, for every revision.

## Reporting (both parts)

- **The A/B tables in `tools/bench/README.md`,** under "0.2.0 increment 5" and "0.2.0 increment 3": scale,
  peak and time before and after, the IR identical, and the machine.
- **The usual gates:** default toolchain and 1.85, the cross-target clippy, and CI after the push.
- **CHANGELOG `[Unreleased]`:** one line per part (Changed: less memory for long Git histories; much less
  time for long CVS histories; output unchanged).

## Review requests

`.git-exclude/review-request/024-increment-5-git.md` and `…/025-increment-3-cvs.md`. After approval,
commit and push, and append the CI result.

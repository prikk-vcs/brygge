# Handoff — CVS decoder corrections: main line only, honest clustering, content-derived identity

**Governing RFC:** RFC 007 (CVS decoder), Accepted; this handoff inherits that state. It builds on the
IR contract 0.2.0 types (RFC 011, handoff 5).

**Owner rulings applied:**
- **D-2 (2026-09-23):** main line only until branch-aware threading lands (0.3.0). Branch revisions are
  dropped-with-record, **with user guidance** (documentation, a guideline, CLI messages).
- **D-3:** non-UTF-8 paths are refused.

**Corrections carried** (numbered in the architect's intake review; this handoff defines them durably):

| ID | What |
|---|---|
| **CR-01** | Branch revisions (`1.2.2.1`, vendor `1.1.1.x`) are clustered into the one linear spine, so replayed trees contain content the main line never had |
| CR-08 | `repo_id` is the operator's filesystem path; clustering fragments interleaved commits and overstates confidence; the same path in `Attic/` and live; symlinks skipped silently |
| CR-03 (CVS) | Path components converted lossily |
| CR-04 (CVS) | A committer and commit time the source never stated |

**Order:** after handoff 5 is approved. It is independent of the hg, SVN and Git batch-2 handoffs.

---

## 1. Reproduce first (CR-01)

Before changing code, add the failing fixture:
- a hand-written `,v` with trunk `1.1 → 1.2 → 1.3`, and a branch revision `1.2.2.1` dated between `1.2`
  and `1.3` with distinct content;
- assert that replaying the IR puts the branch content into a main-line tree.

Report the observed failure in the review request, then fix it. If it does not reproduce, stop and
report.

## 2. Change scope (`crates/brygge-decode-cvs/`)

### 2.1 Main line only (CR-01, D-2)

**Which revisions a file contributes to the main line:**
- **Trunk revisions:** numbers with exactly two components (`1.1`, `1.2`, `2.1`, …).
- **The default (vendor) branch, while it is set:** if the file's admin section has `branch B;` (the
  vendor branch a `cvs import` sets), then B's revisions (`1.1.1.1`, `1.1.1.2`, …) are what a trunk
  checkout yields, so they belong to the main line. They are ordered after the trunk revision B branches
  from.
  - **Exception:** if B's first revision has content identical to its branch-point revision and the
    same date, as `cvs import` produces, the branch-point revision (`1.1`) contributes no separate op.
    This avoids a spurious modify.
- **Every other revision is a branch revision and is not imported.** That includes revisions of a vendor
  branch whose `branch` field is no longer set, because a later trunk commit cleared it.

**What is recorded for it:**
- One drop record, class `Other`:
  - `what = "CVS branch revisions not imported (N revision(s) on M branch(es))"`;
  - reason: *"brygge 0.1.0 imports the CVS main line only; branch history is planned for a later
    release (0.3.0). Keep the source repository."*
- **M counts distinct branch *symbol names* across all files.** A branch's numbers differ from file to
  file, so numbers cannot identify a branch. Revisions on a branch that has no symbol in its file are
  counted as one further group, "unnamed". If there are any, the `what` says `(… M named branch(es) and
  unnamed branch revisions)`.

**Refs** (`--reconstruct-refs`):
- tags reconstruct as today, but only against main-line changesets;
- **a tag that names no main-line revision** (a vendor release tag once the vendor branch is cleared, or
  a tag on a release branch) is not reconstructed. Record it as drop `what = "CVS tags on branch
  revisions not reconstructed (N)"`, class `Other`, with the same reason. It is never skipped silently.
  *(Added after review 008, R-1.)*
- **branch symbols are not reconstructed.** Record them as drop `what = "CVS branch symbols not
  reconstructed (M)"`, class `Other`, with the same reason.
- **A branch symbol** is one in magic form (second-to-last component `0`, e.g. `1.2.0.2`) **or** a
  literal odd-length number of at least three components (the vendor branch, e.g. `1.1.1`, which RCS
  stores literally). A branch's name is looked up in both forms. *(Added after review 008, R-2.)*

**Faithfulness statement:** append to the CVS text in `crates/brygge/src/commands.rs`
`faithfulness_statement`:
> ` Branch history is not imported in this version; the main line is.`

### 2.2 Clustering you can trust (CR-08.2)

**Replace the sequential greedy pass:**
1. Group revisions by the key `(author, log bytes)`.
2. Within a key, sort by `(date, path, rev)`, then split into clusters wherever the gap between
   consecutive revisions exceeds the `window`.
3. A cluster holds **at most one revision per path.** A repeat starts a new cluster, at the second
   occurrence.
4. **Order clusters** by `(latest date, then the smallest "path@rev" string)`.
5. **Per-file order must hold.** If a cluster would place `f@1.3` before `f@1.2`, the later-numbered
   revision moves into a singleton cluster placed immediately after its predecessor's cluster. Count
   each such split.
6. The spine threads linearly. With main line only, a linear spine is now correct.

**Confidence (rule `span-overlap-v1`, deterministic, recorded):**
- `time_score = 100 − min(100, span × 100 / window)`, where `span` is the cluster's latest date minus its
  earliest.
- `overlap` = how many of the cluster's paths are also touched by another cluster whose date range
  intersects this cluster's range widened by `window` on both sides.
- `confidence = floor(time_score × (paths − overlap/2) / paths)`, computed in integers with the division
  last. **Normative integer form** *(fixed after review 008)*: `time_score × (2·paths − overlap) div
  (2·paths)`, clamped to `0..=100`.
- All date and window arithmetic saturates: a window of `u64::MAX` or an extreme date never wraps.
  *(Added after review 008, R-3.)*
- A split singleton from step 5 has confidence `min(50, its own score)`: its placement is brygge's
  judgment.

**Derivation params** on every changeset atom:

| Param | Value |
|---|---|
| `window_secs` | as today |
| `cluster_keys` | `author,log` |
| `confidence_rule` | `span-overlap-v1` |
| `date_rule` | `latest-per-file` |
| `order_splits` | the atom's split count, `0` normally |

**CLI:** from this handoff on, `verify`'s `derivations` check **requires** `date_rule` and
`confidence_rule` for `ReconstructedChangeset` (`confidence_rule` added after review 008, R-8). Remove the handoff-5 exemption comment.

### 2.3 Identity and claims (CR-08.1, CR-04)

- **`repo_id`:** SHA-256 over the concatenation, in ascending path order, of
  `path ‖ 0x00 ‖ rev ‖ 0x00 ‖ decimal date of rev ‖ 0x00 ‖ author of rev ‖ 0x0A`, where `rev` is the
  file's **lowest-numbered trunk revision** (normally `1.1`), for every file that has one. A repository
  with no trunk revision at all is `Error::Read("no main-line revisions")`. *(Amended after review 008,
  R-6: fingerprinting only `1.1` gave every repository without one the same id.)* It is a content-derived fingerprint, never a filesystem path. The same repository decoded from
  two locations gives identical bytes.
- **Claims:**
  - `author` = the committer login, as a `Text` carrying the **exact bytes** (no lossy conversion), with
    **`email` absent**;
  - `author_time` = the representative date, as `Time { seconds, offset_minutes: Some(0) }` (RCS dates
    are UTC by definition). An unparseable RCS date, or a year outside `1970..=9999`, is
    `Error::Read`: a date is never fabricated *(added after review 008, R-3)*;
  - **`committer` and `commit_time` are absent** (the one-claim rule, RFC 011 D-5);
  - the message is the log, as bytes.

### 2.4 Repository shape (CR-08.3, CR-08.4, CR-03)

Add each of these to `floor.rs`. Each is a `FloorRefusal` (exit 20) with a named reason:

| Refused | `feature` | Message |
|---|---|---|
| the same repo-relative path as both `dir/f,v` and `dir/Attic/f,v` | `path in Attic and live` | names the path, says the repository is inconsistent, and suggests `cvs admin`/manual repair of the source |
| a symlink anywhere under the repository root, file or directory (checked with `symlink_metadata`, never followed) | `symlink in repository` | names the entry; brygge reads only the repository it is given |
| a path component that is not valid UTF-8 | `non-UTF-8 path` | shows invalid bytes as `\xNN`; no lossy conversion anywhere on a path, and `,v` detection is done on bytes |
| a symbol name that is not valid UTF-8 *(added after review 008, R-4)* | `non-UTF-8 symbol name` | shows invalid bytes as `\xNN` |
| a default `branch` set while trunk revisions follow its branch point (`cvs admin -b`) *(added after review 008, R-5)* | `default branch with later trunk` | names the file; its main line is ambiguous |

Feature names follow the crate's kebab-case convention (`path-in-attic-and-live`, …). A present but
unparseable `branch` field is `Error::Read`, not "unset".

### 2.5 Documentation (the D-2 guidance)

- **New `docs/src/guide/cvs.md`** (short: one screen):
  - what 0.1.0 carries: the main line and its per-file history, reconstructed changesets marked
    derived, main-line tags with `--reconstruct-refs`;
  - what it does not carry: branch history and branch symbols;
  - how to see a repository's branches before migrating: `rlog -h file,v`, or `grep -A20 '^symbols'`
    on the `,v` files; a branch symbol's number has `0` as its second-to-last component (`1.2.0.2`), and a vendor branch is
    an odd-length number (`1.1.1`);
  - what to do if you need branch history now: keep the source repository, and wait for 0.3.0.

- **`crates/brygge-decode-cvs/README.md`:** the same facts in three lines, linking the guide.
- **`CHANGELOG.md`:** an entry marked **Breaking** (artifact contents change).

## 3. Non-change scope

- The IR contract. No new field is needed; everything above uses 0.2.0 types.
- The RCS parser.
- Ceilings, and other decoders.

## 4. Required tests

1. **CR-01:** the reproduction fixture (§1) now:
   - imports main-line revisions only;
   - produces trees with no branch content;
   - carries the drop record with correct counts;
   - exits `10`.
2. **Vendor import:**
   - a `cvs import`-shaped file (`1.1` and `1.1.1.1` identical and same-dated, `branch 1.1.1;`) gives
     one `Add`, not an `Add` plus a no-op `Modify`;
   - a later `1.1.1.2` on the still-default branch is imported as a `Modify`;
   - with the `branch` field cleared and a `1.2`, the vendor revisions after `1.1` are dropped and
     counted.
3. **Clustering:**
   - two authors committing interleaved across the same window produce **two whole changesets**, not
     fragments;
   - a cluster overlapping another in paths and time has confidence below its time score;
   - the params carry `confidence_rule` and `date_rule`.
4. **Per-file order:** a skewed date that would reverse `f@1.2`/`f@1.3` produces a split, with
   `order_splits = 1` and the order preserved.
5. **`repo_id`:** decoding the same fixture from two different paths gives byte-identical artifacts.
6. **Claims:** `committer` and `commit_time` are absent; `author_time.offset_minutes == Some(0)`.
7. **Shape:** Attic + live → refused; a symlinked `,v` → refused; a symlinked directory → refused; a
   non-UTF-8 path → refused with `\xNN`.
8. **Refs:** a branch symbol is not reconstructed and is counted; a main-line tag still reconstructs.
9. **Statement:** the CVS faithfulness text contains the branch sentence.
10. **Determinism:** decoding twice gives identical bytes, on a fixture exercising branches, the vendor
    branch and an order split.

## 4a. Prohibited shortcuts

- **No approximation of branch content onto the main line.** If a revision's membership is unclear,
  it is a branch revision, and it is counted.
- **No confidence formula other than the one specified.** A change is a new `confidence_rule` name,
  and it needs the architect's review.

## 5. Security gate

The shape refusals and path handling touch untrusted input, so the architect reviews this against
`brygge-03`. `RR-cvs-reconstruction` is folded at the 0.1.0 revision.

## 6. Review request

File `.git-exclude/review-request/008-cvs-corrections.md` with:
- the standard sections;
- the §1 reproduction output;
- the confidence rule's worked numbers for the interleaved fixture.

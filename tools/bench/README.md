# brygge-bench — decoder memory/time harness (RFC 010 OQ-F)

A dev-only measurement harness. It exists so the streaming increments of **RFC 010** are gated on
**evidence, not intuition** (OQ-A). It is not published and is not part of brygge's shipped surface.

## What it measures, and how

For each synthetic corpus, at increasing scale: **peak resident memory**, **wall time**, and the resulting
IR's size (atoms, distinct blobs, content bytes).

- **Zero new dependencies, zero `unsafe`** — consistent with brygge's posture. Peak memory is read from
  `/proc/self/status` (`VmHWM`, the kernel's process high-water mark) on Linux; on other platforms the
  memory column reads `n/a` and time/IR stats still report.
- **One decode per subprocess.** Each `(scenario, scale)` runs in its own child process, so the high-water
  mark reflects that single decode — not the accumulated peak of the whole run.
- The generated corpus is written to a temp file/dir and dropped before the decode, so the measurement is
  the decoder's own footprint (including the input it re-reads internally — an increment-2 target).

`VmHWM` is page-granular and includes allocator retention, so treat it as a faithful *ceiling* indicator
for before/after comparison on one machine, not a precise heap figure.

## Running

```sh
cargo run -p brygge-bench --release            # the full matrix, as a table
cargo run -p brygge-bench --release -- run svn-revs 5000   # one scenario (machine line)
cargo run -p brygge-bench --release -- corpus git-commits 5000 /some/dir   # write a corpus and stop
python3 -I tools/bench/svn-tags-corpus.py target/release/brygge target/release/brygge-bench \
    10 40 160 640 2560 --branch-lines   # svn-tags' matrix, the real binary, one child per run
```

`corpus` writes a scenario's input and prints its kind and path (`git /some/dir`), so the same input can be decoded by
two builds of the `brygge` CLI and the artifacts compared byte for byte, which is how a byte-identical change is checked.

Scenarios: `svn-revs <n>` (a large fixed tree, one branch copy at r1, then ~n single-file edits — content
is bounded, so peak should track *scratch*), `svn-content <n>` (few revisions, n sizeable files — peak
grows with content, the IR floor), `cvs <n>` (n single-revision `,v` files).

The 0.2.0 scenarios (RFC 010 OQ-A; each one a target of a batch B increment):

| Scenario | Shape | Targets |
|---|---|---|
| `git-commits <n>` | a fixed tree of 500 files; `n` commits, each editing one file | increment 5: the snapshot cache grows with distinct root trees |
| `git-content <n>` | 3 commits; `n` files of 8 KiB | the IR floor for Git: peak should track content, not history |
| `cvs-revs <n>` | 20 `,v` files, each with `n` trunk revisions (each rewrites 3 of 200 lines) | increment 3: the O(revisions²) per-file reconstruction |
| `svn-dump <n>` | an SVN dump of `n` revisions over 300 files of 2 KiB, each revision rewriting one | increment 2: the whole parsed `Dump` held beside the IR |
| `svn-deltas <n>` | the `svn-dump` corpus loaded into a repository and dumped with `svnadmin dump --deltas` (needs `svnadmin`) | RFC 013 D-2: reading svndiff; the IR must equal the fulltext corpus's |
| `svn-checked <n>` | the same, dumped as fulltext by `svnadmin dump`, so every text's MD5 and SHA-1 are stated | RFC 013 D-2: the cost of checking every checksum |
| `cvs-branches <n>` | the `cvs-revs` files plus a branch `BR` of `n/4` revisions cut from the middle of the trunk, and a nested branch `NEST` of `n/8` revisions cut from `BR`'s middle; decoded with `--reconstruct-refs` | RFC 013 C-2: branch history and the one-pass reconstruction of nested branches |
| `cvs-branches-plain <n>` | the same files, decoded without the flag (the trunk alone) | the same corpus with the branches not imported: what the flag adds |

Every one is synthetic and deterministic (the same `n` gives byte-identical corpora), and a **self-check** runs
after each decode: the IR's atoms, blobs and content bytes must equal what the generator computed, and for
`cvs-revs` every revision of every file must have exactly the text the generator intended. A scenario whose
check fails prints `FAILED` and the run exits non-zero. Git corpora are built with `git fast-import` (a `git`
binary is needed to run the harness); nothing else is new.

Only the decode is timed, and the kernel's peak mark is reset (`/proc/self/clear_refs`) after the corpus is
written and before the decode starts, so `peak_KiB` is the decode's own high-water mark. (Large corpora are
streamed to disk while they are generated, so generation adds nothing the decode then has to outgrow.)

## Result — RFC 010 increment 1 (SVN snapshot retention bound)

Measured A/B, same machine, `svn-revs` (a 500-file tree, then n single-file edits). Pre-bound is commit
`251ece1` (a `Vec<Tree>` snapshot per revision); post-bound is `c7ba7bf` (only `copyfrom`-referenced
snapshots retained). **The IR is identical in both** (same atoms/blobs/content bytes) — the bound changes
only scratch, never output:

| revisions | pre-bound peak | post-bound peak | reduction |
|---:|---:|---:|---:|
| 1,000  | ~145 MiB  | ~6 MiB  | ~23× |
| 5,000  | ~708 MiB  | ~19 MiB | ~37× |
| 20,000 | **~2.8 GiB** | **~62 MiB** | **~46×** |

Pre-bound peak grew **O(revisions × tree)** (2.8 GiB at 20k revisions over a 500-file tree); post-bound
tracks the **IR** (O(atoms) — the content it must carry). The reduction ratio widens with scale, exactly as
`O(revisions × tree) / O(revisions)` predicts. This is the RFC 010 finding made concrete: streaming does not
shrink the IR (the IR *is* the content), it removes the scratch that exceeded it.

## Gating increments 2–4 (OQ-A)

Baseline for the remaining targets, from the matrix (this machine; indicative, not a contract):

- `svn-content` peak grows with content and is small relative to `svn-revs` at the same file count — the IR
  floor dominates, as expected; the parsed-dump copy (increment 2) is the next scratch above it to measure.
- `cvs` peak grows with file/changeset count; increment 3 (bounded reconstruction) is measured here before
  it is built.

Re-run this harness before and after each further increment; increment 4 (a streaming artifact *writer*,
which alone touches `brygge-ir`) is undertaken only if a measured ~2×IR peak is shown to bind on a real
large repository (RFC 010 D-3).

## 0.2.0 baseline (0.1.1)

The state of the decoders **before any 0.2.0 increment**, against which each batch B increment is an A/B. Run once,
the full matrix, release build (`cargo run -p brygge-bench --release`, 16 minutes, almost all of it `cvs-revs 5000`).

- **Code:** `main` at `c5abed6`. The decoders and `brygge-ir` are byte-for-byte those of the `0.1.1` tag
  (`git diff 0.1.1 HEAD -- crates` is empty); the harness is the one of this batch.
- **Machine:** AMD Ryzen 9 9950X (16 cores, 32 threads), 59 GiB RAM, Linux 7.2.7, rustc 1.98.1, git 2.55.0. Corpora
  in a tmpfs `/tmp`. The machine was shared with other work (load about 4 at the start), so **times are indicative;
  peaks are the process's own high-water mark and are not affected by other work.** Re-run on the same machine for
  an A/B.
- **All twenty rows passed their self-check.** The IR statistics (atoms, blobs, content bytes) are printed with
  every row so an A/B can show the IR is unchanged.

### `git-commits` (increment 5: the Git snapshot cache)

| commits | peak | time | atoms | blobs | content | peak ÷ atoms |
|---:|---:|---:|---:|---:|---:|---:|
| 1,000 | 62 MiB | 171 ms | 1,001 | 1,500 | 20 KiB | 63 KiB |
| 5,000 | 294 MiB | 862 ms | 5,001 | 5,500 | 86 KiB | 60 KiB |
| 20,000 | 1,160 MiB | 3.5 s | 20,001 | 20,500 | 341 KiB | 59 KiB |

**Peak grows far faster than content, and linearly with commits**: 1.1 GiB at 20,000 commits for 341 KiB of content,
a steady ~60 KiB per commit, which is one 500-file snapshot per distinct root tree (O(commits × tree)). This is the
evidence increment 5 must remove: after it the peak should track the IR, not commits × tree.

### `git-content` (the Git IR floor)

| files | peak | time | atoms | blobs | content | peak ÷ content |
|---:|---:|---:|---:|---:|---:|---:|
| 1,000 | 27 MiB | 40 ms | 3 | 1,101 | 8.6 MiB | 3.2× |
| 5,000 | 122 MiB | 200 ms | 3 | 5,501 | 43.0 MiB | 2.8× |

Peak tracks content (about 3× it, falling with scale): the floor the increment must not push above. Nothing here for a
bound to remove.

### `cvs-revs` (increment 3: CVS delta chains)

| revisions per file (20 files) | peak | time | atoms | blobs | content | peak ÷ content |
|---:|---:|---:|---:|---:|---:|---:|
| 200 | 92 MiB | 1.4 s | 4,000 | 4,000 | 19.8 MiB | 4.6× |
| 1,000 | 456 MiB | 34.6 s | 20,000 | 20,000 | 102.0 MiB | 4.5× |
| 5,000 | 2,286 MiB | 920.1 s | 100,000 | 100,000 | 528.3 MiB | 4.3× |

**Time is quadratic in the chain length and peak is not**: 5× the revisions costs 24× then 27× the time (1.4 s → 34.6 s →
920 s), the per-file walk from `head` for every revision; the peak stays a constant ~4.5× the content. So increment 3's
evidence is **time** (this column), not peak; whether it also lowers the peak (fewer live `FileRev` texts) is what the
A/B will show.

### `svn-dump` (increment 2: the parsed dump held beside the IR)

| revisions | peak | time | atoms | blobs | content | peak ÷ content |
|---:|---:|---:|---:|---:|---:|---:|
| 1,000 | 14 MiB | 25 ms | 1,003 | 1,300 | 2.5 MiB | 5.3× |
| 5,000 | 49 MiB | 130 ms | 5,003 | 5,300 | 10.4 MiB | 4.7× |
| 20,000 | 184 MiB | 526 ms | 20,003 | 20,300 | 39.6 MiB | 4.6× |

Peak is a steady ~4.6× the content and above the ~3× floor `git-content` shows: the difference is the dump held in
memory beside the IR, which increment 2 targets. It does not grow *faster* than content; it is a constant multiple of
it, which is why it matters at the largest dumps.

### The older scenarios, on the same run

| scenario | scale | peak | time | atoms | blobs | content |
|---|---:|---:|---:|---:|---:|---:|
| `svn-revs` | 1,000 | 8.2 MiB | 77 ms | 1,003 | 1,500 | 13 KiB |
| `svn-revs` | 5,000 | 26 MiB | 384 ms | 5,003 | 5,500 | 53 KiB |
| `svn-revs` | 20,000 | 87 MiB | 1.6 s | 20,003 | 20,500 | 209 KiB |
| `svn-content` | 200 / 1,000 / 4,000 | 3.8 / 4.7 / 8.0 MiB | 0–3 ms | 2 | 1 | 256 B |
| `cvs` | 200 | 5.4 MiB | 1 ms | 200 | 200 | 3 KiB |
| `cvs` | 1,000 | 12 MiB | 12 ms | 1,000 | 1,000 | 16 KiB |
| `cvs` | 4,000 | 37 MiB | 85 ms | 4,000 | 4,000 | 69 KiB |

`svn-revs` is increment 1's scenario: 87 MiB at 20,000 revisions is ~4 KiB per atom, and no longer O(revisions × tree).
(`svn-content` writes one identical 256-byte body many times, so it has a single blob; it is kept as it was.) Their
numbers are from this run's methodology (decode only, peak reset), which can differ slightly from the increment-1
table above; that table is unchanged.

## 0.2.0 increment 5: the Git snapshot retention bound

RFC 010 increment 5: `brygge-decode-git` keeps a snapshot of a root tree only until the last commit that will use it
(its own tree, or its first parent's) has been processed. A/B, **the two builds back to back on the same machine and the
same corpora**: "before" is `72f42a2` (the Git decoder is unchanged since 0.1.1), "after" is the increment.

| scenario | scale | peak before | peak after | time before | time after | atoms | blobs | content |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `git-commits` | 1,000 | 61.8 MiB | 8.4 MiB (7.3x lower) | 168 ms | 152 ms | 1,001 | 1,500 | 20 KiB |
| `git-commits` | 5,000 | 293.5 MiB | 27.0 MiB (10.9x) | 848 ms | 751 ms | 5,001 | 5,500 | 86 KiB |
| `git-commits` | 20,000 | 1,159 MiB | 98.8 MiB (**11.7x**) | 3.6 s | 3.1 s | 20,001 | 20,500 | 341 KiB |
| `git-content` | 1,000 | 27.1 MiB | 26.8 MiB | 40 ms | 40 ms | 3 | 1,101 | 8.6 MiB |
| `git-content` | 5,000 | 121.4 MiB | 119.3 MiB | 201 ms | 199 ms | 3 | 5,501 | 43.0 MiB |

**The IR is identical** (atoms, blobs and content bytes match in every row, and the self-checks pass); **the artifacts are byte
for byte identical**: 9 repositories x {plain, `--infer-renames`} = 18 comparisons of the CLI's artifact, stdout and exit code
between the two builds, all identical. The nine: five history shapes (branches and a merge with a stash and a tag; a tree
reverted to an earlier one, twice, with an empty commit; an octopus merge with a second root; criss-cross merges; renames and a
copy), a 61-commit wide tree, and the bench's `git-commits` 5,000 and 20,000 and `git-content` 1,000 corpora.

**What the result says.** The growth per commit fell from **~59 KiB to ~5 KiB**: the O(commits x tree) snapshot term is gone.
The peak is still not flat: it now tracks the IR, at ~5 KiB per atom at 20,000 commits, which is what `svn-revs` (the same
shape, an SVN history over a 500-file tree) reaches after increment 1 (87 MiB at 20,000 revisions in the baseline; 99 MiB here,
the difference being Git's own maps of commits and refs). `git-content` does not rise (it is 1% lower). Time is not worse: 9%
to 14% lower on the large runs, on a machine that was busy (load 10 to 30), so treat the times as indicative.

## 0.2.0 increment 3: CVS trunk reconstruction in one pass

RFC 010 increment 3: `brygge-decode-cvs` reconstructs every main-line revision of a file in **one pass** down its delta
chain (`RcsFile::contents_of_many`), where it walked from `head` once per revision. A/B, back to back, same corpora; "before"
is `72f42a2`, "after" is the increment. One more change rides with it: a reconstructed revision's bytes are allocated at their
exact size (growth slack was being kept for the whole decode).

| scenario | revisions per file (20 files) | peak before | peak after | time before | time after | atoms | blobs | content |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `cvs-revs` | 200 | 91.4 MiB | 87.1 MiB | 1.4 s | **84 ms** (17x) | 4,000 | 4,000 | 19.8 MiB |
| `cvs-revs` | 1,000 | 455.8 MiB | 435.8 MiB | 35.8 s | **0.55 s** (66x) | 20,000 | 20,000 | 102.0 MiB |
| `cvs-revs` | 5,000 | 2,284 MiB | 2,211 MiB | 1,098 s (18 min) | **5.6 s** (197x) | 100,000 | 100,000 | 528.3 MiB |

(The "before" at 5,000 took 920 s in the baseline above and 1,098 s in this A/B: the machine was busier now, load 20 to 30. The
"after" numbers were taken at load 14 to 16.) **The peak is not higher at any scale: it is 3% to 5% lower** (the exact-size
allocation).

**The IR is identical and so are the artifacts:** 30 comparisons of the CLI's artifact, stdout and exit code between the two
builds (15 repositories x {plain, `--reconstruct-refs`}), all identical. The fifteen: the existing test fixtures (a
`cvs import`, a cleared vendor branch, an order split, a branch revision, a vendor import with no trunk), a dead middle
revision, a dead head, all of them together in one repository, **three malformed files** (an unreachable revision, an unknown
diff command, a bad diff count: the same error text and exit code from both builds), and the bench's `cvs` and `cvs-revs`
corpora.

**Time: from quadratic to nearly linear, with a second cost now visible.** The reconstruction itself is linear (0.6 s of the
5.6 s at 5,000 revisions per file, from an instrumented run that is not committed). What remains grows faster than linearly:
**`cluster::reconstruct` takes 4.0 s at 100,000 revisions** (53 ms at 10,000, 195 ms at 20,000, 948 ms at 40,000: about 4x per
doubling). From reading the code, the likely cause is `finalize`, which scores each changeset by scanning every other changeset's range
for each of its paths, O(changesets squared); I timed the whole function, not that step. It was hidden behind the
reconstruction and is now the larger part. It is outside this increment (it is not reconstruction); see the review request.

## 0.2.0 increment 3b: the CVS changeset overlap, indexed per path

RFC 010 increment 3b: the confidence rule `span-overlap-v1` counts, for each changeset, its paths that another changeset
touches within a window; `cluster::finalize` scanned every changeset's date range for each path (O(changesets squared x paths)).
It is now answered from a per-path index (each path's changesets sorted by earliest date, with the two largest latest dates per
prefix, so a binary search answers the same predicate). A/B, back to back: "before" is increment 3 (`f0550ed`), "after" is 3b.

| scenario | revisions per file (20 files) | peak before | peak after | time before | time after | atoms | blobs | content |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `cvs-revs` | 200 | 87.1 MiB | 88.0 MiB | 84 ms | 79 ms | 4,000 | 4,000 | 19.8 MiB |
| `cvs-revs` | 1,000 | 434.7 MiB | 434.9 MiB | 0.55 s | 0.44 s | 20,000 | 20,000 | 102.0 MiB |
| `cvs-revs` | 5,000 | 2,208.2 MiB | 2,210.2 MiB | 5.7 s | **2.4 s** | 100,000 | 100,000 | 528.3 MiB |

The IR is identical (each row's atoms, blobs and content bytes match; the self-check, which compares every revision's text, passes).
The peak is **not lower**: it is 0.04% to 1% higher (0.2 to 2 MB), the per-path index (about 56 bytes per changeset per path).

**The clustering alone, from an instrumented run (not committed), by total revisions:**

| revisions | before 3b | after 3b |
|---:|---:|---:|
| 10,000 | 53 ms | 29 ms |
| 20,000 | 195 ms | 66 ms |
| 40,000 | 948 ms | 187 ms |
| 100,000 | 4,033 ms | **488 ms** (8.3x) |

Growth is now about 2.3x to 2.8x per doubling (a sort and an index build: n log n, plus allocation), where it was 4x. **The
whole decode now grows linearly to within the cost of a log:** 79 ms, 0.44 s, 2.4 s for 5x the revisions each time (5.6x, then
5.4x), where it was 6.5x then 10x. At 100,000 revisions the phases are reconstruction 0.57 s, clustering 0.49 s, atoms 0.74 s;
the rest is reading and parsing the `,v` files and finishing the IR.

**The artifacts are byte-identical:** 40 comparisons of the CLI's artifact, stdout and exit code between the two builds (20
repositories x {plain, `--reconstruct-refs`}), all identical: the crate's fixtures and malformed files, three repositories built to
stress the overlap (a few (author, log) pairs with bursts of near-equal dates, changesets with identical dates, and widely spread
dates: their confidences range over 0 to 100), and the `cvs` and `cvs-revs` corpora up to 5,000 revisions per file.

## Result — RFC 013 C-2 (CVS branch history, nested branches)

`cvs-branches` and `cvs-branches-plain`, 20 files, `n` trunk revisions each, one `BR` of `n/4` and one nested `NEST` of `n/8`
revisions per file. Every revision is dated an hour from every other, so it is one atom (no clustering), the parents are exact
and every branch's tree is its parent's, so there is no branch-point atom: **atoms = 20 (n + n/4 + n/8)**, and the self-check
compares the atoms, blobs and content bytes with the generator's. Release build, one machine, one decode per process.

| scenario | revisions per file | peak | time | atoms | time per atom |
|---|---:|---:|---:|---:|---:|
| `cvs-branches-plain` | 200 | 89 MiB | 92 ms | 4,000 | 23 us |
| `cvs-branches` | 200 | 120 MiB | 132 ms | 5,500 | 24 us |
| `cvs-branches-plain` | 1,000 | 442 MiB | 517 ms | 20,000 | 26 us |
| `cvs-branches` | 1,000 | 595 MiB | 640 ms | 27,500 | 23 us |
| `cvs-branches-plain` | 5,000 | 2,236 MiB | 3.0 s | 100,000 | 30 us |
| `cvs-branches` | 5,000 | 3,030 MiB | 3.4 s | 137,500 | 25 us |
| `cvs-branches-plain` | 10,000 | 4,484 MiB | 6.0 s | 200,000 | 30 us |
| `cvs-branches` | 10,000 | 6,090 MiB | 8.8 s | 275,000 | 32 us |

**Near-linear**: 5x the revisions cost 5.6x (plain) and 4.8x (with branches) in time from 200 to 1,000, then 5.9x and 5.3x from
1,000 to 5,000; the time per atom stays between 23 and 32 microseconds, the same as the trunk alone (the peak is the IR's content,
as in `cvs-revs`). The nested branch adds no quadratic term: `contents_of_many` walks every line once (a branch point on a branch is
captured during the parent branch's walk), where a per-revision `content_of` would walk the whole chain for each.

**No regression without the flag, and none for single-level branches with it.** Against the C-1 build (`7665c0d`), the same
corpora: `cvs-revs` and `cvs-branches-plain` (trunk alone), 0.09 to 6 s, are the same as before within the run-to-run noise (about
10 %; three interleaved pairs of `cvs-revs 10,000`: 5.4 s now against 5.9 to 6.1 s before), and the CLI's artifact, stdout and exit code are **byte-identical** on the `cvs` and `cvs-revs` corpora, on `cvs-branches` without the flag,
and on the `cvs-branches` files with the `NEST` symbol removed (single-level branches, with the flag). Only `cvs-branches` with the
flag differs, by design: C-1 counts `NEST` as not imported (5,000 atoms at 200), C-2 imports it (5,500).

## Result — RFC 013 D-2 (Subversion delta dumps, and checksums checked)

`svn-checked` and `svn-deltas` are the `svn-dump` corpus (n revisions over 300 files of 2 KiB, one rewritten each) made
into a real `svnadmin` dump, which states every text's MD5 and SHA-1 (the older scenarios' hand-generated dumps state none,
so they cannot show the cost of checking). Release build, one machine, one decode per process. **Before** is the
decoder at `c854692` (which reads fulltext only and checks nothing); **after** is this change. The IR is identical in every
row (the self-check compares atoms, blobs and content bytes with the generator's).

| scenario | revisions | peak before | peak after | time before | time after | atoms | content |
|---|---:|---:|---:|---:|---:|---:|---:|
| `svn-checked` (fulltext, checksums stated) | 1,000 | 14.3 MiB | 12.0 MiB | 59-120 ms | 66 ms | 1,003 | 2.5 MiB |
| `svn-checked` | 5,000 | 51.6 MiB | 41.1 MiB | 262-276 ms | 320-327 ms | 5,003 | 10.4 MiB |
| `svn-checked` | 20,000 | 186.7 MiB | 140.1 MiB | 1.04 s | 1.24-1.25 s | 20,003 | 39.6 MiB |
| `svn-checked` | 50,000 | 453.7 MiB | 339.8 MiB | 2.8-4.0 s | 3.4-4.3 s | 50,003 | 98.2 MiB |
| `svn-deltas` (`--deltas`) | 1,000 | n/a (refused) | 12.0 MiB | n/a | 93 ms | 1,003 | 2.5 MiB |
| `svn-deltas` | 5,000 | n/a | 40.4 MiB | n/a | 369 ms | 5,003 | 10.4 MiB |
| `svn-deltas` | 20,000 | n/a | 139.5 MiB | n/a | 1.54 s | 20,003 | 39.6 MiB |
| `svn-deltas` | 50,000 | n/a | 337.7 MiB | n/a | 3.2 s | 50,003 | 98.2 MiB |

- **Checking every checksum costs about 20 % of the time** at 20,000 revisions (1.04 s to 1.24 s: MD5 and SHA-1 over 41.5 MB of
  text; it is linear in the content), and the delta form about 25 % on top of the fulltext (1.54 s).
- **The peak is lower**, by about a quarter: the raw dump is dropped as soon as it is parsed, where it used to be held for the
  whole decode.
- The older scenarios (`svn-dump`, `svn-revs`, `svn-content`; their dumps state no checksum) are unchanged within the run-to-run
  noise (`svn-dump 20,000`: 1.25-1.55 s before, 1.34-1.44 s after; `svn-revs 20,000`: 4.3-4.4 s before, 3.9-5.2 s after); their
  peak is 5-25 % lower for the same reason.

## RFC 014 D-4: real repositories

Every repository in `tools/corpus/real.toml` (review 066's amended R-1 list), fetched or dumped once into
the project's own git-ignored `fixtures/corpus` (review 111 N-1, superseding RFC 014 D-4's "always outside
this repository" — `tools/fixtures.sh fetch` does the fetching, `tools/fixtures.sh verify` checks every
pin/hash with no network), decoded with every variant its kind supports, verified through the real `brygge`
binary (internally, and `--against-source` against the fetched repository itself), and summarized into a
shape — all via `brygge-bench real tools/corpus/real.toml fixtures/corpus`. **One decode per subprocess**
(`real-decode-one`, spawned by `real` per entry/variant), the same method the synthetic scenarios above
use, so each peak is that one decode's own high-water mark and not a running total across every repository
this process has already decoded.

**brygge `0.3.1` @ `09fe12d`. Machine: Linux 7.2.10-1-cachyos x86_64, AMD Ryzen 9 9950X (32 threads), release
build.** `Peak÷content` is the peak RSS divided by the IR's own content bytes, so a large peak can be told
apart from "a multiple of content, as designed" (the synthetic Git scenarios run about 2.8×) versus a real
problem.

**"Peak" below is the decode alone (P1 in RFC 015 D-1's sense) — `brygge decode`, the command a user
actually runs, peaks higher** (P3: about 4× the content before batch W, per "RFC 015 D-1: where the peak
goes"; fish-shell's P1 here reads 9.6 GiB, its P3 is 18.7 GiB). Review 074 found this after this table was
first written; not re-run here since D-1's own table already has the real-corpus P1-P6 figures for
`fish-shell` and `tortoisehg-thg`, and amending every row's exit/verify/shape data for the rest would mean
re-running the whole corpus for a label's sake.

| Repository | Pin | Variant | Exit | Time | Peak (decode only, P1) | Content | Artifact | Peak÷content | Atoms | Files/commit (min/med/p90/max) | Tree (start/med/max/end) | Dropped | Flagged | verify | against-source |
|---|---|---|---|---:|---:|---:|---:|---:|---:|---|---|---:|---:|---|---|
| `jq` (git) | `34f7186b` | default | **refused** | — | — | — | — | — | — | — | — | — | — | — | — |
| `jq` (git) | `34f7186b` | infer_renames | **refused** | — | — | — | — | — | — | — | — | — | — | — | — |
| `fish-shell` (git) | `6cf31373` | default | ok | 35.9 s | 9,624 MiB | 4,619 MiB | 4,861,138,620 B | 2.08× | 23,496 | 0/1/5/597 | 93/1,595/2,261/2,261 | 6 | 0 | pass | corresponds |
| `fish-shell` (git) | `6cf31373` | infer_renames | ok | 36.1 s | 9,623 MiB | 4,619 MiB | 4,861,397,555 B | 2.08× | 23,496 | 0/1/5/597 | 93/1,595/2,261/2,261 | 6 | 0 | pass | corresponds |
| `eza` (git) | `98442ab1` | default | ok | 937 ms | 183 MiB | 73.1 MiB | 79,900,871 B | 2.51× | 3,374 | 0/1/7/686 | 4/228/1,124/735 | 6 | 0 | pass | corresponds |
| `eza` (git) | `98442ab1` | infer_renames | ok | 931 ms | 185 MiB | 73.1 MiB | 80,051,113 B | 2.53× | 3,374 | 0/1/7/686 | 4/228/1,124/735 | 6 | 0 | pass | corresponds |
| `tortoisehg-thg` (hg) | `ff01aa72ea78` | default | ok | 64.1 s | 3,487 MiB | 1,666 MiB | 1,757,988,803 B | 2.09× | 20,596 | 0/1/3/300 | 26/634/767/747 | 4 | 0 | pass | corresponds |
| `evolve` (hg) | `bcdd16c129b3` | default | ok | 15.3 s | 787 MiB | 364 MiB | 388,063,826 B | 2.16× | 7,536 | 0/1/10/147 | 10/232/357/357 | 4 | 0 | pass | corresponds |
| `openbsd-games` (cvs) | 2026-10-20 | default | ok | 445 ms | 599 MiB | 185 MiB | 195,630,354 B | 3.23× | 1,428 | 1/1/8/642 | 642/631/675/583 | 3 | 1 | pass | reproduces |
| `openbsd-games` (cvs) | 2026-10-20 | reconstruct_refs | ok | 557 ms | 635 MiB | 185 MiB | 195,651,740 B | 3.43× | 1,429 | 1/1/8/642 | 642/631/675/583 | 6 | 2 | pass | reproduces |
| `openbsd-cpio` (cvs) | 2026-10-20 | default | ok | 3 ms | 6.9 MiB | 0.7 MiB | 719,690 B | 10.2× | 7 | 1/1/45/45 | 45/45/45/0 | 3 | 0 | pass | reproduces |
| `openbsd-cpio` (cvs) | 2026-10-20 | reconstruct_refs | ok | 5 ms | 7.1 MiB | 0.7 MiB | 721,053 B | 10.6× | 7 | 1/1/45/45 | 45/45/45/0 | 4 | 1 | pass | reproduces |
| `openbsd-libpcap` (cvs) | 2026-10-20 | default | ok | 30 ms | 33.0 MiB | 8.6 MiB | 9,194,825 B | 3.83× | 234 | 1/1/5/23 | 21/25/26/26 | 3 | 0 | pass | reproduces |
| `openbsd-libpcap` (cvs) | 2026-10-20 | reconstruct_refs | ok | 35 ms | 34.7 MiB | 8.6 MiB | 9,214,100 B | 4.03× | 234 | 1/1/5/23 | 21/25/26/26 | 4 | 0 | pass | reproduces |
| `wordpress-core` (svn) | r2700 | default | ok | 321 ms | 185 MiB | 78.2 MiB | 83,344,922 B | 2.36× | 2,701 | 0/1/4/163 | 0/149/1,141/1,141 | 4 | 0 | pass | corresponds |
| `wordpress-core` (svn) | r2700 | reconstruct_refs | ok | 321 ms | 185 MiB | 78.2 MiB | 83,346,399 B | 2.37× | 2,701 | 0/1/4/163 | 0/149/1,141/1,141 | 4 | 0 | pass | corresponds |

**Reproducibility:** a second run from the same cache (`fish-shell`, `openbsd-games`, `wordpress-core`,
spot-checked via two direct `brygge decode` invocations each) gives byte-identical artifacts (`cmp`),
confirmed before writing this table.

**The zstd revlog path (review 066/068, F-2):** this machine's Mercurial (7.2.4) clones with
`revlog-compression-zstd` **by default** — `evolve`'s cached clone above is already in zstd form (its
revlog chunks start `28 b5 2f fd`), inverting the handoff's original assumption that a clone needed
upgrading *to* zstd. To get the comparison review 066 asked for, a second clone was made with
`--config format.revlog-compression=zlib` (chunks start `78 9c`, confirmed zlib) and decoded separately
(not run through `real.toml`, since the generic fetch path has no per-clone compression override — this
was a one-off, by hand). **The two decodes produced byte-identical artifacts** (`cmp`), confirming the
revlog's own compression form leaves no trace in the IR, on real history (not just the synthetic `0344`
fixture), at `evolve`'s real scale.

### Findings (reported, not fixed in this batch)

1. **`jq` is refused outright, on both variants:** "refused Git feature 'submodule' below the floor:
   submodule (gitlink) at `modules/oniguruma` points outside this repository; refused rather than
   approximated." This is C-2a's own stated behaviour (refuse rather than misread), and the owner-ratified
   floor (FA-3, "for Git, refuse, never approximate") working as designed — kept as the row demonstrating
   it. `eza` (`eza-community/eza`, tag `v0.23.5`) was added as the corpus's decodable Git sample instead:
   689 merges, both annotated (76) and lightweight (20) tags, confirmed real renames (`R100`), no
   submodules at the pin (no `.gitmodules`, no mode-`160000` tree entries), EUPL-1.2, mid-size (3,374
   atoms). The architect records Git submodule support (carrying a gitlink as a stated pointer, with a
   drop record for what it points to) as a candidate for a later RFC — a floor change, for the owner to
   decide, not part of 0.4.0.
2. **Memory at real scale: fish-shell (23,496 commits, ~2,300-file tree) peaks at 9.6 GiB; TortoiseHg
   (20,596 changesets) at 3.5 GiB.** RFC 010 deferred its streaming increment "until a real import is
   memory-bound" — a mid-size, real project needing ~10 GiB is memory-bound on an ordinary 8–16 GiB
   machine. The architect plans this as the next cycle's theme after 0.4.0; the peak÷content column above
   (2.08–2.53× for Git, close to the synthetic baseline's ~2.8×) is its first input.
3. **The real-world oracle revealed a genuine measurement bug in this harness, not in brygge:** the first
   run of `real` decoded every repository **in one process**, and `/proc/self/status`'s `VmHWM` reset
   (`echo 5 > clear_refs`) did not fully undo the allocator's retained memory from a prior, larger decode in
   the same process — `evolve`'s peak read **9,317 MiB** right after `fish-shell`'s ~9.4 GiB decode in the
   same process, and **787 MiB** (12x smaller) once isolated into its own subprocess, matching the
   synthetic scenarios' already-established "one decode per subprocess" method. The table above is from the
   corrected, isolated run; `real.rs` and `main.rs` were changed to always spawn `real-decode-one` per
   (entry, variant), closing the gap for any future entry too, not just the ones that happened to expose it
   here.
4. **This session also found and corrected a stale hash:** `wordpress-core`'s SVN dump hash in
   `real.toml`, as left by the session that was interrupted mid-work, did not match a freshly reproduced
   `svnrdump dump https://core.svn.wordpress.org -r 0:2700` (no cached dump file existed on disk to check
   it against). Re-dumped and re-hashed; the corrected hash is what's in `real.toml` now and what this run
   verified against.

## RFC 015 D-1: where the peak goes

**Measurement only — no change to any published crate** (the handoff's own scope). `brygge-bench profile
<out-dir> <real-corpus-cache-dir>` runs six phases (P1 decode only, P2 decode+serialize, P3 the full CLI
decode, P4 `verify`, P5 `inspect`, P6 `verify --against-source`) against `fish-shell` and `tortoisehg-thg`
(the real corpus's two largest) and the largest synthetic scenario per kind that has one
(`git-commits 20000`, `cvs-revs 5000`, `svn-dump 20000`; **no hg equivalent exists** — see the finding
below). **Every phase runs in its own process** (`profile-one` for P1/P2, the real `brygge` binary for
P3-P6), **polled from outside** by the orchestrator (`/proc/<pid>/status`'s `VmHWM`, `/proc/<pid>/statm`
for the timeline, every 50 ms) — the same reason `real.rs`'s decodes do, and for the same cause this batch
found there too: a process that measures itself can have an earlier step's allocator retention inflate a
later step's own reading of its own peak. Here it's stronger still, since P1 and P2 genuinely are
different processes (profile-one re-invoked), never sharing an address space at all.

**brygge `0.4.0` @ `799b025`. Machine: Linux 7.2.10-1-cachyos x86_64, AMD Ryzen 9 9950X (32 threads), 59
GiB RAM, release build. Output filesystem: `/home` (btrfs on SSD, 158 GiB free at the time of the run) —
the real-corpus source clones stay on the machine's tmpfs cache from batch R; artifacts and the spill test
write to btrfs, so file-write costs reflect a real disk, not RAM.**

### The attribution table

| Target | Atoms | Content | Metadata | Artifact | Meta/atom | P1 | P2 | P3 | P4 | P5 | P6 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `fish-shell` (git) | 23,496 | 4,619 MiB | 15.2 MiB | 4,636 MiB | 680 B | 9,620 MiB | 18,671 MiB | 18,676 MiB | 9,325 MiB | 9,326 MiB | 18,932 MiB |
| `tortoisehg-thg` (hg) | 20,596 | 1,666 MiB | 9.4 MiB | 1,676 MiB | 481 B | 3,485 MiB | 6,812 MiB | 6,813 MiB | 3,395 MiB | 3,395 MiB | 6,868 MiB |
| `git-commits` (synthetic, 20,000) | 20,001 | 0.33 MiB | 5.6 MiB | 6.6 MiB | 296 B | 99.9 MiB | 101.0 MiB | 100.0 MiB | 37.9 MiB | 37.8 MiB | 129.6 MiB |
| `cvs-revs` (synthetic, 5,000×20 files) | 100,000 | 528 MiB | 36.0 MiB | 567 MiB | 377 B | 2,215 MiB | 3,291 MiB | 3,292 MiB | 1,344 MiB | 1,344 MiB | 3,513 MiB |
| `svn-dump` (synthetic, 20,000) | 20,003 | 39.6 MiB | 4.2 MiB | 44.5 MiB | 219 B | 139.5 MiB | 198.6 MiB | 243.4 MiB | 112.8 MiB | 112.8 MiB | 232.5 MiB |

### The derived holders (handoff §2)

`decoder's working set` subtracts content and metadata from P1; `serialization buffers` is P2−P1; `the
file write` is P3−P2; `the reader's copy` is P4 (same order as P5). All in MiB.

| Target | Content store | Decoder working set (P1−content−metadata) | Serialization buffers (P2−P1) | File write (P3−P2) | Reader's copy (P4) | Peak÷content (P1 / P6) |
|---|---:|---:|---:|---:|---:|---|
| `fish-shell` | 4,619 | **4,986** | 9,052 | 5 | 9,325 | 2.08× / 4.10× |
| `tortoisehg-thg` | 1,666 | **1,809** | 3,328 | 1 | 3,395 | 2.09× / 4.12× |
| `git-commits` | 0.33 | **93.9** | 1.1 | **−1.0** | 37.9 | 300× / 389× |
| `cvs-revs` | 528 | **1,651** | 1,076 | 1 | 1,344 | 4.20× / 6.65× |
| `svn-dump` | 39.6 | **95.7** | 59.0 | **44.8** | 112.8 | 3.52× / 5.87× |

### What does not add up (reported plainly, as asked)

1. **The decoder's working set is sometimes *larger* than the content itself** — `fish-shell`'s is 4,986
   MiB against 4,619 MiB of content, and `tortoisehg-thg`'s is 1,809 against 1,666. This is **not
   metadata's own in-memory size**: `metadata_bytes` is `encode_metadata().len()`, the *serialized*
   record (brygge-ir has no public way to ask for the live `Vec<ChangeAtom>`'s own heap footprint, and
   the handoff's formula uses the serialized figure as the available proxy). Rust's in-memory
   representation of that many atoms, each with `String`/`Vec` fields, is reliably larger than its
   packed-varint encoding — so "decoder working set" here is really **decoder logic's own allocations
   plus however much the live metadata structures exceed their serialized size**, not a clean isolation
   of the two. Treat it as an upper bound on the decoder's own cost, not an exact figure. This is squarely
   `gix`'s (and, for hg, the revlog reader's) own object/commit-graph/manifest caches — brygge's own code
   does not hold a second copy of this scale.
2. **`git-commits`' file-write holder is negative** (−1.0 MiB): P3's peak (100.0 MiB) is *below* P2's
   (101.0 MiB). This scenario's content is tiny (0.33 MiB) — its peak is almost entirely the Git decoder's
   own fixed overhead (loading the repository, `gix`'s own structures), which varies by roughly a
   megabyte between separate process runs. **Noise, not a real negative cost** — a holder's subtraction
   only means something once the term it's isolating is large relative to the baseline it's subtracted
   from, which is why this batch deliberately profiled the *real* corpus's largest entries and not only
   synthetic ones.
3. **No synthetic hg scenario exists**, so there is no "hg equivalent" row the handoff asked for
   alongside `git-commits`/`cvs-revs`/`svn-dump`. Git's `git fast-import` lets `git-commits` build 20,000
   commits in one streamed subprocess; Mercurial has no comparable bulk-import mechanism available here,
   and building an hg-scale equivalent by shelling out to `hg commit` once per revision would mean tens of
   thousands of process spawns — minutes to hours, not a measurement tool. `tortoisehg-thg` (20,596 real
   changesets) already stands in as the large hg data point; a true synthetic generator (if wanted) is its
   own piece of work, not added here to avoid forcing something weak into existence just to fill the row.

### The per-atom metadata cost (handoff §3)

RFC 010's **5 KiB/atom** figure was synthetic (a worst-case design assumption, per `HANDOFF.md`). Measured
on both the real corpus and these more realistic synthetic scenarios: **219–680 bytes/atom** — 7 to 23×
lower. `fish-shell` (680 B/atom, the richest commit messages and the most path operations per atom) and
`tortoisehg-thg` (481 B/atom) bracket the real-world range; the synthetic scenarios (219–377 B/atom, one
or two trivial path operations per atom, short synthetic messages) sit below it, as expected for
deliberately minimal generators. **Batches W/S/V should size any fixed per-atom budget from the real-world
end of this range, not the synthetic one, and with real headroom above 680 B, not 5 KiB.**

### The spill cost (handoff §3, feeds RFC 015 D-3)

`brygge-bench profile-spill git <fish-shell> <spill-dir>`: decode (baseline, in memory), **write every
blob to its own file** (still holding the whole decoded `Ir`), **drop the `Ir` entirely**, then **read
every blob back from disk one at a time** (never holding more than one), as a disk-backed store would
read on demand.

| Step | Time | Peak |
|---|---:|---:|
| Decode (baseline) | 37.9 s | 9,618 MiB |
| Write every blob (61,428 files; **still holding the `Ir`**) | 8.0 s | 9,400 MiB |
| Drop the `Ir`, then read every blob back, one at a time | 1.2 s | **4,703 MiB** |

- **A real reduction, but only to about half, not to near-zero.** Dropping the fully-materialized `Ir`
  and reading blobs back one at a time *does* lower the peak (9,400 → 4,703 MiB) — genuine memory is
  released, not just the measurement resetting. But a one-at-a-time read-back of a disk-backed store
  should cost on the order of the baseline process plus one blob (tens of MiB, not thousands), so **~4.7
  GiB of resident memory survives the drop.** This is very likely glibc `malloc`'s own arena retention:
  large single allocations (plausibly the content store's biggest backing buffers) are typically `mmap`'d
  and returned to the OS immediately on free, which is consistent with *some* of the ~9.4 GiB being
  released; many smaller allocations (plausibly the `Vec<ChangeAtom>` and its nested `String`/`Vec`
  fields) stay in retained heap arenas that `free()` does not hand back to the kernel.
- **The implication for D-3:** `drop()`-ing an already-fully-materialized `Ir` is not, by itself, a
  working spill strategy on this allocator — it leaves several GiB resident regardless. A genuinely
  disk-backed content store would need to **never fully materialize the content in the first place**
  (stream it to disk as it is produced, holding only metadata and on-demand blob handles), not build the
  whole `Ir` and spill it afterward. That shapes what a streaming writer (D-2) and a disk-backed store
  (D-3) actually need to do, rather than trying to reclaim memory after the fact.
- **The write step's own peak (9,400 MiB) is lower than decode's (9,618 MiB)** by a small, plausible
  margin — likely the same per-run noise as finding 2 above, at a scale where it does not change the
  conclusion.

### Summary for batches W, S and V

- **The IR's content dominates the floor** (2.08–4.20× at P1 on real-content-heavy targets), but **P2's
  serialization step roughly doubles the peak** on every target — holding the decoded `Ir` and its
  serialized bytes simultaneously is the single largest, most consistent cost in this table, and the
  clearest streaming-writer target (RFC 015 D-2: write blobs as they are produced, never holding both
  forms at once).
- **The reader's path (P4/P5) is reliably the cheapest phase** on every target (always below P1), so
  `verify`/`inspect` are not the problem; **P6 (`verify --against-source`) is reliably the most expensive**
  (it holds both a fresh decode and the reader's reconstruction at once) — any future streaming reader
  should prioritize not regressing P4/P5's cheap path while P6 stays inherently the sum of both.
- RFC 010's 5 KiB/atom planning figure is about 10–23× too high for real history; re-baseline from
  219–680 B/atom.
- Naive `drop()` does not reclaim most of a large `Ir`'s memory on this machine's allocator — D-2/D-3's
  design should avoid ever fully materializing content, not rely on freeing it after the fact.

## Result — RFC 015 (batches W, S and V)

The baseline column below reproduces "RFC 015 D-1: where the peak goes" above (brygge `0.4.0` @
`799b025`), unchanged. The 0.5.0 column is the final state after all three implementation batches:
P1/P3 (decode) from request 052's own A/B (batch S, commit `975030f`); P4/P5/P6 (`verify`/`inspect`/
`--against-source`) from request 053's own A/B (batch V, commit `4a87792`). Batch W's own intermediate
numbers (before S's disk-backed store existed) are in request 050; they are not repeated here, since
P1/P3's 0.4.0→0.5.0 column already carries their effect through to the final state. All MiB, 1 MiB =
1,048,576 B.

| Target | Phase | 0.4.0 (`799b025`) | 0.5.0 | Δ |
|---|---|---:|---:|---|
| `fish-shell` | P1 (decode, library) | 9,620 | 5,031 | −48% |
| `fish-shell` | P3 (`decode`, full CLI) | 18,676 | **410** | −98% |
| `fish-shell` | P4 (`verify`) | 9,325 | **64** | −99% |
| `fish-shell` | P5 (`inspect`) | 9,326 | **64** | −99% |
| `fish-shell` | P6 (`verify --against-source`) | 18,932 | **472** | −98% |
| `tortoisehg-thg` | P1 | 3,485 | 1,838 | −47% |
| `tortoisehg-thg` | P3 | 6,813 | **172** | −97% |
| `tortoisehg-thg` | P4 | 3,395 | **45** | −99% |
| `tortoisehg-thg` | P5 | 3,395 | **45** | −99% |
| `tortoisehg-thg` | P6 | 6,868 | **201** | −97% |
| `cvs-revs-5000` (× content, 528 MiB) | P1 | 2,215 (4.20×, D-1's own ratio) | 1,228 (2.33×) | −45% |
| `cvs-revs-5000` | P3 | 3,292 (6.2×, 3,292 ÷ 528) | **698 (1.3×, 698 ÷ 528)** | −79% |
| `cvs-revs-5000` | P4 | 1,344 | **245** | −82% |
| `cvs-revs-5000` | P5 | 1,344 | **245** | −82% |
| `cvs-revs-5000` | P6 | 3,513 | **897** | −74% |
| `svn-dump-20000` | P1 | 139.5 | 118.0 | −15% |
| `svn-dump-20000` | P3 | 243.4 | 118.1 | −51% |
| `svn-dump-20000` | P4 | 112.8 | 32.4 | −71% |
| `svn-dump-20000` | P5 | 112.8 | 32.4 | −71% |
| `svn-dump-20000` | P6 | 232.5 | *not measured in request 053* | |
| `git-commits-20000` | P1 | 99.9 | 77.8 | −22% |
| `git-commits-20000` | P3 | 100.0 | 100.3 | floor-dominated, no change |
| `git-commits-20000` | P4 | 37.9 | 30.9 | −18% |
| `git-commits-20000` | P5 | 37.8 | 31.2 | −18% |
| `git-commits-20000` | P6 | 129.6 | *not measured in request 053* | |

**Artifacts are byte-identical** at every step (W, S-1, S-2, V each proved this independently, against
every bench corpus and all nine real-corpus entries). **What remains uncompacted:** the metadata (every
target's P4/P5 floor above is still roughly its artifact's metadata size, held fully in memory — RFC
015 did not touch this) and the spill's own disk cost (decoding now needs free disk space of about the
artifact's size, in `--spill-dir` or the output directory, which was not a cost before this work).

**Decode time** (full `brygge decode`, ms; batch S's own before/after, `S-1`-only binary → `S-2`,
request 052 F-2 — this baseline is already post-batch-W, not 0.4.0, since batch S did not change
decode's wall-clock path relative to W and this is the only decode-time A/B actually measured):

| Target | before (`S-1`) | after (`S-2`, `9e87cfe`) | Δ |
|---|---:|---:|---|
| `fish-shell` | 38,807 | 39,412 | +1.6% (noise) |
| `tortoisehg-thg` (real corpus) | 64,988 | 64,557 | −0.7% |
| `cvs-revs` (bench) | 23 | 23 | 0% |
| `svn-dump` (bench) | 5 | 5 | 0% |
| `git-commits` (bench) | 13 | 13 | 0% |

No row slowed by more than 2%, well inside run-to-run noise at these scales (request 052's own
assessment, not re-litigated here).

**`verify`/`inspect` time** (ms; batch V's own before/after, pre-streaming-reader → `4a87792`, request
053 §3 — svn-dump/git-commits P4/P5 and all P6 times were reported only qualitatively there, "no row
slower", not as exact figures, so they are not repeated as numbers here):

| Target | Phase | before | after | Δ |
|---|---|---:|---:|---|
| `fish-shell` | P4 | 8,721 | 4,107 | −53% |
| `fish-shell` | P5 | 8,803 | 4,057 | −54% |
| `tortoisehg-thg` | P4 | 2,556 | 1,603 | −37% |
| `tortoisehg-thg` | P5 | 2,959 | 1,603 | −46% |


## Result — per-release corpus (review 111 N-2, review 127 F-2)

One row per release, for eza, fish-shell and TortoiseHg, measured on the release commit per
`docs/src/development/releasing.md`'s cut checklist, **only when a decoder, `brygge-ir`'s reader, writer
or content store, or the CLI's decode path changed since the previous release** — otherwise the previous
release's row is reused, stated as such rather than re-measured. For request 061's and review 100's
earlier figures (contract 0.2.0 vs. 0.3.0, around the compact-artifacts batch), see those documents
directly; they are not repeated here. Their method was the same as this table's (`os.wait4` per child on
the real CLI, `measure2.py`) — what actually differs is the input (unpinned clones, carrying upstream's
history as of the fetch date) and the code (`d635961`, several commits before 0.7.0).

**The method, stated once:** `tools/bench/release-corpus.py <brygge-binary> fixtures/corpus
tools/corpus/real.toml eza fish-shell tortoisehg-thg` (review 128 R-2: the kind for each name comes from
`real.toml` itself, not from scanning `fixtures/corpus`'s subdirectories). For each repository it runs
`brygge decode <kind> <source> --out <artifact>` then `brygge verify <artifact>`, each as **one child**,
its own peak RSS taken with `os.wait4`'s `ru_maxrss` (never `RUSAGE_CHILDREN`'s running total across two
calls — the review 099 lesson) and timed around the same call. Fetched and verified once into the
project's own `fixtures/corpus` (review 111 N-1, review 127 F-1's `tools/bench real-fetch`/`real-check`
split, review 128 G-1's ref-normalized pins), never re-cloned per release. **Going
forward** (`releasing.md`'s N-2 step), this script runs twice in one session — once with
`fixtures/previous-release/<version>/brygge`, once with the new release build — so both rows can be
compared under identical conditions; this table's 0.7.1 row is the first one, filled retroactively with
only the one binary that exists for it.

| Release | Repository | Artifact size | Decode time | Decode peak RSS | `verify` time | `verify` peak RSS |
|---|---|---:|---:|---:|---:|---:|
| 0.7.1 | eza | 10,727,417 B | 1,396 ms | 105,024 KiB | 151 ms | 16,712 KiB |
| 0.7.1 | fish-shell | 991,301,357 B | 54,969 ms | 427,944 KiB | 9,933 ms | 80,444 KiB |
| 0.7.1 | tortoisehg-thg | 336,240,068 B | 72,171 ms | 256,680 KiB | 3,458 ms | 57,264 KiB |

**From this row on, the pins fix the decoded history (review 128 G-1).** The row above was re-measured
after `real-fetch` normalizes every Git clone's refs to exactly the pin (its default branch reset to the
pin, every remote removed, every tag not merged into the pin deleted) and clones every hg entry with
`hg clone -r <pin>` (no bookmarks, a single head). An earlier, unpinned measurement of this same binary
against ordinary clones (jq 62 commits past its pin, fish-shell 166, eza 1) gave fish-shell 1,065,484,138
bytes — **7.5% larger than the figure above** (the difference taken against the smaller, pinned figure)
— because the clone still carried upstream's later commits and tags; it was never a decoder change or a
measurement error, and is not repeated here now that the cause is known and fixed. eza's row
(10,727,417 B) is close to, but not identical to, request 061's 10,728,077 B: expected, since that
unpinned clone's branch was one commit past eza's pin, and G-1 also deletes the `v0.22.0` tag, not
merged into the pin, that the clone still carried. tortoisehg-thg's row matches request 061's figure
(336,240,068 B) exactly, since hg's `heads(all())`/no-bookmarks check already held for that clone.

**0.7.1's row stands for 0.7.0 too:** `git diff --stat 0.7.0 0.7.1 -- crates/` touches only
`crates/*/Cargo.toml` (the `homepage` removal), `crates/brygge/README.md`, and the two plugin integration
test files (`encode_integration.rs`, `transform_integration.rs`, Rust 1.100's test-binary-layout fix) —
no decoder, `brygge-ir`, or CLI decode-path source file. So 0.7.0's own corpus figures would be identical
to 0.7.1's within run-to-run noise; this table records 0.7.1's real measurement once, rather than two
runs that could never meaningfully differ.

Machine: AMD Ryzen 9 9950X (16 cores), 59 GiB RAM, x86_64 Linux. Measured once, with
`fixtures/previous-release/0.7.1/brygge` — 0.7.1's own published Linux binary (`tools/fixtures.sh
fetch`'d and checksum-verified, never a local rebuild), not a second build of this tree: the measurement
is 0.7.1's, and it stands for 0.7.0 as explained above.

### The 0.8.0 cut (request 077)

This cycle changed decoders (the SVN copy rule, `--branch-lines`, the CVS `trunk` ref), `brygge-ir`'s
builder and `Edit`, and the CLI — the real corpus run is due (111 N-2). Both rows below are from one
session, this machine, `fixtures/previous-release/0.7.1/brygge` and the 0.8.0 release build:

| Release | Repository | Artifact size | Decode time | Decode peak RSS | `verify` time | `verify` peak RSS |
|---|---|---:|---:|---:|---:|---:|
| 0.7.1 (re-measured) | eza | 10,727,417 B | 1,434 ms | 107,840 KiB | 173 ms | 17,332 KiB |
| 0.7.1 (re-measured) | fish-shell | 991,301,357 B | 63,314 ms | 434,532 KiB | 9,580 ms | 79,500 KiB |
| 0.7.1 (re-measured) | tortoisehg-thg | 336,240,068 B | 81,149 ms | 254,996 KiB | 3,442 ms | 56,544 KiB |
| 0.8.0 | eza | 10,727,417 B | 1,207 ms | 104,576 KiB | 142 ms | 16,936 KiB |
| 0.8.0 | fish-shell | 991,301,357 B | 51,085 ms | 430,308 KiB | 9,677 ms | 79,172 KiB |
| 0.8.0 | tortoisehg-thg | 336,240,068 B | 94,999 ms | 256,228 KiB | 4,422 ms | 55,444 KiB |

**No decoder, `brygge-ir`, or CLI decode-path source file touches Git, hg or CVS decoding differently
for these three repositories this cycle** (0.8.0's changes are Subversion's copy rule and
`--branch-lines`, the CVS `trunk` ref, the Git encoder, and the builder's copy-record guard, none of
which this table's three repositories exercise on decode). **Comparison:** artifact bytes are identical
for every repository, both releases; every peak RSS is within 1% between releases; decode and `verify`
times move in both directions by up to about 20% per repository, which is this machine's own run-to-run
noise, not a regression — confirmed by re-measuring `tortoisehg-thg` with both binaries a second time in
this same session: the unchanged 0.7.1 binary alone gave 81,149 ms and 92,255 ms on two successive runs
(a 14% swing with nothing different), and the second 0.7.1/0.8.0 pair (92,255 ms / 81,808 ms) reversed
which release looked faster. No artifact-size or peak-RSS difference exceeds noise, so nothing here
stops the cut.

Machine: the same as the table above, same session.

### The 0.10.0 cut (request 091)

The Git decoder, the IR reader and writer, and `verify` changed this cycle (RFC 021), so the real corpus run is due
(111 N-2). The rows below are from one session on this machine: `fixtures/previous-release/0.9.0/brygge`, then the
0.10.0 release build, each the median of three.

**Load-confounded, so no time comparison was drawn from this session** (review 174's rule; kept below, labelled, for
the record — superseded by the quiet re-measurement that follows):
- `uptime` was 13.81 at the start of the 0.9.0 leg and about 24.8 at the start of the 0.10.0 leg;
- the 0.9.0 binary's own three fish-shell decodes ranged from 58,007 to 95,088 ms;
- **the peaks do not depend on machine load, and are comparable:** every one is within about 5%;
- the artifact sizes differ by a few bytes, from the version string in provenance (review 174). That the history is
  the same is proved by the cross-version check, not by bytes.

| Release | Repository | Artifact size | Decode time | Decode peak RSS | `verify` time | `verify` peak RSS |
|---|---|---:|---:|---:|---:|---:|
| 0.9.0 (re-measured, load-confounded) | eza | 10,727,417 B | 1,551 ms | 104,580 KiB | 161 ms | 16,880 KiB |
| 0.9.0 (re-measured, load-confounded) | fish-shell | 991,301,357 B | 73,947 ms | 432,628 KiB | 10,756 ms | 79,088 KiB |
| 0.9.0 (re-measured, load-confounded) | tortoisehg-thg | 336,240,068 B | 81,025 ms | 253,004 KiB | 3,664 ms | 55,436 KiB |
| 0.10.0 (load-confounded) | eza | 10,727,410 B | 1,437 ms | 105,948 KiB | 161 ms | 17,784 KiB |
| 0.10.0 (load-confounded) | fish-shell | 991,301,349 B | 62,497 ms | 430,660 KiB | 10,379 ms | 80,968 KiB |
| 0.10.0 (load-confounded) | tortoisehg-thg | 336,240,069 B | 87,997 ms | 256,136 KiB | 3,573 ms | 56,152 KiB |
| 0.10.0 | jq (new: 0.9.0 refuses it) | 27,397,705 B | 1,165 ms | 109,524 KiB | 284 ms | 10,896 KiB |

**The 0.8.1 and 0.9.0 cuts' corpus figures** are in their cut requests (080, 085). They were not copied here, which
is an omission this section names rather than repairs from memory.

### The quiet re-measurement (request 093)

**The binaries, named by their own SHA-256, never a local rebuild** (review 195's Q-1):
- **0.9.0:** `d1e2e35d…ca500e`, the published `brygge-x86_64-unknown-linux-gnu.tar.gz`'s `brygge`, fetched fresh for
  this run, checked against its own `.sha256` and attestation, and deleted afterwards;
- **0.10.0:** `0ad7bd3e…f28856`, `fixtures/previous-release/0.10.0/brygge`, the pinned, published, attested binary
  (`tools/fixtures.sha256`).

Six runs, alternating binaries (0.9.0, 0.10.0, 0.9.0, 0.10.0, 0.9.0, 0.10.0) in one sitting on one machine, each
checked against `uptime`'s 1-minute average **immediately before that run started**; the threshold held for every
run (the highest reading, 13.61, was still comfortably under 16), so **no run was discarded this time.** Three runs
per binary; the table gives the median of three for each.

| Release | Repository | Artifact size | Decode time (median) | Decode peak RSS (median) | `verify` time (median) | `verify` peak RSS (median) |
|---|---|---:|---:|---:|---:|---:|
| 0.9.0 | eza | 10,727,417 B | 1,213 ms | 105,236 KiB | 144 ms | 16,448 KiB |
| 0.9.0 | fish-shell | 991,301,357 B | 49,695 ms | 438,496 KiB | 9,559 ms | 79,240 KiB |
| 0.9.0 | tortoisehg-thg | 336,240,068 B | 69,762 ms | 254,196 KiB | 3,553 ms | 56,432 KiB |
| 0.10.0 | eza | 10,727,410 B | 1,214 ms (+0.1%) | 105,388 KiB (+0.1%) | 143 ms (−0.7%) | 17,260 KiB (+4.9%) |
| 0.10.0 | fish-shell | 991,301,349 B | 49,707 ms (+0.0%) | 436,484 KiB (−0.5%) | 9,238 ms (−3.4%) | 80,892 KiB (+2.1%) |
| 0.10.0 | tortoisehg-thg | 336,240,069 B | 68,299 ms (−2.1%) | 256,808 KiB (+1.0%) | 3,373 ms (−5.1%) | 56,648 KiB (+0.4%) |

**The worst adverse swing is eza's `verify` peak RSS at +4.9%, well under the 10% gate.** No figure moved by a
multiple, the shape a real regression would show. **Judgment: no stop, no regression.**

## Result — Git target (RFC 019 D-8)

One row per run: the real decode (and, where the handoff's §1 "Before encoding" step applies,
`transform authors --require-all` with a placeholder map — see the notes below each decode kind) into
an artifact, then `encode git` (with its own check against the artifact), then a real `git fsck
--strict` on the result. The method is `tools/bench/release-corpus.py`'s `os.wait4`-per-child discipline
(review 127 F-2), extended with `--decode-args` and `--encode-git` (RFC 019 D-8, batch G4): one child per
measurement, one session, one machine, the same as every other table in this file.

| Source | Repository | Decode args | Commits | Refs | `encode git` check | Decode time | Decode peak RSS | Encode (+check) time | Encode (+check) peak RSS | `git fsck --strict` |
|---|---|---|---:|---:|---|---:|---:|---:|---:|---|
| Git → Git | eza | (default) | 3,372 | 96 | `corresponds` | 1,653 ms | 109,904 KiB | 2,447 ms | 62,420 KiB | clean |
| Git → Git | fish-shell | (default) | 23,270 | 104 | `corresponds` | 64,287 ms | 429,456 KiB | 94,343 ms | 369,520 KiB | clean |
| Mercurial → Git | tortoisehg-thg | (default) | 20,596 | 4 | `corresponds` | 78,294 ms | 254,240 KiB | 35,291 ms | 258,796 KiB | clean |
| Subversion → Git | wordpress-core | `--branch-lines` | 2,697 | 7 | `corresponds` | 655 ms | 104,600 KiB | 1,325 ms | 42,256 KiB | clean |
| CVS → Git | openbsd-games | `--reconstruct-refs` | 1,430 | 123 | `corresponds` | 1,806 ms | 164,988 KiB | 5,331 ms | 114,948 KiB | clean |
| CVS → Git | openbsd-cpio | `--reconstruct-refs` | 7 | 5 | `corresponds` | 18 ms | 9,168 KiB | 30 ms | 8,932 KiB | clean |
| CVS without `--reconstruct-refs` | openbsd-cpio | (default) | — | — | refused | 15 ms | 9,180 KiB | — (refused before writing) | — | — |
| Subversion → Git, deleted branch | codeblocks | `--branch-lines` | 6,115 | 40 | `corresponds` | 107,380 ms | 1,456,268 KiB | 23,349 ms | 185,040 KiB | clean |

**Every peak stayed well under the 1.5× bound** (the highest ratio among the first seven rows,
tortoisehg-thg's, is 1.02×; codeblocks's is 0.13×; every other row is 0.40×–0.86×). No run's check
failed, and no `git fsck --strict` complained about anything.

**Memory, against the published 0.7.1's plain decode of the same dump** (review 151 D-4, `os.wait4` per
child, this machine): codeblocks `--branch-lines`, with this request's fix, peaks at 1,456,268 KiB (this
run) / 1,457,836 KiB (the architect's); 0.7.1 plain peaks at 1,506,448 KiB (this run) / 1,505,576 KiB
(the architect's) — the two independent measurements agree to within 0.1%. **G5's bound (1.5× plain)
holds**, at about 0.97×. **About 1.5 GB of peak for 640 MB of content is above RFC 015's "memory tracks
metadata."** It is likely the per-live-line snapshot retention (§3) for codeblocks' unusually large
number of tag copies, and is **pre-existing, not 0.8.0's** — the architect will plan it as a scale item;
not tuned here.

- **Subversion and CVS rows** show the decode's own figures (`decode`, `verify`, `encode git`, without
  the authors step, which `release-corpus.py` does not yet instrument). The real author-mapping
  workflow — `inspect --authors`, filling in each right-hand side with a placeholder
  `<name>@example.invalid` address, `transform authors --require-all`, then `encode git` on the mapped
  artifact — was run by hand for both: `transform authors` mapped all 97 identities for openbsd-games
  and all 6 for openbsd-cpio, with none left unmapped, and `encode git` on each mapped artifact still
  gave `corresponds` with a clean `fsck`, the same commit/ref counts as the table (openbsd-cpio's row
  above already uses `--reconstruct-refs`; `transform authors` does not change counts).
- **`CVS without --reconstruct-refs`:** `decode` succeeds (exit 10, "recorded loss") with 0 refs, exactly
  as it would without this batch. `encode git` then refuses, naming every unreachable atom and the fix:

  ```
  the artifact cannot be written as Git:
    7 atom(s) are not reachable from any ref, starting with COPYING.LIB@1.1\u{000a} COPYING@1.1\u{000a}
    ChangeLog@1.1\u{000a} Makefile@1.1\u{000a} NEWS@1.1\u{000a} README@1.1\u{000a}…, main.c@1.2,
    Makefile@1.2, fnmatch.h@1.2, COPYING.LIB@1.2\u{000a} ChangeLog@1.2\u{000a} Makefile@1.3\u{000a}
    NEWS@1.2\u{000a} README@1.2\u{000a} VERSION@1.2\u{000a}… — decode with --reconstruct-refs, which is
    needed to give this history any ref at all
  ```

**The strongest check** (the architect's method, review 136): for eza and fish-shell, every result
commit's tree, author, committer and time were compared against the source commit its own
`Brygge-Source-Id` trailer names, over the real source repositories in `fixtures/corpus`:

| Repository | Commits compared | Mismatches |
|---|---:|---:|
| eza | 3,372 | 0 |
| fish-shell | 23,270 | 0 |

**The Subversion tree comparison** (as request 074 did, over the same corpus entry): `wordpress-core`'s
`trunk` tip (`r2700`) and `1.5` tip (`r2690`, `branches/1.5`) were each exported from a local load of the
real dump (`svnadmin load`, then `svn export file://… <path>@<rev>`) and compared against the matching
Git tree (`git archive`). The only files that differ are `wp-includes/class-pop3.php` and
`wp-includes/functions-compat.php`, both only in their `$Id$`/`$Id: … $` line — Subversion's
`svn:keywords` expansion, which Git has no equivalent of and the IR does not carry; confirmed by reading
the property directly (`svn:keywords` on that path is `Author Date Id Revision`). No other file differs,
on either branch.

**The deleted-branch Subversion repository, searched for properly this time (RFC 019 Amendment A,
review 149):** two real, public candidates were found and confirmed to really have one, with real
`svnrdump`/`svn log` queries against their live hosts, not from memory:

- **Apache's `jakarta/oro`** (`https://svn.apache.org/repos/asf/jakarta/oro/`, Apache License 2.0):
  `branches/oro`, created in r54271 (copied from `trunk@54270`) and deleted in r54630. **Not used:**
  `svn.apache.org` is one shared repository across every Apache project, so `svnrdump dump` over this
  subtree still has to walk the *global* revision counter for the whole range — a test dump of
  `r0:5000` (of the ~54,630 needed) fetched only 72 KiB in 60 s before being stopped; the project's own
  commit rate afterward would not fix that, since the overhead is per global revision, not per commit
  touching the path.
- **SourceForge's `codeblocks`** (`https://svn.code.sf.net/p/codeblocks/code/`, GPL-3.0-or-later): its
  own, dedicated revision counter (not shared), several real deleted or deleted-then-renamed branches
  (`branches/codecompletion_refactoring`, deleted r6220, the earliest; also `release-16.xx`,
  `release-17.xx`, `release-18.xx`, `release-20.xx`, `release-24.12`, `release-25.01`, each deleted or
  renamed-and-deleted between r11258 and r13623). **Fetched for real:** `svnrdump dump
  https://svn.code.sf.net/p/codeblocks/code -r 0:6220` (`codecompletion_refactoring`, the earliest and
  so the cheapest range), 6,221 revisions, 170 MB, ~22 minutes. Loaded locally and confirmed the
  deletion is really there.

  **The real decoder bug this dump surfaced is now fixed (review 151, request 076), and the entry is
  added.** `decode svn --branch-lines` on this real dump wrote an artifact that failed its own
  canonical-form check on read-back: `ChangeAtom.copies are not strictly ascending by (to, from,
  from_atom)`. Worse than first understood: the **default** (single-spine) and `--reconstruct-refs`
  decodes of this same dump had the identical fault — since 0.1.0, any revision that copies a directory
  and also restates a file inside it as its own copy wrote an artifact its own reader rejected. It
  failed closed, so nothing wrong was ever written, but the repository could not be decoded at all; the
  published 0.7.1 itself hits it (`decode` exits 10 and writes the artifact, then `verify` gives
  `internal: fail`, FAIL).

  Two real faults, both fixed:
  - `IrBuilder::add_atom` sorted `copies` but had no check for a duplicate `(to, from, from_atom)`
    triple (only for a duplicate op *path*), so the write itself succeeded silently; only a strict
    read-back (what `verify`, `encode git` and `--against-source` all do) caught it.
    `IrBuilder::add_atom` now refuses such a duplicate, naming the triple, for every decoder and every
    `Edit`-based transform;
  - the duplicate is made in `tree.rs`'s shared, single-spine copy tracking (read by both the default
    decode and `--branch-lines`), diagnosed on this exact dump: r5310 adds
    `branches/scintilla/src/plugins/contrib/IncrementalSearch` as a whole-directory copy from
    `trunk/.../IncrementalSearch@5309`, and the same revision separately restates every one of that
    directory's files (and its `images/`/`images/16x16/` subdirectories) with its own explicit
    delete-then-add, copying from the identical source path at the identical source revision. The
    directory-snapshot expansion and the explicit-node handling each pushed their own `CopyRecord` for
    the same 49 files — two records for one destination. An earlier fix collapsed two *equal* records
    in `branch_lines.rs`'s `resolve_copies` alone, which happened to hold for this one case but is the
    wrong place and the wrong rule: it covers neither the default/`--reconstruct-refs` decodes nor the
    general case of a directory copy later replaced by a copy from a *different* source (which is not
    a duplicate to collapse — it is a false second copy claim). The real fix, in `tree.rs`'s
    `apply_revision`: within one revision, a node that **deletes or replaces** a path voids every
    earlier copy record of that revision under that path — SVN's own meaning, since the explicit
    delete-then-add restates the path, so an earlier directory-copy record no longer describes the
    result. Each destination then keeps exactly one record per revision: the last node's. No collapse
    needed anywhere.

  With both fixes, `codeblocks` (`r0:6220`) is now added to `real.toml`, with the S-3 canonical dump
  hash. All three Subversion decode modes now succeed and `verify` cleanly: plain, `--reconstruct-refs`
  and `--branch-lines`, each `verify`/`verify --against-source` giving `corresponds`. `encode git` (with
  G2's check, `corresponds`) succeeds on the `--branch-lines` artifact; its deleted branch appears as
  `refs/brygge/other/branches/codecompletion_refactoring@r5769` (the branch's actual last revision, not
  r6220 — r6220 is only the end of the dumped range), and `git fsck --strict` is clean. Its row is in
  the table above. The full real corpus (`brygge-bench real tools/corpus/real.toml fixtures/corpus`,
  every entry, every variant, internal `verify` and `--against-source`) was also re-run under the new
  guard: every row `ok` (jq's two variants fail only on the pre-existing, unrelated submodule refusal).

**Memory:** AMD Ryzen 9 9950X (16 cores), 59 GiB RAM, x86_64 Linux — the same machine and method as
every other table here.

## Result — 0.8.1 (Amplification bounds, request 078)

Four declared ceilings (`max_tree_entries`, `max_copy_expansion`, `max_retained_entries`, `max_ir_ops`)
against the amplification found while scoping 0.9 (review 157): a small Subversion dump with many
directory copies, or a small Git repository whose trees reuse one subtree, could otherwise make `decode`
use memory out of all proportion to its size. Each is checked before storing, not after allocating.

**The real corpus still decodes byte-identically to 0.8.0** — no ceiling fires on any real repository.

**Cost per retained entry, measured (not estimated):** a Subversion input built to reach
`max_retained_entries`'s 20,000,000-entry ceiling was refused at exit 20 after 16.7 s, peaking at
3,312,196 KiB — about **170 bytes per retained entry**. At that cost the default stays well under the
~4 GiB judgment threshold for choosing it silently; 0.9's shared-structure tree (RFC 020) is what makes
the real cost smaller still, for any repository that is not itself amplifying.

**Every corpus repository stays at least 2.96× under every ceiling** (the architect's own measurement;
`codeblocks`'s `max_retained_entries` use is the tightest).

**Both of the architect's own amplifying inputs are refused at the default ceilings**, run once by hand
and never committed (the generator inputs themselves are not part of this record, by design — see
CHANGELOG's `### Security` entry for 0.8.1): the Subversion one in 16.7 s, peaking at 3,312,196 KiB; the
Git one in 22 s, peaking at 686 MiB.

## Result — RFC 020 batch M, "measure first" (request 082)

**Measurement only — no change to any published crate** (the handoff's own scope; this batch touches
only `tools/bench/`). The question: where codeblocks' and a tag-heavy synthetic dump's memory and time
go, and whether the retained snapshots and the working tree dominate the peak — RFC 020 D-1's premise,
before any decoder change.

### A new scenario: `svn-tags-plain <n>` / `svn-tags <n>`

A 2,000-file trunk, then `n` times "edit one trunk file, copy trunk to `tags/t<n>`" (RFC 020's "157"
shape, already in the accepted RFC — not a secret). `svn-tags-plain` decodes without `--branch-lines`,
`svn-tags` with it. Level 1: both self-check against the generator's own analytically-known atom/blob/
byte counts (plain) or ref count (branch-lines, `n + 1` — the trunk plus every tag), confirmed exactly at
every scale tried.

**The handoff's own matrix named N = 10, 40, 160. 160 already crosses 0.8.1's `max_retained_entries`
ceiling** (confirmed: refused, `the snapshot total brygge must retain for later copies in this dump`,
`20000000 entries`) **— tighter than the handoff assumed.** Why: a tag's files become ordinary live paths
in the one repository-wide tree from then on, so retaining a later trunk snapshot (for a later tag) costs
the trunk *plus every earlier tag* — the retained-entry total is `K·N·(N+1)/2` (K = 2,000), not `K·N`.
Solving for the ceiling: 140 is the largest round N that still completes; 150 already refuses (confirmed
both ways). The matrix below uses 140 in place of 160; **this is a stop-condition-adjacent finding for
the architect, not something this batch fixed** — RFC 020's own text may want this number corrected
before batch S, though it does not change D-1's premise (if anything it strengthens it: the growth really
is the quadratic shape RFC 020 describes).

| Scenario | N | Peak (KiB) | Time (ms) | Atoms | Blobs | Content (B) |
|---|---:|---:|---:|---:|---:|---:|
| `svn-tags-plain` | 10 | 45,852 | 59 | 23 | 2,010 | 514,560 |
| `svn-tags-plain` | 40 | 352,376 | 1,260 | 83 | 2,040 | 522,240 |
| `svn-tags-plain` | 140 | 3,414,076 | 13,744 | 283 | 2,140 | 547,840 |
| `svn-tags-plain` | 150 | — | — | refused: `20000000 entries` | | |
| `svn-tags` (`--branch-lines`) | 10 | 36,404 | 94 | 21 | 2,010 | 514,560 |
| `svn-tags` | 40 | 310,940 | 2,009 | 81 | 2,040 | 522,240 |
| `svn-tags` | 140 | 3,266,632 | 28,034 | 281 | 2,140 | 547,840 |
| `svn-tags` | 150 | — | — | refused: `20000000 entries` (same message) | | |

Machine: AMD Ryzen 9 9950X, 59 GiB RAM, x86_64 Linux, release build.

**The cost of one tree entry today, measured (differential, not absolute — cancels the roughly-constant
baseline):** `(peak(140) − peak(10)) / (retained(140) − retained(10))`, retained(N) = `2000·N·(N+1)/2`.
Three pairs agree closely: (140,10) → 175.6 B; (140,40) → 173.2 B; (120,10) → 178.0 B. **About 175 B per
retained entry** — consistent with review 159's ~170 B figure for the same structure (a different
ceiling's message, the same `Tree`/`FileEntry` cost), cross-validating both.

### The profile, by phase (`brygge-bench profile`, extended)

`codeblocks` (the real corpus's own tag-heavy entry, already known from the Git-target table above to
have an unusually large retained-snapshot cost) and `svn-tags` at N = 140 (not the handoff's 160, per
above), each plain and `--branch-lines`. P1 = decode only (library); P2 = decode + serialize; P3 = the
full CLI `decode`; P4/P5/P6 = `verify`/`inspect`/`verify --against-source`.

| Target | Phase | Time (ms) | Peak (KiB) |
|---|---|---:|---:|
| `codeblocks` | P1 | 66,376 | 2,190,868 |
| `codeblocks` | P2 | 84,966 | 2,376,760 |
| `codeblocks` | P3 | 59,509 | 1,503,220 |
| `codeblocks` | P4 | 1,602 | 545,600 |
| `codeblocks` | P5 | 1,252 | 87,296 |
| `codeblocks` | P6 | 52,940 | 1,505,928 |
| `codeblocks` (`--branch-lines`) | P1 | 109,412 | 2,145,532 |
| `codeblocks` (`--branch-lines`) | P2 | 97,889 | 2,382,524 |
| `codeblocks` (`--branch-lines`) | P3 | 95,653 | 1,461,804 |
| `codeblocks` (`--branch-lines`) | P4 | 1,101 | 47,084 |
| `codeblocks` (`--branch-lines`) | P5 | 1,051 | 32,820 |
| `codeblocks` (`--branch-lines`) | P6 | 151,113 | 1,431,952 |
| `svn-tags-140` | P1 | 21,218 | 3,409,568 |
| `svn-tags-140` | P2 | 15,977 | 3,439,932 |
| `svn-tags-140` | P3 | 16,429 | 3,440,376 |
| `svn-tags-140` | P4 | 1,352 | 1,759,548 |
| `svn-tags-140` | P5 | 300 | 167,144 |
| `svn-tags-140` | P6 | 16,681 | 3,533,796 |
| `svn-tags-140` (`--branch-lines`) | P1 | 30,999 | 3,265,732 |
| `svn-tags-140` (`--branch-lines`) | P2 | 35,000 | 3,266,700 |
| `svn-tags-140` (`--branch-lines`) | P3 | 27,703 | 3,266,488 |
| `svn-tags-140` (`--branch-lines`) | P4 | 50 | 4 |
| `svn-tags-140` (`--branch-lines`) | P5 | 50 | 12 |
| `svn-tags-140` (`--branch-lines`) | P6 | 25,949 | 3,268,076 |

**`codeblocks --branch-lines` P3 (1,461,804 KiB) agrees with the Git-target table's earlier figure above
(1,456,268 / 1,457,836 KiB, two independent measurements) to within 0.4%** — the same shape, re-confirmed
across batches, not a regression.

**P3 reported `ok=false`** for `codeblocks --branch-lines` only: the real dump has paths outside the
default layout, so the decode exits flagged (30, `CONVENTION_VIOLATION`), which `measure()` did not yet
recognize as success — the decode and serialization both plainly succeeded (the artifact was written,
108,071,349 bytes, matching P2's `artifact_bytes` exactly). **Fixed (review 168 N-2):** `measure()` now
accepts exit 30 alongside 0 and 10, so this reads `ok=true`.

### Separating what the RFC must change from what it must not (§2)

Exact counts, from the public `Ir` (`ir.atoms.len()`, `ir.atoms[].ops.len()`/`.copies.len()`,
`ir.refs.len()`, `ir.content.total_bytes()` — no decoder change needed):

| Target | Atoms | Ops | Copies | Refs | Content (B) |
|---|---:|---:|---:|---:|---:|
| `codeblocks` | 6,221 | 106,778 | 57,600 | 0 | 663,442,936 |
| `codeblocks` (`--branch-lines`) | 6,115 | 58,139 | 3,973 | 40 | 640,032,915 |
| `svn-tags-140` | 283 | 282,140 | 280,000 | 0 | 547,840 |
| `svn-tags-140` (`--branch-lines`) | 281 | 2,140 | 0 | 141 | 547,840 |

**(a) retained snapshots and (b) the working tree cannot be read as a live counter** (both are gone once
`decode()` returns; only the final `Ir` is inspectable from outside) — **estimated**, per the handoff's
own method, from what the generator is known to build: `retained(140) = 2000·140·141/2 = 19,740,000`
entries × ~175 B/entry (measured above) ≈ **3.22 GiB**, the same for both modes (the retention math is
`tree.rs`'s, shared by `--branch-lines` unchanged).

**(c) IR metadata and (d) the content store are measured directly**, using `size_of` on the public atom/
op/copy/ref types (`BlobId` 32 B, `PathOp` 168 B, `CopyRecord` 184 B, `RefRecord` 464 B, `ChangeAtom`
600 B — stack sizes; heap path-string bytes added at the generator's own known average length, ~18 B for
`svn-tags`):

| Target | (c) IR metadata (est.) | (d) Content store (measured) | (c)+(d) |
|---|---:|---:|---:|
| `svn-tags-140` | ~109.0 MiB | 0.52 MiB | ~109.5 MiB |
| `svn-tags-140` (`--branch-lines`) | ~0.6 MiB | 0.52 MiB | ~1.1 MiB |
| `codeblocks` | ~39.2 MiB (40 B avg path) | 632.6 MiB | ~671.8 MiB |
| `codeblocks` (`--branch-lines`) | ~16.1 MiB | 610.2 MiB | ~626.3 MiB |

**svn-tags-140: (a)+(b) ≈ 3.22 GiB against a measured peak of 3.25 GiB (plain) / 3.12 GiB
(`--branch-lines`) — (a)+(b) alone accounts for essentially the whole peak** (the (a)+(b)+(c)+(d) sum
lands within ~3% of measured peak either way, so (e) is ~0, inside this estimate's own margin).
**`--branch-lines` makes (c) nearly disappear** (0.6 MiB vs 109.0 MiB plain) — RFC 019 D-9's "a copy that
creates a root has no ops" measured directly: 2,140 ops instead of 282,140, zero copy records instead of
280,000.

**N-1 correction (review 168): the content store is not resident in the real CLI's process.** P1/P2 are
library calls using `Storage::Memory` (`brygge_decode_svn::decode`'s own default), so (d) is genuinely
part of *their* peak. P3–P6 run the real `brygge` binary, which always spills content to disk (RFC 015
D-4) — the content is never in that process's resident memory, so (d) must not be subtracted from P3's
peak, as the first version of this section wrongly did.

**The correct split, for P3 (the real `decode` command users actually run): peak 1,503,220 KiB; (c) ≈
39.2 MiB; so (a)+(b)+(e) ≈ 1.43 GiB, about 96% of peak — not the 67–70% first reported here.** The
conclusion is stronger, not weaker: on a real, tag-heavy repository, the retained snapshots and the
working tree are very nearly the whole of the real command's peak, the same as on the synthetic
`svn-tags`. (a)+(b)+(e) still cannot be split further from outside (a real repository's retained-entry
count, unlike `svn-tags`'s known `K`/`N`, is not independently knowable without the decoder reporting it),
but with (d) correctly excluded there is essentially nothing left for (e) to be: RFC 020's own Motivation
table already names codeblocks' 316 copy-source revisions, consistent with (a)+(b) alone accounting for
nearly all of it.

### The judgment

**Does (a)+(b) dominate the peak? Yes, clearly, on both.** On `svn-tags` (the shape D-1 targets directly)
— essentially the entire peak in both modes. **On `codeblocks`'s real CLI peak (P3, corrected per N-1),
(a)+(b)+(e) is about 1.43 GiB of a 1.50 GiB peak — about 96%**, with (c) (the IR's metadata alone, ~39
MiB; the content store is spilled to disk, never resident in that process) making up the rest. A real
repository is *not* less amplification-dominated than the synthetic worst case, as this batch first
reported — the retained-snapshot/working-tree cost is nearly the whole peak either way. **D-1 is
confirmed as specified, more strongly than first reported**: a retained-snapshot/working-tree cost this
large, for both a real repository with many tags and a deliberately adversarial one, is exactly what
shared-structure sharing (D-1) removes.

### What could not be measured from outside, and why

- **The three-way time split** (dump parse / `apply_revision` incl. tree clone and diff / the builder):
  `brygge-decode-svn`'s public API is `decode`/`decode_with_storage` only — `tree::apply_revision` and
  `dumpstream::parse_dump_with` are not reachable from outside the crate, even though some are `pub` in a
  private module. Only the whole decode call can be timed as one span externally (P1/the matrix's own
  `time_ms`), which this batch used instead of inventing a fake split.
- **A live (a)/(b) counter during decode**: once `decode()` returns, `kept`/`prev_tree` are gone; only the
  estimate above (built from what the generator is known to build, per the handoff's own allowance) was
  available, not a true in-decode reading.
- **codeblocks' own (a)+(b) vs (e) split**: unlike the synthetic scenario, a real repository's retained-
  entry count is not derivable from outside without the decoder reporting it.

None of these required or used a decoder change; the gaps are stated rather than approximated past what
the public API and the generator's own known shape support.

## Result — RFC 020 batch S, F-1/F-2/F-3 (review 169's fix batch on request 083)

Review 169 found that the first batch-S submission's `svn-tags` time figures (24.8 s at N=160, 87
minutes at N=2,560, plain) did not match a clean measurement of the real binary (2.1 s at N=160), and
that `--branch-lines` was genuinely quadratic in N — not the "sibling-scan" explanation first given, but
`branch_lines.rs`'s own per-revision partition, which flattened the **whole** repository tree every
revision to bucket it by root. This section replaces that batch's §5 with the real binary's own figures,
and reports the fix (F-1), the measurement method (F-2), and the kept-snapshot release (F-3).

### F-2: why the gap, and the method going forward

**Investigated, not assumed.** The hypotheses offered (`Storage::Memory` vs. the real binary's
`Storage::Spill`, a self-check walk, the corpus's own shape summary) were checked by re-running the
*exact same* `svn-tags-plain N=160` scenario, in-process and as the real binary, back to back, with
nothing else intentionally started on the machine:

| Method | Time |
|---|---:|
| `brygge-bench run svn-tags-plain 160` (in-process, `Storage::Memory`, decode only) | 0.66–0.83 s |
| `brygge decode svn … --out …` (the real binary, one child, `Storage::Spill` + serialize + report) | 1.96–2.32 s |

**In-process is *faster*, not 24.8 s slower** — the expected direction, since it skips the content-spill,
serialization and report steps the real binary always does. The originally-reported 24.8 s (and the 87
minutes at N=2,560) could not be reproduced under a clean run of the *same, unfixed* code either; this
machine runs several other sessions' builds concurrently (`uptime`'s load average was seen as high as ~60
on a 32-thread machine during this batch), and the first submission's own figures were not measured
pinned against a clean moment the way its codeblocks figures were. **The in-process/real-binary gap
itself is not the finding; `--branch-lines`'s genuine quadratic cost (below) is.**

Going forward, `svn-tags`'s matrix is still recorded from the real binary, one child per run
(`os.wait4`), matching `release-corpus.py`'s own discipline — not because the in-process method was shown
to add overhead, but because the real binary is what a user runs, and what the handoff's own stop
condition ("codeblocks' peak or time is worse than 0.8.1's") is about. **New script:**
`tools/bench/svn-tags-corpus.py <brygge-binary> <brygge-bench-binary> <n>... [--branch-lines]` — `svn-tags`
is synthetic (not a `real.toml` entry), so `release-corpus.py` itself has nothing to measure it from; this
is the same method, for the one scenario it does not cover.

### F-1: `--branch-lines` must take each root's tree by sharing, not by flattening

**The bug:** every revision, `decode_with`'s partition ran `applied.tree.flat_entries()` over the
**whole** repository tree to bucket paths by root, then rebuilt each touched root's relative tree from
that flat list — O(tree size) per revision. `svn-tags`' tree grows by the trunk's file count (K) with
every tag, so this was O(N²·K) over a history of N tags, the same shape D-1 exists to remove, reintroduced
one layer up.

**The fix:** only the roots a revision's own nodes name (`touched`, already derived from node paths, never
from the tree) get a fresh relative tree, looked up directly — `Tree::dir_at`, O(depth) — not rebuilt by
flattening. The live-root set (for detecting a line that just died) comes from the layout containers'
own immediate children (a new `Tree::child_names`, one level only) and the trunk's own liveness, not from
flattening either — still correct for a root killed by its *containing* directory's removal (a new
level-1 test, `deleting_the_whole_branches_container_kills_every_branch_under_it`, covers exactly this: a
root never named directly, only implied by its container's deletion). The copy-created first atom's own
diff now reuses `tree::diff` (made `pub(crate)`) instead of a second, separately-written flattening
diff — the same `Rc::ptr_eq`-skipping walk D-1's main fix already has, not a parallel implementation of
it. `layout_honesty` (both modules) still flattens the final tree once, after the whole decode — O(tree
size) total, not per-revision — checked and left as is.

**Proof — byte-identity, again, every corpus entry, all three modes:**

| Repository | Mode | `cmp` | Exit (old/new) |
|---|---|---|---:|
| wordpress-core | plain | **identical** | 10 / 10 |
| wordpress-core | `--branch-lines` | **identical** | 10 / 10 |
| wordpress-core | `--reconstruct-refs` | **identical** | 10 / 10 |
| codeblocks | plain | **identical** | 10 / 10 |
| codeblocks | `--branch-lines` | **identical** | 30 / 30 |
| codeblocks | `--reconstruct-refs` | **identical** | 30 / 30 |

**`svn-tags`/`svn-tags-plain`, the real binary, one child per run, N = 10 … 2,560 (256× range):**

| N | Mode | Peak (KiB) | Time (ms) |
|---:|---|---:|---:|
| 10 | plain | 33,284 | 146 |
| 40 | plain | 112,028 | 493 |
| 160 | plain | 442,136 | 1,735 |
| 640 | plain | 1,690,316 | 6,605 |
| 2,560 | plain | 6,744,272 | 24,569 |
| 10 | `--branch-lines` | 12,088 | 41 |
| 40 | `--branch-lines` | 26,088 | 105 |
| 160 | `--branch-lines` | 85,732 | 367 |
| 640 | `--branch-lines` | 327,740 | 1,450 |
| 2,560 | `--branch-lines` | 1,288,420 | 7,032 |

**Both about linear, time included, across the full range.** Peak per-tag slope (plain): 2,625 / 2,751 /
2,600 / 2,632 KiB — within 5.7% of each other; a 4× step in N gives a 3.4×–3.8× time step, not the
~12×–20× the unfixed `--branch-lines` showed (below). `--branch-lines`'s own per-tag slope is tighter
still: 467 / 497 / 504 / 500 KiB — within 1.5%; its time steps (2.6×, 3.5×, 4.0×, 4.8× per 4× N) converge
toward linear as N grows past the small-N fixed-cost noise.

**codeblocks, median of three, the real binary, old (0.8.1) vs. new, all three modes:**

| Mode | 0.8.1 time | New time | Speedup | 0.8.1 peak | New peak | Peak cut |
|---|---:|---:|---:|---:|---:|---:|
| plain | 56,110 ms | 5,888 ms | **9.5×** | 1,500,868 KiB | 355,588 KiB | 76.3% |
| `--branch-lines` | 143,404 ms | 5,622 ms | **25.5×** | 1,447,836 KiB | 355,704 KiB | 75.4% |
| `--reconstruct-refs` | 90,008 ms | 6,055 ms | **14.9×** | 1,501,952 KiB | 355,876 KiB | 76.3% |

Artifact bytes match the old binary's exactly in every mode (131,581,296 / 108,071,349 / 131,583,831).
**`--branch-lines` is the biggest winner** — exactly the mode F-1's bug was in; before this fix, batch S's
own first submission could only show the win on the single-spine path, because the real quadratic cost
was hiding in a module that submission never touched.

### F-3: release a kept snapshot after its last use

The first submission's premise — "a revision is kept only if a later copy names it, so there is no last
use to detect" — confused *whether* a snapshot is needed with *until when*: a branch created at r100 from
trunk@99 and later deleted only ever uses r99's snapshot at r100, but without this fix it stays pinned
(holding every directory version r99 had, however much of it a later revision has since replaced) to the
end of the decode regardless. **The fix:** the pre-scan now counts each source revision's uses; a use is
consumed (decremented) as its revision is processed, and the `kept` entry is dropped the moment the count
reaches zero. **Level 1:** `a_snapshot_used_once_is_released_from_kept_right_after_its_last_use` —
`decode_with`'s own `kept` map (not observable any other way, since it is a local) is read back through a
`#[cfg(test)]`-only thread-local probe set right after the main loop; the test proves a once-used
snapshot is gone from `kept` by then, which an unfixed decode (nothing ever removed) would not show.

**Measured, codeblocks, peak RSS, all three modes, with and without the fix (otherwise identical
binaries):**

| Mode | Without F-3 | With F-3 |
|---|---:|---:|
| plain | 354,728 KiB | 356,036 KiB |
| `--branch-lines` | 354,752 KiB | 355,284 KiB |
| `--reconstruct-refs` | 356,096 KiB | 354,948 KiB |

**The gain is nil for codeblocks** (every difference is within measurement noise, under 0.5%) — the
bound is correct regardless (it tightens what `kept` can cost in the shape the handoff describes), but
codeblocks' own peak is evidently not dominated by stale `kept` entries at the moment it peaks. Not
re-measured on a shape designed to show the gain (e.g. a huge branch created and deleted early, then a
long unrelated tail): F-3 only asked for codeblocks' own figure, and this already answers it honestly,
as the handoff's own wording allowed for.

### The usual checks

All green: `cargo fmt --check`; clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`,
`x86_64-apple-darwin`, `+1.87`; `cargo test --workspace --locked` **1,069 passed, 0 failed** on stable,
`+1.87`, and the no-identity run; the deep mutation sweep **1,069 passed, 0 failed**; isolation (ok),
links (ok, 85), the book (ok, no warnings), `cargo deny` (advisories/bans/licenses/sources ok). The
release-tools suite hit one flaky `check-published` case on its first run (the same pre-existing flake
seen in review 168's own CI run, unrelated to this batch's files) — clean, **75/75**, on retry.

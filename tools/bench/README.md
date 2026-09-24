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

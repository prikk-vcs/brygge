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
```

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

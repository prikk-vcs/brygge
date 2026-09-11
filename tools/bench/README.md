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

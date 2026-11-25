# Handoff — RFC 020 batch M: measure first (request 082)

**Governing:** RFC 020 (accepted 2026-11-25), "The release plan", batch M. **Measurement only:** no change to any
published crate. `tools/bench` only. **On `main`.**

**The question, before any change:** where do the Subversion decoder's memory and time go on a copy-heavy history,
and does D-1's shared tree address the large part?

## 0. Evidence plan first (N-3)

- **Everything here is level 3 by nature:** it is a measurement of the real decoder.
- **Level 1:** the new scenario's generator, tested (its dump decodes, with the expected atom and ref counts).

## 1. A new bench scenario: `svn-tags`

- **What it generates:** in `brygge-bench`, a dump with a K-file trunk, then N times "edit one trunk file, copy trunk
  to `tags/tN`". It is 157's shape, generated in-process like the other `svn-*` scenarios.
- **The parameters:** `svn-tags <n>` with K fixed at 2,000. Matrix N = 10, 40, 160. Higher N exceeds 0.8.1's
  ceiling: record where it refuses.
- **What it measures:** run with `run_matrix`'s existing method (one decode per subprocess), in both modes: the plain
  decode and `--branch-lines`.

## 2. The profile, by phase

**Extend `brygge-bench profile` to Subversion**, for codeblocks (`fixtures/corpus/svn/codeblocks.dump`, plain and
`--branch-lines`) and `svn-tags` N=160.

**Separate the memory into what the RFC must change and what it must not:**
- **(a)** the retained snapshots (`kept`): the entry count and an estimate in bytes, at its high-water mark;
- **(b)** the working tree (`prev_tree`, and the per-root trees under `--branch-lines`);
- **(c)** the IR's metadata in the builder (atoms and ops);
- **(d)** the content store (the spill);
- **(e)** everything else: the peak minus (a)–(d).

Use counters printed at the high-water mark from a measurement-only hook in `tools/bench`. If an estimate needs a
size, use `size_of` plus the measured average path length. **Never change the decoder to measure it:** where a count
cannot be had from outside, say so.

**And the time, split three ways:**
- the dump parse;
- `apply_revision`, including its tree clone and its diff;
- the builder.

Coarse timers around the calls the bench can make are enough. Say what each timer covers.

## 3. What the request reports

- **The tables for §1 and §2.**
- **One judgment, with figures:**
  - does (a) + (b) dominate the peak on codeblocks and on `svn-tags`, and does the diff dominate the time? If yes,
    D-1 is confirmed as specified;
  - **if not,** what does dominate, and **stop**. RFC 020 is then amended before batch S.
- **The cost of one tree entry today,** measured. Batch S compares against it.

## 4. Checks

- fmt and clippy on `tools/bench`, on stable and 1.87;
- the bench's own tests;
- `cargo test --workspace` summed, plus the no-identity run. Nothing else changes.

## 5. Request

- **File** `.git-exclude/review-requests/082-measure-first.md`.
- **Push after approval only.**

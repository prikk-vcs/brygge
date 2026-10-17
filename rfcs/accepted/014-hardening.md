# RFC 014 — Hardening (0.4.0): mutation-test every untrusted parser, and prove brygge on real repositories

**Status.** **Accepted (2026-10-17)** by the owner ("Yes. Accepted."), with D-7 answering OQ-1. Handoffs: `rfcs/handoffs/014-hardening/`. Proposed (2026-10-17) by the architect. **Revised (2026-10-17)** after the owner's review of
draft 1, which proposed weekly fuzzing on a nightly toolchain. The owner's points:
- this project has no date windows, so the work must be **trigger-driven**;
- a nightly toolchain is not ordinary, so it needs a good reason, and there is none (D-5);
- "real-world validation" needed explaining (D-4).

The owner's question OQ-1 (the existing advisory check) is answered in D-7.

## Summary

brygge's signature danger is **hostile source input** (threat model T-2/T-8), and its defence is a set of
hand-written, bounds-checked parsers. 0.3.0 added three more. RUSTSEC-2026-0344 then showed what
example-based tests miss: an allocation bug that shipped in four releases, found only by an advisory. 0.4.0
adds two kinds of proof:

1. **Mutation testing of every parser that reads untrusted bytes,** including the IR artifact reader a target
   runs on artifacts it receives. It runs inside the ordinary test suite, on stable Rust, on all four
   platforms, with **no new dependency and no new public feature**.
2. **Real-world validation:** decoding well-known public repositories of each source kind end to end. That also
   yields the import **shapes** brygge offered to send prikk (letter 003).

## Constraints

- **Nothing a user installs changes:** no new dependency, no cargo feature, no new public API. A user reading
  brygge's docs or crates.io page sees nothing to wonder about.
- **Stable Rust only,** on the four CI platforms, like every other gate.
- **Trigger-driven, never time-driven:** work runs because something changed, or because a release is being
  cut.
- **Every finding becomes a permanent regression test.**

## Decisions

### D-1 — Mutation tests, in each crate's own test suite

- **One test module per untrusted parser,** written as ordinary `#[cfg(test)]` code, so it is invisible to
  users:

  | Parser | What a mutated input is fed through |
  |---|---|
  | CVS RCS `,v` | `parse_rcs`, then every revision's content (one pass and per revision) |
  | SVN dumpstream | the dump parser, and a whole decode of small dumps |
  | svndiff | `svndiff::apply` against a base |
  | hg revlog | the index parse, `decompress` (zlib, zstd), and delta chains |
  | hg metadata | the changelog and manifest entries, `fncache`, obsstore, bookmarks, phaseroots |
  | IR artifact | `brygge_ir::artifact::from_bytes`, then the `verify --internal` checks |

- **The seeds** are the crate's existing valid fixtures, and later D-4's real files.
- **The mutations are format-aware, not only random:**
  - byte flips, inserts, deletes and splices between seeds;
  - **targeted edits where bombs live:** length and count fields, varints, delta and window sizes, revision
    numbers, offsets set to 0, to the maximum, and to off-by-one values around every declared ceiling.
- **What every mutated input must give:** `Ok`, or a typed error. Never:
  - a panic;
  - a hang (each input runs under a per-input time budget);
  - an output larger than the parser's declared ceiling.
- **Determinism:** a small in-repo pseudo-random generator, seeded from the test name. A failure prints the
  seed and the iteration, and reproduces exactly.
- **The generator is shared test code,** not a dependency. Where crates share it, it lives in one place (a
  `#[cfg(test)]` module per crate, or a `publish = false` dev-only crate). The handoff settles which.

### D-2 — Two depths

- **Quick,** in every `cargo test`: a few hundred mutations per seed, which takes seconds. It runs wherever the
  suite runs, on all four platforms.
- **Deep:** the same tests with a large iteration count, chosen by an environment variable
  (`BRYGGE_MUTATION_DEPTH=deep`).
  - It runs **under a memory cap** on Linux (an address-space limit for the test process, set by the workflow,
    about 2 GiB). So an allocation that escapes a ceiling, the RUSTSEC-2026-0344 class, kills the run and
    fails it, even when the parser would have returned in time.
  - The cap comes from the operating system, so brygge needs no allocator hook and keeps `forbid(unsafe_code)`.

### D-3 — The triggers (no dates)

| Trigger | What runs |
|---|---|
| **Every push** | the quick depth, as part of the ordinary suite |
| **A change to a parser's code, or to `Cargo.lock`** (a dependency a parser uses can change its behaviour; that is how 0344 entered) | the **deep** depth, in a CI job filtered on those paths |
| **A release cut** | the deep depth, run and recorded as a step of the cut prep, alongside the other gates |
| **A finding** | its input becomes a regression test that runs in every `cargo test` from then on |
| **On demand** | the deep job, through `workflow_dispatch` |

When no parser and no dependency changes, the deep job never runs, and it costs nothing.

### D-4 — Real-world validation corpus

- **Today every test input is synthetic:** small repositories our test scripts create. They cover what we
  thought of; real projects carry what we did not (decades-old tool versions, odd encodings, huge or messy
  histories). So D-4 uses **existing public open-source repositories,** not dummy projects. For example:
  - Mercurial's own repository, for hg;
  - a mid-size Git project;
  - an SVN project dumped with `svnrdump`;
  - a historic CVS module.
- **A pinned manifest,** `tools/corpus/real.toml`, with two or three repositories per source kind, each at a
  pinned revision. Criteria:
  - publicly reachable, with a clear license;
  - decoded in minutes;
  - together covering the features brygge claims.
  - The dev team proposes the list in its request, and the architect approves it. **No owner decision is
    needed.**
- **For each,** a script records:
  - `decode` (both flags);
  - `verify`;
  - `verify --against-source`;
  - time and peak;
  - **the shape:** commits, files per commit, and the tree size over depth.
- **When it runs:** by hand, triggered by a release cut (the results are recorded in the cut prep), and by a
  change to a decoder (the reviewer may ask for it). Never on a schedule.
- **Any mismatch is a defect.** The shapes are what brygge sends prikk once its import work is scheduled.

### D-5 — Why not a coverage-guided fuzzer (draft 1's choice)

- **Coverage guidance is the one real advantage:** libFuzzer can find deep inputs that random mutation rarely
  reaches.
- **But the standard tool, `cargo-fuzz`,** needs a nightly compiler, and runs **only on Unix-like systems, not
  Windows** (its README). brygge proves every gate on four platforms with stable Rust.
- **`afl.rs`** needs the AFL++ C toolchain and root-level system configuration.
- Neither belongs in an ordinary, reproducible gate. D-1's format-aware mutations aim directly where
  RUSTSEC-2026-0344 and its kind live (sizes and counts), on the ordinary toolchain.
- Coverage guidance stays a possible later addition, if mutation testing leaves a known gap. It is not adopted
  now.

### D-6 — Release

0.4.0 ships when:
- every D-1 module exists, and passes at both depths;
- the deep job has run once on a parser change, its findings fixed;
- the D-4 results are recorded.

It also carries the unreleased broken-pipe fix (`f7a16bf`). The threat model's revision (v0.8) records
mutation testing as a verification control, making the C-2d ceilings **tested** claims.

### D-7 — The one poll of an external source, named so it cannot be mistaken for a schedule (OQ-1)

- **Kept, and reframed.** The owner allowed keeping the advisory check, with one concern: that someone would
  read it as precedent for date-driven work. So:
- **The job is named after its trigger, not its frequency:**
  - the file `security-audit.yml` becomes **`advisory-watch.yml`**;
  - the workflow name "Scheduled Security Audit" becomes **"Advisory watch"**.
- **Its header comment says what it is, and what it is not:**
  - it watches for one external event, a new RustSec advisory, which nothing in this repository can signal,
    and for which GitHub offers no event. So it **polls**. The cron line is the polling interval, not a task
    cadence;
  - **it is brygge's only scheduled workflow.** Every other piece of work runs because something changed, or
    because a release is cut;
  - adding another scheduled workflow requires an RFC.
- **`GOVERNANCE.md` gains the rule,** under how work is organised:

  > **Work is triggered, never scheduled.** Jobs run on a push, a change to the paths they guard, a release
  > cut, a finding, or on demand. The single exception is the advisory watch, which polls the RustSec database
  > because a new advisory is an external event with no trigger here. A new scheduled job needs an RFC.

- **No prose calls it "weekly" any more:**
  - the threat model's C-4d and RR-1 (at the 0.4.0 cut);
  - RFC 012 D-7, through an amendment note pointing here.
  - Released CHANGELOG entries stay as they were written, since they are history.
- **The polling interval is unchanged.** It is an implementation detail, stated only in the workflow's cron
  comment.

## Order and proof

0. **Batch A (small, first):** D-7's rename, header comment, `GOVERNANCE.md` rule, and RFC 012 note.
1. **Batch M:** the mutation modules and the generator, both depths, and the CI deep job with its path filter
   (proven on a topic branch).
2. **Batch R:** the D-4 corpus, its script and the results.

They are independent. Every finding gets its own fix and review request.

## Consequences

- brygge's parsers gain an adversarial line of proof on the ordinary toolchain, on every platform.
- The ceilings become tested claims.
- brygge can answer prikk's offer with measured shapes, not invented ones.

# Handoff — 0.2.0, batch A: measure before building (RFC 010 OQ-A)

**Governing:** RFC 010 (bounded memory), accepted, as amended 2026-09-24 ("Plan for 0.2.0", increment 5,
OQ-B resolved). **Theme:** 0.2.0 Scale (`ROADMAP.md`).

**Why first.** RFC 010 gates every remaining increment on evidence. The harness exists (`tools/bench`),
but it has no scenario that exercises two of the three targets:
- **Git** (increment 5): there is no Git scenario at all;
- **CVS delta chains** (increment 3): today's `cvs` scenario is single-revision files, which have no
  chains.

This batch adds the scenarios and records the **baseline on 0.1.1**, so that each increment in batch B is
an A/B against it. **No decoder code changes here.**

---

## 1. New scenarios in `tools/bench`

Each is synthetic, deterministic, generated in a temporary directory, and measured one decode per
subprocess, as the existing ones are. Pick scales so the largest completes in minutes on a developer
machine, and state the machine in the results.

| Scenario | Shape | Targets | Suggested scales |
|---|---|---|---|
| `git-commits <n>` | a fixed tree of ~500 files; n commits, each editing one file | increment 5: the snapshot cache grows with distinct root trees, and each commit makes one | 1k, 5k, 20k |
| `git-content <n>` | few commits; n sizeable files | the IR floor for Git (peak should track content, not history) | 1k, 5k |
| `cvs-revs <n>` | ~20 `,v` files, each with n trunk revisions (each revision edits a few lines) | increment 3: the O(revisions²) per-file reconstruction | 200, 1k, 5k |
| `svn-dump <n>` | an SVN dump of n revisions over a few hundred files, with sizeable content | increment 2: the whole parsed `Dump` held beside the IR | 1k, 5k, 20k |

- **Generate Git corpora with `git fast-import`,** which is fast and deterministic with fixed identities
  and dates, not n `git commit` calls. The bench already needs no new dependency; keep it that way.
- **Generate `cvs-revs` by writing the `,v` files directly** (RCS format: a trunk chain of reverse diffs
  from `head`), as the existing `cvs` scenario writes its files. The file must be valid RCS: decode it
  with brygge, and check each revision's content against the text the generator intended, in the
  scenario's self-check.
- **Each scenario prints the IR stats** (atoms, blobs, content bytes) next to peak and time, so the A/B can
  show the IR is unchanged.

## 2. The baseline

- Run the full matrix on `main` at the time (0.1.1 code), release build, same machine, and **record it**
  in `tools/bench/README.md`, under a new section "0.2.0 baseline (0.1.1)": one table per scenario (scale,
  peak, time, IR stats), with the commit hash and the machine.
- **Interpretation, one line per scenario:** does peak grow faster than content? That is the evidence each
  increment in batch B must reduce.
- Keep increment 1's existing results table as it is.

## 3. Proof

- The scenarios run; each self-check passes.
- The baseline tables are in the README.
- The usual gates pass. `tools/bench` is `publish = false` and dev-only; clippy covers it.

## 4. Non-change scope

- Every decoder and `brygge-ir`.
- CI: the bench is not run in CI (timing on shared runners is noise).

## 5. Review request

File `.git-exclude/review-request/022-scale-measurement.md` with the standard sections and the baseline
tables. After approval, commit and push, and append the CI result.

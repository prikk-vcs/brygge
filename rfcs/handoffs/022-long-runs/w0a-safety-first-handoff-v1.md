# Handoff — RFC 022 batch W0a: the safeguards first, after incident 094 (request 095)

**Governing:**
- reviews 203 and 204 (the incident, the horizontal investigation, the measures P-1 to P-7);
- HANDOFF §7a (the owner's rulings of 2026-11-28).

**Nothing in this batch spawns a watchdog,** and it contains no new spawning code. It adds only guards, checks and
limits, so that W0 can follow safely.

## 0. Before anything else: make the working tree safe

- The uncommitted W0 code on `batch-w0-watchdog` is the code that caused incident 094. **Do not run, build-and-test or
  commit it.**
- **Set it aside:** `git stash push -u -m "w0-wip, incident 094: do not run"`. Then check that `git status` is clean.
- **Start W0a from `main`** (at `486e18c` or later, which holds HANDOFF §7a and this handoff), on the proving branch
  `prove/w0a-safety`.

## 1. The measures in this batch

- **P-1, the parts that only refuse** (no watchdog exists yet; they are written into `crates/brygge/src/watchdog.rs`
  as a module with no spawn):
  - **(b):** a `fn may_spawn() -> bool`, false when `BRYGGE_INTERNAL_WATCHDOG` is in this process's own environment;
  - **(c):** under `cfg(test)`, the future `Watchdog::start()` is a function that returns
    `Err(ErrorKind::Unsupported)` and contains no `Command`. Write that `cfg(test)` form now, and a level-1 test that
    asserts it. W0 adds the non-test form later.
- **P-2:** the three `brygge-bench` self re-runs (`real.rs` `run`, `main.rs` `run_matrix`, `profile.rs`):
  - **compiled out under `cfg(test)`:** the test build gets a form that panics with "a self re-run is never compiled
    into a test build", and contains no `Command`;
  - refused when the bench's own child marker is in the environment (add one if there is none);
  - **a level-1 test for each.**
- **P-3:** `tools/check-self-exec.sh`, run by CI's `links · book · release tools` job.
  - **The rule:** it lists every `current_exe` in `crates/`, `tools/` and `examples/`, and fails on any site not in its
    own reviewed list.
  - **Each listed site carries one line naming its guard:**
    - the `brygge-ir` test: `--exact` and its marker;
    - the three bench sites: P-2;
    - `brygge_bin`: no spawn;
    - the future watchdog: P-1.
  - **A self-test:** a planted unlisted `current_exe` makes it fail. Do not commit the planted line.
- **P-4:** `timeout-minutes` on every job of every workflow.
  - **The defaults:** 30 for the test and build jobs, 15 for the rest;
  - **`release.yml`'s AUR build:** 30, as a build job. (This said "measured, plus 50%" first; withdrawn by review
    205 R-4, since that job runs after the irreversible crates.io publish);
  - a level-2 check in `tools/test-release-tools.sh`: every job in every workflow has `timeout-minutes`.
- **P-5:** `tools/contained-test.sh <cargo test arguments…>`.
  - **Where `systemd-run --user --scope` works,** it runs `cargo test …` with `-p TasksMax=256 -p MemoryMax=8G`.
  - **Otherwise,** it runs a subshell with `ulimit -u 512 -v 8000000`. It says which limit it used on its first line,
    and never runs unlimited.
  - **A self-test:** a command that tries to start more than the limit's processes fails inside the limit. Write it
    as a shell loop that stops itself at 300 processes, so that it can never exceed the limit it tests. **That
    self-test runs in CI only** (HANDOFF §7a's first row).
- **P-6:** the tests remove what they create in the system temporary directory. Today they leave
  `brygge-cli-encode-git-check-pass-*.ir`.
  - **A check,** in `tools/contained-test.sh`'s own wrapper: it lists the temporary directory's brygge-named entries
    before and after, and fails if any were added;
  - fix the tests that leave them.
- **P-7:** already in HANDOFF §7a. Nothing to build.

## 2. How this batch is run (HANDOFF §7a)

- **P-2 changes spawning code,** and P-5's self-test starts processes, so the **first runs are in CI:**
  - push the work-in-progress to `prove/w0a-safety` (the one exception to "push only approved");
  - CI runs it, including Windows and macOS.
- **Only after CI is green,** run locally, and only through `tools/contained-test.sh`.
- **Never bare `cargo test`,** from this batch on.

## 3. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- **the test suite through `tools/contained-test.sh`,** summed, and the no-identity run inside it too;
- `tools/check-self-exec.sh`;
- the timeout check;
- links, the book, the release tools, `cargo deny`.

## 4. Request

- **File** `.git-exclude/review-requests/095-w0a-safety.md`, with the CI run of `prove/w0a-safety` and the contained
  local run.
- **After the architect's approval,** `main` is fast-forwarded from the branch.
- **Then W0 resumes** from the stash, with P-1(a) and (d) added. **Its first runs are in CI** on `prove/w0-watchdog`.

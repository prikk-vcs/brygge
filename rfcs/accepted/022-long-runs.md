# RFC 022 — Long runs: progress, and a clean stop (0.11.0)

**Status.** **Accepted (2026-11-28)** by the owner: "Yes. Accepted." Handoffs: `rfcs/handoffs/022-long-runs/`.

**Refinement D-1a (2026-11-28, after incident 094; owner-accepted).** W0's first implementation re-ran
`std::env::current_exe()` from inside `decode`. Under `cargo test` that is the test harness, so the unit tests became
a fork bomb that hung the owner's machine twice. D-1's watchdog is therefore started with four guards:
- (a) only `main` starts it, and the commands receive a guard that unit tests pass disabled;
- (b) a process carrying the watchdog marker never starts another;
- (c) under `cfg(test)` the start is compiled out and returns `Unsupported`;
- (d) the child must answer `ready <pid>` within 2 seconds, or it is killed and the run continues without a watchdog.

**How the work runs safely** is in HANDOFF §7a: spawning code is proven in CI first on a `prove/<batch>` branch;
every local test run goes through `tools/contained-test.sh`; CI checks every self re-run against a reviewed list;
every CI job has a timeout. Batch **W0a** puts those safeguards in place before W0 resumes.
Proposed (2026-11-27) by the architect. It follows the owner's rulings of
2026-11-26 and 2026-11-27:
- 0.11.0's theme (198);
- no `unsafe`, so no signal-handling dependency (199);
- the watchdog design, accepted after the verification that overturned the lifeline (200).

## Summary

It builds the two external-design requirements that were never built:
- **OP-02,** "Large imports show progress and are cancellable; a stop is a clean partial, never an ambiguous target
  state";
- **OP-03,** "Honesty is emitted continuously, not only at the end".

**Measured today (198):** stopping a long run leaves a hidden temporary artifact (786 MB for fish-shell), or a
half-written repository with its `git fast-import` still running; and a long run is silent.

**After this RFC:**
- a stop removes what brygge made, and says so in one sentence;
- a stop that no program can catch is named on the next run;
- a person watching a terminal sees progress, with the running honesty counts.

**The cost:** no new dependency, no `unsafe`, `std` only. Every existing output, exit code and artifact is
unchanged.

## Decisions

### D-1 — The watchdog

- **brygge stays an ordinary foreground process.** Its exit codes, Ctrl-C, Ctrl-Z, `fg`, pipes and signals behave
  as for any program.
- **The commands that create temporary paths** (`decode`, `encode git`, `transform`, `verify --against-source`) start
  one **watchdog**: the same binary, through an internal entry (D-2), in a **new process group**.
  - It uses `std::os::unix::process::CommandExt::process_group(0)` on Unix, and
    `std::os::windows::process::CommandExt::creation_flags(CREATE_NEW_PROCESS_GROUP)` on Windows;
  - its stdin is a pipe from brygge;
  - its stdout is null; its stderr is brygge's.
- **Registration:** as brygge creates a temporary path, it writes `temp <path>` to the pipe. When the path is renamed
  into place or removed, it writes `done <path>`.
  - **The paths are percent-encoded** (the machine-output alphabet), because a path may contain a newline;
  - a line that does not parse makes the watchdog refuse that line, and touch nothing.
- **When the pipe closes, brygge has ended,** however it ended: Ctrl-C, SIGTERM, a closed terminal, a panic, an
  abort, the out-of-memory killer or `kill -9`. The watchdog then removes every path still registered (D-3), **then**
  writes one sentence to stderr, and exits:
  `interrupted: removed <path>…; nothing was written`
- **On a normal end,** nothing is registered and the watchdog exits silently.
- **The read-only commands** (`inspect`, `verify` without `--against-source`, `encode --list`, `--help`,
  `--version`) start no watchdog.

### D-2 — The internal entry is not a command

- **The watchdog is started** by brygge with an internal environment marker, `BRYGGE_INTERNAL_WATCHDOG=1`, and a
  pipe on stdin.
- **If either is missing,** or stdin is a terminal, it refuses with exit 2: "this is an internal entry of brygge, not
  a command".
- **It is never in `--help`, the guides' command lists or the machine output.** It is documented once, in
  `HANDOFF.md`, as internal.

### D-3 — What the watchdog may remove (safety)

**It removes a registered path only if all of these hold:**
- **its name is one of brygge's own temporary forms, carrying brygge's process id:**
  - `.<name>.brygge-tmp-<pid>` (an artifact being written);
  - `<dir>.partial-<pid>-<n>` (`encode git`'s repository);
  - `.brygge-transform-<pid>-…` (a transform's output);
  - `encode git`'s private `HOME` and `GIT_CONFIG_GLOBAL` (in the system temporary directory, with their own
    `<pid>`);
- `symlink_metadata` shows a regular file, or a directory, and **never a symbolic link,** which is not followed;
- **it is the path brygge registered,** compared exactly, and never a prefix or a match.

**Otherwise it removes nothing,** and its sentence names the path instead: "not removed: <path> (<reason>)".

**`encode git`'s partial repository:**
- `git fast-import` is in brygge's process group, so Ctrl-C stops it as well;
- if brygge alone was killed, `fast-import` sees end-of-file and exits by itself;
- the watchdog removes the directory **once its contents stop changing** (no change in 1 second, checked for at most
  30 seconds), and otherwise names it, as above.

### D-4 — The stops no program can catch: named on the next run

- **The cases:** the whole process tree killed (brygge and its watchdog), a power loss, or a closed Windows console
  (D-7). They can leave a temporary path.
- **What the next run does:** the next `decode`, `encode git` or `transform` that writes into the same directory
  looks for brygge's own temporary forms there (D-3), and names each one on stderr:
  `found <path> (<size>), a temporary file of brygge process <pid>; if no brygge is running, remove it with: rm <path>`
  (`del` on Windows).
- **The wording claims no more than brygge knows.** `std` cannot tell portably, without `unsafe`, whether another
  process is alive, so brygge never says "abandoned".
- **It never deletes such a path:** brygge did not make it in this run.

### D-5 — Progress (OP-02)

- **Where:** on stderr, **only when stderr is a terminal** (`std::io::IsTerminal`). It is never written in a pipe, a
  log or machine output, so nothing that reads brygge's output changes.
- **There is no flag:** a terminal is the only place it helps.
- **What:** one line, rewritten in place at most ten times a second. It holds:
  - the phase, and the count so far, against the total where it is known. For example,
    `decode: 12,345 of 23,270 commits`, `decode: writing the artifact, 412 MB`, `encode git: 9,000 of 23,270 commits`,
    `check: 4,100 of 23,270 commits`;
  - where the total is not known, the count alone.
- **The line is cleared** before any other stderr output, and before the final report, so the report reads as it does
  today.

### D-6 — Honesty while the run goes (OP-03)

- **The progress line carries the running counts** of what is recorded so far as dropped and as flagged, for
  example `…, 3 dropped, 1 flagged`, in the same words as the final report.
- **The final report is unchanged,** and remains the authority.

### D-7 — Exit statuses, and Windows, stated

- **An interrupted brygge's exit status is the system's own:** 130 after Ctrl-C and 143 after SIGTERM, on Unix. On
  Windows, the status Ctrl-C gives.
  - **40 stays reserved and unused.** `machine-output.md` says so, together with which status a stop gives.
- **Windows:** the watchdog is **attached** to the console, in its own process group, so that Ctrl-C does not reach
  it and its sentence can be shown.
  - **Closing the console ends every attached process,** the watchdog included, so that case falls to D-4;
  - `DETACHED_PROCESS` was weighed. The watchdog would survive a close, but its sentence could not be shown, which is
    a silent action, so it was rejected.

### D-8 — Housekeeping

- **`HANDOFF.md` §9:** remove the lines that are no longer true:
  - "Peak memory is O(content)… nothing streams" (untrue since 0.5.0);
  - "No progress reporting or cancellation", which this RFC answers.
- **The test suite removes everything it creates** in the system temporary directory. Today it leaves files named
  `brygge-cli-encode-git-check-pass-*.ir`. A level-1 check lists the temporary directory before and after
  `cargo test`.

## Evidence

**Batch W0, the platform proof, comes first and gates the rest.** An integration test starts a real `brygge decode`
on a fixture large enough to be stopped mid-write, then stops it:
- **on Linux and macOS:** `kill -INT`, then `kill -9`, through the system's `kill` command;
- **on Windows:** `taskkill /F`.

The test asserts that the temporary path is gone and the sentence was printed. **It runs in CI on all three systems.**
A real Ctrl-C on Windows cannot be generated from `std`, so the request states that case as not proved, rather than
claiming it.

**If any system disproves the design, the RFC returns to the owner before W1.**

**After W0:**
- **level 1:**
  - the protocol's encoding and parsing;
  - D-3's every refusal (a symbolic link, a foreign name, a prefix match);
  - the progress line's formatting;
- **level 2:**
  - each command stopped mid-run;
  - Ctrl-Z and `fg` through a pseudo-terminal under an interactive shell, as in 200;
  - the next run naming a leftover;
  - **a normal run leaves no process and prints exactly today's output;**
- **level 3:**
  - fish-shell `decode` and `encode git` stopped mid-write, by hand, nothing left, as measured in 198;
  - **every corpus artifact byte-identical to 0.10.0** (within the version-stamping rule of review 174).

## Threat model

`brygge-03` is updated:
- **C-7:** a stop removes the temporary paths;
- **C-8:** cancellation is built, and progress is built;
- **TB-9:** brygge and its own watchdog. The watchdog trusts nothing but its pipe, and D-3 bounds what it can remove;
- **RR-total-memory:** unchanged.

## The release plan

| Batch | What |
|---|---|
| **W0** | the platform proof (above) |
| **W1** | D-1 to D-4, D-7 |
| **P** | D-5 and D-6 |
| **H** | D-8 |
| **cut** | — |

## Reviewed against the owner's philosophy

| Clause | Where it is met |
|---|---|
| clean | `std` only, with no dependency and no `unsafe`. brygge stays an ordinary process. One silent helper, started only where it is needed |
| safe and secure | the watchdog removes only what brygge registered, in brygge's own temporary forms, never through a link (D-3). The next run never deletes; it names (D-4) |
| robust | it covers every death of brygge, `kill -9` and the out-of-memory killer included. A tree-wide stop is named on the next run. W0 proves the design on all three systems before anything is built on it |
| sophisticated | the cleanup is triggered by brygge's own end, with no polling and no global state. Progress appears only where someone is looking |
| no confusion | job control works as for any command. The exit status is the standard one. One sentence says what a stop did, and the next-run message claims no more than brygge knows. Normal output is unchanged |

## Open questions for the owner

None. The design is ruled (200). The details above are the architect's, each tested in the table. If any reads
wrongly, say so before acceptance.

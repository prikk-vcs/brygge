# Handoff — RFC 022 batch W0, v2: the watchdog, for `decode`, built to D-1a, and its proof on three systems (request 096)

**This replaces v1.** v1 told the implementer to spawn `current_exe()` from inside `decode`, which caused incident 094
(review 203). v2 keeps v1's design (D-1, D-2, D-3). It changes:
- **who** may start the watchdog;
- **how** a start is refused and confirmed;
- **where** the first runs happen.

**Governing:**
- RFC 022 D-1 to D-3, and refinement D-1a (in RFC 022's status);
- reviews 203 to 206;
- HANDOFF §7a.

W0a's guards are on `main` (`fb4a885`): `watchdog.rs`'s `may_spawn` and the `cfg(test)` `start`,
`tools/contained-test.sh` and `tools/check-self-exec.sh`.

**W0 gates the release:** nothing else of RFC 022 is built until W0 shows the design works on Linux, macOS and
Windows.

**Scope, deliberately narrow:**
- the watchdog;
- its internal entry;
- the removal rules;
- **only `decode`** registering its temporary artifact.

`encode git`, `transform`, `verify --against-source`, the next-run naming (D-4), progress and the rest are W1, P and H.

## 0. Where the work starts, and where it first runs (HANDOFF §7a, first row)

- **Branch `prove/w0-watchdog` from `main` at `fb4a885` or later.**
- **The stash is reference only.** Do not `git stash apply` it: its `watchdog.rs` conflicts with `main`'s, and its
  `run_decode` holds the incident's call. Read what you need from it with
  `git show 'stash@{0}^3:crates/brygge/src/watchdog.rs'` (and `git show 'stash@{0}:<path>'` for the tracked files),
  and write the code onto `main`'s guards. Drop the stash once W0 is approved.
- **This is spawning code, so its first runs are in CI.** Push work-in-progress to `prove/w0-watchdog` and let CI run
  it, including Windows and macOS.
  - **Before CI is green:** building and `clippy` locally are fine. Running anything that can start the watchdog is
    not: no tests, no `brygge decode`.
  - **After CI is green:** run locally, and only through `tools/contained-test.sh`. The hand-run terminal script (§3)
    runs inside a `systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -p MemorySwapMax=0` of its own.
- **After any run that hangs or exhausts the machine:** stop, report at once, and do not re-run until the cause is
  fixed in the code.

## 1. Evidence plan first (N-3)

- **Level 1:**
  - **the protocol:** encode, then parse, with paths holding a newline, `%`, spaces and non-ASCII;
  - **a malformed line** is refused, and nothing is touched;
  - **D-3's every refusal:** a symbolic link where the registered path was; a name not in brygge's temporary forms; a
    temporary form carrying a different process id; a path that only shares a prefix with the registered one. Each
    leaves the path untouched and names it;
  - **D-1a:**
    - a unit-test build's `start` is `Unsupported` (already on `main`; keep it);
    - `Guard::disabled()` does nothing on `register`, `release` and `finish`;
    - the handshake's parser accepts only `ready <the child's own pid>`.
- **Level 2, in CI on all three systems (§4):**
  - a real `brygge decode`, stopped part-way;
  - a decode that is not stopped;
  - **the handshake refusing a child that is not the watchdog** (§2).
- **Level 2, by hand, on Linux, after CI is green:** Ctrl-C and Ctrl-Z through a pseudo-terminal under an interactive
  `bash` with job control, as review 200 did. Commit the script under `tools/`, and report its output. It is not a CI
  test.
- **What stays unproved, said so:** a real Ctrl-C on Windows. `std` cannot generate one, and the request must say "not
  proved", never "works".

## 2. The parent side (D-1, D-1a)

**(a) Only `main` starts a watchdog.**
- **The type:** `pub enum Guard { Active(Watchdog), Disabled }`.
- **`main`,** after parsing, and only for `decode` in W0, calls `Watchdog::start()`. On `Ok`, it passes
  `Guard::Active`; on `Err`, it reports once on stderr (`note: no clean-stop watchdog for this run: <reason>`) and
  passes `Guard::Disabled`. The run then goes on exactly as 0.10.0's would.
- **`run_decode` takes `&mut Guard`,** and contains no call to `start`. **Every unit test passes `Guard::disabled()`.**
- **The methods on `Guard`:**
  - `register(&mut self, path: &Path)` writes `temp <encoded>`;
  - `release(&mut self, path: &Path)` writes `done <encoded>`;
  - `finish(self)` closes the pipe and waits for the watchdog's exit.
  - On `Disabled`, each does nothing.
  - **A write that fails** (the watchdog gone) is reported once, on stderr, and the command goes on. Losing the
    watchdog must never fail a decode that would otherwise succeed.

**(b) The recursion refusal.** `start()`'s non-test form returns an error at once, before any `Command` exists, when
`may_spawn()` is false: "refused: this process is itself a watchdog".

**`Watchdog::start()`'s non-test form** (`#[cfg(not(test))]`; the `cfg(test)` form on `main` stays as it is):
- it spawns `current_exe()` with `BRYGGE_INTERNAL_WATCHDOG=<this process's id>`;
- stdin and stdout piped, stderr inherited;
- **its process group:** on Unix, `process_group(0)`; on Windows, `creation_flags(CREATE_NEW_PROCESS_GROUP)`, the
  constant `0x200`, named, with its documentation link.

**(d) The handshake:**
- **The wait:** `start()` reads the child's first stdout line on a helper thread and waits for it with
  `mpsc::Receiver::recv_timeout(2 s)`.
- **Accepted only:** `ready <pid>`, where `<pid>` equals `child.id()`.
- **Anything else** (a timeout, end of file, another line or another id): `Child::kill`, then `wait`, then return an
  error naming what was seen (for example `the watchdog did not answer within 2 s`). `main` then continues without
  one.
- **A level-2 test:** start the real binary as the watchdog with stdout's first line forced wrong is not possible
  from outside. Instead, unit-test the parser, and add one integration test that points the start at a program that
  never answers. Put the program behind a `#[cfg(not(test))]`, test-only environment variable read by `start()`, such
  as `BRYGGE_TEST_WATCHDOG_EXE`, honoured only in a debug build (`cfg!(debug_assertions)`). The test asserts that the
  child was killed within 3 s and that the decode succeeded. **Say in the request** whether you chose this or
  something better, and why.

**The encoding:** the machine-output alphabet's percent-encoding (`display::machine_value`), so one encoding serves
both.

**`tools/check-self-exec.sh`:** `crates/brygge/src/watchdog.rs` moves from 0 to 1, with the note "P-1: only `main`
calls `start`; refused under the marker; compiled out under `cfg(test)`; `ready` handshake within 2 s".

## 3. The watchdog side, and the internal entry (D-2, D-3)

- **`main`, before argument parsing:**
  - with `BRYGGE_INTERNAL_WATCHDOG` set, it runs the watchdog loop, and **never reaches command dispatch**;
  - **a marker that is not a decimal process id,** or a stdin that is a terminal (`IsTerminal`), is refused with exit
    2: "this is an internal entry of brygge, not a command";
  - **the loop contains no `Command`.** A watchdog never starts anything.
- **The loop:**
  - first write `ready <own pid>` and flush;
  - read lines, keeping the registered paths in order;
  - **at end of file,** for each path still registered, apply D-3:
    - the exact path;
    - brygge's temporary form `.<name>.brygge-tmp-<pid>`, where `<pid>` equals the marker's value;
    - `symlink_metadata` shows a regular file;
  - then remove it, or name it;
  - then print the one sentence, if anything was registered;
  - then exit 0.
- **The sentence:** `interrupted: removed <path>; nothing was written` (for several paths, `removed <p1>, <p2>`). A
  path not removed: `interrupted: not removed: <path> (<reason>)`.

## 4. The proof in CI (W0's gate)

A new integration test, `crates/brygge/tests/stop_integration.rs`, on all three systems. It runs the real binary,
`CARGO_BIN_EXE_brygge`.

1. **The fixture:** generate, in the test, an SVN dump with content large enough that the artifact write takes
   visibly long, for example 40 files of 4 MiB of pseudo-random bytes. It is not committed, and it is removed
   afterwards (P-6).
2. **Start** `brygge decode svn <dump> --out <dir>/a.ir` as a child process.
3. **Poll** `<dir>` every 10 ms, for at most 60 s, until `.a.ir.brygge-tmp-<pid>` exists. That is the write phase.
4. **Stop the child:**
   - Linux and macOS: `kill -INT <pid>` through the system's `kill` command, in one test, and `kill -9 <pid>` in
     another;
   - Windows: `taskkill /F /PID <pid>`.

   Use the commands themselves, never a library.
5. **Assert, within 10 s:**
   - the temporary path is gone;
   - `<dir>` holds no `a.ir`;
   - the child's stderr ends with the sentence naming that path.
6. **Assert there is no watchdog left:** on Linux, no process carrying the marker. Elsewhere, observe the watchdog
   child's own exit through a second, test-only pipe it inherits.

**Plus:**
- **the normal case:** a decode that is not stopped leaves no process, and its stderr is byte-identical to 0.10.0's
  for the same fixture;
- **the handshake refusal** (§2(d)).

**If the poll times out** (the write was too quick to catch), the test fails. It never passes vacuously.

**If any system's test fails, stop and report.** RFC 022 then returns to the owner.

**Withdrawn from review 204 §3:** "integration tests start brygge under `prlimit --nproc=64`". `RLIMIT_NPROC` counts
every process the user already has, so on a desktop it would refuse to start at all. The bounds are:
- the scope, locally;
- the runner and `timeout-minutes`, in CI;
- in the code: the recursion refusal, the handshake, and a watchdog that never spawns.

## 5. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- **`tools/contained-test.sh --workspace --locked`,** summed, and the no-identity run (HANDOFF §7's command), after
  CI is green;
- `tools/check-self-exec.sh`, the isolation check, links, the book, the release tools, `cargo deny`;
- the deep mutation sweep is not due (no parser changes), unless `mutation.yml`'s filter says so.

## 6. Request

- **File** `.git-exclude/review-requests/096-w0-watchdog.md`, with:
  - the evidence plan;
  - the CI run of `prove/w0-watchdog`;
  - the contained local run;
  - the hand-run terminal output;
  - an exact list of everything that ran locally, and under which limit.
- **The CI run is W0's gate:** the architect reads the Windows and macOS logs before W1 is handed off.

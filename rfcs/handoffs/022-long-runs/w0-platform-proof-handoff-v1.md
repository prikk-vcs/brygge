# Handoff — RFC 022 batch W0: the watchdog, for `decode`, and its proof on three systems (request 094)

**Governing:** RFC 022 D-1, D-2 and D-3, accepted 2026-11-28. **W0 gates the release:** nothing else of RFC 022 is
built until W0 shows the design works on Linux, macOS and Windows. **On a topic branch,** `batch-w0-watchdog`,
because this is process and platform code. Push only after approval, and name the branch.

**Scope, deliberately narrow:**
- the watchdog;
- its internal entry;
- the removal rules;
- **only `decode`** registering its temporary artifact.

`encode git`, `transform` and `verify --against-source`, the next-run naming (D-4), progress, and the rest are W1, P
and H.

## 0. Evidence plan first (N-3)

- **Level 1:**
  - the protocol: encode, then parse, with paths holding a newline, `%`, spaces and non-ASCII;
  - a malformed line refused, and nothing touched;
  - **D-3's every refusal:**
    - a symbolic link where the registered path was;
    - a name not in brygge's temporary forms;
    - a temporary form carrying a different process id;
    - a path that only shares a prefix with the registered one.

    Each leaves the path untouched and names it.
- **Level 2, a real `brygge decode`, stopped part-way, in CI on all three systems (§3).**
- **Level 2, by hand, on Linux:** Ctrl-C and Ctrl-Z through a pseudo-terminal under an interactive `bash` with job
  control, as review 200 did. Commit the script under `tools/`, and report its output. It is not a CI test.
- **What stays unproved, said so:** a real Ctrl-C on Windows. `std` cannot generate one, and the request must say
  "not proved", never "works".

## 1. The watchdog (D-1)

- **A module in the CLI crate,** `crates/brygge/src/watchdog.rs`, with `std` only. The workspace's
  `unsafe_code = "forbid"` stays.
- **`Watchdog::start() -> io::Result<Watchdog>`:**
  - it spawns `current_exe()` with `BRYGGE_INTERNAL_WATCHDOG=1`;
  - stdin piped, stdout null, stderr inherited;
  - Unix: `process_group(0)`. Windows: `creation_flags(CREATE_NEW_PROCESS_GROUP)` (the constant `0x200`, named, with
    its documentation link).
- **The methods:**
  - `register(&mut self, path: &Path)`, which writes `temp <encoded>`;
  - `release(&mut self, path: &Path)`, which writes `done <encoded>`;
  - `finish(self)`, which closes the pipe and waits for the watchdog's exit.
  - A write that fails (the watchdog gone) is **reported once, on stderr, and the command goes on.** Losing the
    watchdog must never fail a decode that would otherwise succeed. The run is then exactly as protected as 0.10.0's.
- **`decode`:** start the watchdog before the temporary artifact is created. Register it immediately after its
  `create_new`. Release it after the rename, or after its removal on a refusal. Call `finish` before the exit.
- **The encoding:** the machine-output alphabet's percent-encoding (`display::machine_value`), so one encoding serves
  both.

## 2. The watchdog side, and the internal entry (D-2, D-3)

- **`main`,** before argument parsing:
  - with `BRYGGE_INTERNAL_WATCHDOG=1`, it runs the watchdog loop;
  - **with the marker but stdin a terminal** (`IsTerminal`), it refuses with exit 2: "this is an internal entry of
    brygge, not a command".
- **The loop:**
  - read lines, keeping a list of registered paths in order;
  - at end of file, for each path still registered, apply D-3: the exact path; brygge's temporary form
    `.<name>.brygge-tmp-<pid>`; the `pid` equal to the watchdog's parent's (passed in the marker's value, for example
    `BRYGGE_INTERNAL_WATCHDOG=<pid>`, since `std` has no portable parent-id); `symlink_metadata` a regular file;
  - then remove it, or name it;
  - then print the one sentence, if anything was registered;
  - then exit 0.
- **The sentence:** `interrupted: removed <path>; nothing was written` (several paths: `removed <p1>, <p2>`). A path
  not removed: `interrupted: not removed: <path> (<reason>)`.

## 3. The proof in CI (W0's gate)

- **A new integration test,** `crates/brygge/tests/stop_integration.rs`, on all three systems:
  1. **The fixture:** generate an SVN dump in the test with content large enough that the artifact write takes
     visibly long, for example 40 files of 4 MiB of pseudo-random bytes. It is not committed.
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
  6. **Assert there is no watchdog left:** on Linux, no process with the marker in its environment. Elsewhere, the
     watchdog child's own exit is observed through a second, test-only pipe it inherits.
- **Plus the normal case:** a decode that is not stopped leaves no process, and its stderr is **byte-identical** to
  0.10.0's for the same fixture.
- **If the poll times out** (the write was too quick to catch), the test fails. It never passes vacuously.
- **If any system's test fails, stop and report.** RFC 022 then returns to the owner.

## 4. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- `cargo test --workspace` summed, plus the no-identity run;
- the isolation check, links, the book, the release tools, `cargo deny`;
- the deep mutation sweep is not due (no parser changes), unless `mutation.yml`'s filter says so.

## 5. Request

- **File** `.git-exclude/review-requests/094-w0-watchdog.md`, with the evidence plan and the hand-run pseudo-terminal
  output.
- **Push the branch only after approval.** **The CI run of the branch is W0's gate:** I read the Windows and macOS
  logs myself before W1 is handed off.

# Handoff — the CLI must not panic when its reader closes the pipe

**Found by the architect** in review 052 (a UX defect; no security or data effect). **Crate:** `brygge`
(the CLI). One review request.

## Today

```
$ brygge --version | head -1
brygge 0.3.1

thread 'main' panicked at …/library/std/src/io/stdio.rs:…:
failed printing to stdout: Broken pipe (os error 32)
```

- The exit code is 101.
- **The cause:** Rust ignores SIGPIPE, so when the reader closes stdout early, the next `println!` gets
  `EPIPE` and **panics**.
- `crates/brygge/src/commands.rs` has 23 `print!`/`println!` sites and `main.rs` 4. So every
  `brygge … | head`, `| less` quit early, or `| grep -m1` can end in a panic and a wrong exit code.
- brygge's machine output is designed to be piped, so this is the common case, not an edge case.

## The rule

- **When stdout is closed by its reader, brygge stops writing to stdout and finishes normally:**
  - no panic and no message;
  - **the exit code is the one the command would have had anyway** (0/10/20/30/50/1/2, as documented in
    `machine-output.md`);
  - the artifact a `decode` writes, and the verdict a `verify` reaches, do not depend on whether anyone read
    the report.
- **stderr:** a failed write to stderr is ignored (best effort), never a panic. brygge must not fail because
  its diagnostics had nowhere to go.
- **Any other stdout write error** (a full disk behind a redirect, for example) is not silent. It is reported
  on stderr as an output error, and exits `1`, the same as today's open and IO failures. A redirect that
  loses the report must not look like success.
- **How:** route every stdout write through one small writer (a locked `stdout` with `writeln!`, and one
  place that turns `ErrorKind::BrokenPipe` into "stop writing"), and every stderr write through one helper.
  - **Not** by resetting SIGPIPE: that needs `libc` and `unsafe`, and `forbid(unsafe_code)` holds.
  - `print!`/`println!`/`eprint!`/`eprintln!` should no longer appear in the CLI's non-test code. A
    `clippy::print_stdout`/`print_stderr` `deny` on the crate would keep it that way. Your call; say which in
    the request.

## Tests

- **For `--version`, `inspect` (a multi-line report) and `decode` (stdout report plus artifact):** spawn the
  real binary with stdout piped, read one line, close the pipe, and wait.
  - Assert the exit code is the command's normal one (0 or 10, as the fixture gives), and stderr contains no
    `panicked`.
  - For `decode`, the artifact is complete and verifies.
  - These run on all four CI platforms. On Windows a closed pipe also maps to `BrokenPipe`; if it does not,
    say so and how you handled it.
- **A stdout write error that is not `BrokenPipe`:** where it can be produced portably (for example `/dev/full`
  on Linux), it gives exit `1` and a stderr message. Unix-only is fine; skip elsewhere.
- Every existing CLI test passes unchanged.

## CHANGELOG `[Unreleased]`, Fixed

"`brygge … | head` (or any reader that stops early) no longer panics with 'Broken pipe': brygge stops writing
and exits with the command's own status. Other output errors are reported and exit `1`."

## Review request

`.git-exclude/review-requests/038-cli-broken-pipe.md`. After approval, commit and push to `main`, and append
CI.

//! A controllable fake `brygge encode` plugin, built only to drive `crates/brygge`'s own integration
//! tests of the `encode` command (RFC 017 D-3). It writes no target format and is not a real encoder:
//! never published, never shipped. Built alongside `brygge` by the same `cargo build`/`cargo test
//! --workspace`, so the tests can copy it onto a temporary `PATH` under a target-shaped name
//! (`brygge-encode-<target>`) and run it exactly as `brygge encode <target> …` would.
//!
//! Controls, read from its own argv (`brygge`'s mandatory `--artifact <path>` is ignored, like any other
//! argument it does not recognize):
//! - `--marker <path>`: write an empty file there as the very first thing it does, proving it started
//!   (used to prove the opposite: that `encode`'s verify-first gate kept it from starting at all).
//! - `--then-exit <n>`: exit with code `n` (default `0`) after the steps below.
//! - `--then-abort`: call [`std::process::abort`] instead of exiting normally — raises `SIGABRT` (6) on
//!   Unix, with no handler installed, so the process dies by that signal.
//! - every argument, recognized or not (including `--artifact <path>` itself), is echoed verbatim to
//!   stdout as `argv: <arg>`, one per line, before any of the above — proving `brygge encode` passes
//!   plugin arguments through exactly, with no shell.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Write as _;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut stdout = std::io::stdout();
    for arg in &args {
        let _ = writeln!(stdout, "argv: {arg}");
    }
    let _ = stdout.flush();

    let mut then_exit: i32 = 0;
    let mut then_abort = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--marker" => {
                let path = it.next().expect("--marker needs a value");
                std::fs::write(path, b"").expect("write marker file");
            }
            "--then-exit" => {
                let n = it.next().expect("--then-exit needs a value");
                then_exit = n.parse().expect("--then-exit needs an integer");
            }
            "--then-abort" => then_abort = true,
            _ => {}
        }
    }

    if then_abort {
        std::process::abort();
    }
    std::process::exit(then_exit);
}

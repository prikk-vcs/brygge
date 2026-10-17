//! The CLI's only writes to stdout and stderr (the `cli-broken-pipe-handoff-v1.md` rule): every other
//! module calls [`stdout`] and [`stderr`] instead of `print!`/`println!`/`eprint!`/`eprintln!`, which are
//! denied in this crate's non-test code (`#![deny(clippy::print_stdout, clippy::print_stderr)]` in
//! `main.rs`).
//!
//! brygge's machine output is designed to be piped, so a reader that stops early (`brygge … | head`, a
//! pager quit, `| grep -m1`) is the common case, not an edge case:
//!
//! - **A closed read end (`BrokenPipe`) is not a failure.** Rust ignores `SIGPIPE`, so the next write
//!   after the pipe closes would otherwise turn into an `EPIPE` and panic. [`stdout`] instead remembers
//!   the pipe is closed and silently drops every later write; the caller finishes the command normally,
//!   and the exit code is the one the command would have had anyway — it never depends on whether anyone
//!   read the report.
//! - **Any other stdout write error** (a full disk behind a redirect, for example) is not silent: it is
//!   reported on stderr and [`stdout`] tells the caller to stop and report `exit::FAILURE` instead of its
//!   own exit code. A redirect that loses the report must not look like success.
//! - **Every write is followed by a flush, and the flush error is caught too.** Stdout is line-buffered, so
//!   a report without a trailing newline would otherwise sit in the buffer until exit, where Rust ignores
//!   the final flush's error — a full disk could then silently lose the report's tail while the command
//!   still exits as if nothing were wrong. Flushing inside [`stdout`] closes that gap.
//! - **A failed write to stderr is ignored, best effort, never a panic.** brygge must not fail because its
//!   own diagnostics had nowhere to go.

use std::io::{self, ErrorKind, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::exit;

/// Set once stdout's reader has closed the pipe; every write after that is a silent no-op.
static STDOUT_CLOSED: AtomicBool = AtomicBool::new(false);

/// Write `s` to stdout.
///
/// Returns `None` when the caller should proceed exactly as if nothing had gone wrong — the write
/// succeeded, or the pipe was already (or just now) found closed. Returns `Some(exit::FAILURE)` only for
/// a genuine *other* write error, already reported on stderr; the caller must then return that code
/// instead of the exit code it was about to compute.
#[must_use]
pub fn stdout(s: &str) -> Option<i32> {
    if STDOUT_CLOSED.load(Ordering::Relaxed) {
        return None;
    }
    let mut out = io::stdout().lock();
    write_or_report(&mut out, s, &STDOUT_CLOSED)
}

/// The logic of [`stdout`], over any writer, so it can be tested against a fake one that simulates
/// `BrokenPipe` and other errors without needing a real pipe (the real pipe-closing behavior is an
/// integration test, in `tests/cli_integration.rs`, that spawns the actual binary).
fn write_or_report(w: &mut impl io::Write, s: &str, closed: &AtomicBool) -> Option<i32> {
    match w.write_all(s.as_bytes()).and_then(|()| w.flush()) {
        Ok(()) => None,
        Err(e) if e.kind() == ErrorKind::BrokenPipe => {
            closed.store(true, Ordering::Relaxed);
            None
        }
        Err(e) => {
            stderr(&format!("cannot write to stdout: {e}"));
            Some(exit::FAILURE)
        }
    }
}

/// Write `s` and a trailing newline to stderr, best effort: a failed write here is silently dropped,
/// never a panic (brygge's own diagnostics having nowhere to go is not itself a failure).
pub fn stderr(s: &str) {
    let mut err = io::stderr().lock();
    let _ = writeln!(err, "{s}");
}

#[cfg(test)]
mod tests;

//! Unit tests for the write-or-report logic, against a fake writer that can be told to fail (no real pipe
//! needed; the real pipe-closing behavior is `tests/cli_integration.rs`'s job).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{self, ErrorKind};
use std::sync::atomic::{AtomicBool, Ordering};

use super::write_or_report;

/// A writer that succeeds, or fails once with a chosen error kind, or always fails with it. `write`'s
/// failure is independent of `flush`'s: a writer can succeed at `write` and still fail at `flush`, which is
/// exactly the case `write_or_report` must also catch (F-1).
struct Fake {
    kind: Option<ErrorKind>,
    once: bool,
    failed_already: bool,
    flush_kind: Option<ErrorKind>,
    written: Vec<u8>,
}

impl Fake {
    fn ok() -> Self {
        Self {
            kind: None,
            once: false,
            failed_already: false,
            flush_kind: None,
            written: Vec::new(),
        }
    }

    fn failing(kind: ErrorKind) -> Self {
        Self {
            kind: Some(kind),
            once: false,
            failed_already: false,
            flush_kind: None,
            written: Vec::new(),
        }
    }

    /// Fails the *first* write with `kind`, then succeeds.
    fn failing_once(kind: ErrorKind) -> Self {
        Self {
            kind: Some(kind),
            once: true,
            failed_already: false,
            flush_kind: None,
            written: Vec::new(),
        }
    }

    /// `write` succeeds; the following `flush` fails with `kind`.
    fn failing_flush(kind: ErrorKind) -> Self {
        Self {
            kind: None,
            once: false,
            failed_already: false,
            flush_kind: Some(kind),
            written: Vec::new(),
        }
    }
}

impl io::Write for Fake {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(kind) = self.kind {
            if !self.once || !self.failed_already {
                self.failed_already = true;
                return Err(io::Error::from(kind));
            }
        }
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.flush_kind {
            Some(kind) => Err(io::Error::from(kind)),
            None => Ok(()),
        }
    }
}

#[test]
fn a_successful_write_reports_nothing_and_keeps_the_pipe_open() {
    let mut w = Fake::ok();
    let closed = AtomicBool::new(false);
    assert_eq!(write_or_report(&mut w, "hello", &closed), None);
    assert_eq!(w.written, b"hello");
    assert!(!closed.load(Ordering::Relaxed));
}

#[test]
fn a_broken_pipe_is_silently_dropped_and_marks_the_pipe_closed() {
    let mut w = Fake::failing(ErrorKind::BrokenPipe);
    let closed = AtomicBool::new(false);
    assert_eq!(write_or_report(&mut w, "hello", &closed), None);
    assert!(
        closed.load(Ordering::Relaxed),
        "the pipe is now known closed"
    );
}

#[test]
fn any_other_write_error_is_reported_and_tells_the_caller_to_fail() {
    let mut w = Fake::failing(ErrorKind::PermissionDenied);
    let closed = AtomicBool::new(false);
    assert_eq!(
        write_or_report(&mut w, "hello", &closed),
        Some(crate::exit::FAILURE)
    );
    assert!(
        !closed.load(Ordering::Relaxed),
        "not a broken pipe: the pipe is not marked closed"
    );
}

#[test]
fn a_write_that_fails_once_then_succeeds_is_still_the_first_errors_outcome() {
    // write_all may call write() more than once; the first error it sees is authoritative, whatever a
    // later retry inside the same call would have done.
    let mut w = Fake::failing_once(ErrorKind::BrokenPipe);
    let closed = AtomicBool::new(false);
    assert_eq!(write_or_report(&mut w, "hello", &closed), None);
    assert!(closed.load(Ordering::Relaxed));
}

#[test]
fn a_write_that_succeeds_but_a_flush_that_fails_is_still_reported() {
    // F-1: stdout is line-buffered, so a report without a trailing newline could sit unflushed and lose
    // its tail to a full disk unless the flush error is caught too, not just write's.
    let mut w = Fake::failing_flush(ErrorKind::Other);
    let closed = AtomicBool::new(false);
    assert_eq!(
        write_or_report(&mut w, "hello", &closed),
        Some(crate::exit::FAILURE)
    );
    assert!(
        !closed.load(Ordering::Relaxed),
        "not a broken pipe: the pipe is not marked closed"
    );
}

#[test]
fn a_flush_that_reports_broken_pipe_is_also_silently_dropped() {
    let mut w = Fake::failing_flush(ErrorKind::BrokenPipe);
    let closed = AtomicBool::new(false);
    assert_eq!(write_or_report(&mut w, "hello", &closed), None);
    assert!(
        closed.load(Ordering::Relaxed),
        "the pipe is now known closed"
    );
}

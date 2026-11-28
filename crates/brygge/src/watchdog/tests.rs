#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

// P-1(b): tested over `is_unmarked`'s explicit value, never by mutating the real process environment
// (`std::env::set_var`/`remove_var` are `unsafe` in this edition; the workspace forbids `unsafe`
// entirely).
#[test]
fn is_unmarked_is_true_only_when_the_marker_is_absent() {
    assert!(
        is_unmarked(None),
        "without the marker, a process must be allowed to start a watchdog"
    );
    assert!(
        !is_unmarked(Some("4242".into())),
        "with the marker already present (whatever its value), this process must never start \
         another watchdog — that is what bounds the recursion a fork bomb needs"
    );
}

// A sanity check that `may_spawn` itself reads the real environment correctly, in the one case every
// `cargo test` run is already in: unmarked (nothing in this test suite ever sets `MARKER_ENV`).
#[test]
fn may_spawn_is_true_in_an_ordinary_test_process() {
    assert!(may_spawn());
}

// P-1(c): the unit-test build's `Watchdog::start` cannot spawn anything — it refuses outright, every
// time, regardless of the environment.
#[test]
fn start_is_unsupported_in_a_unit_test_build() {
    match Watchdog::start() {
        Ok(_) => panic!("a unit-test build must never start a real watchdog"),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::Unsupported),
    }
}

// ---- D-1a(d): the handshake, proven on a bare `std::io::pipe()` — no process started (handoff §2(d)) --

use std::time::{Duration, Instant};

#[test]
fn await_ready_accepts_ready_with_the_matching_pid() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(b"ready 4242\n").unwrap();
    drop(writer);
    assert!(await_ready(reader, 4242, Duration::from_secs(2)).is_ok());
}

#[test]
fn await_ready_rejects_ready_with_a_different_pid() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(b"ready 1\n").unwrap();
    drop(writer);
    let err = await_ready(reader, 4242, Duration::from_secs(2)).unwrap_err();
    assert!(
        err.to_string().contains("ready 1"),
        "names what was seen: {err}"
    );
}

#[test]
fn await_ready_rejects_a_line_that_is_not_the_ready_message() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(b"garbage\n").unwrap();
    drop(writer);
    assert!(await_ready(reader, 4242, Duration::from_secs(2)).is_err());
}

#[test]
fn await_ready_errors_on_end_of_file_before_any_line() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(writer); // closed at once: the reader sees end of file, not a timeout.
    let start = Instant::now();
    assert!(await_ready(reader, 4242, Duration::from_secs(2)).is_err());
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "end of file is immediate, never the timeout"
    );
}

#[test]
fn await_ready_times_out_when_the_writer_never_writes() {
    let (reader, writer) = std::io::pipe().unwrap();
    let start = Instant::now();
    let err = await_ready(reader, 4242, Duration::from_millis(200)).unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(3), "in under 3 s");
    assert!(err.to_string().contains("did not answer"));
    drop(writer); // kept open (never written to) for the whole wait, exactly as the handoff specifies.
}

// ---- D-2: the internal entry's marker/terminal refusals ---------------------------------------------

#[test]
fn classify_internal_entry_accepts_a_decimal_pid_on_a_non_terminal_stdin() {
    assert_eq!(classify_internal_entry("4242", false), Ok(4242));
    assert_eq!(classify_internal_entry("  4242  ", false), Ok(4242));
}

#[test]
fn classify_internal_entry_refuses_a_marker_that_is_not_a_decimal_pid() {
    assert_eq!(
        classify_internal_entry("not-a-pid", false),
        Err(exit::USAGE)
    );
    assert_eq!(classify_internal_entry("", false), Err(exit::USAGE));
    assert_eq!(classify_internal_entry("-1", false), Err(exit::USAGE));
    assert_eq!(classify_internal_entry("4242.0", false), Err(exit::USAGE));
}

#[test]
fn classify_internal_entry_refuses_a_terminal_stdin_even_with_a_valid_pid() {
    assert_eq!(classify_internal_entry("4242", true), Err(exit::USAGE));
}

// ---- the protocol: encode, then parse (handoff §1, level 1) ------------------------------------------

#[test]
fn parse_line_round_trips_paths_with_a_newline_percent_space_and_non_ascii() {
    for raw in [
        "plain.txt",
        "has a space.txt",
        "has\na newline.txt",
        "has%percent.txt",
        "non-ascii-日本語.txt",
    ] {
        let path = std::path::PathBuf::from(raw);
        let line = format!("temp {}", encode_path(&path));
        match parse_line(&line) {
            Some(Message::Temp(p)) => assert_eq!(p, path, "round trip for {raw:?}"),
            other => panic!("expected Message::Temp({path:?}), got {other:?}"),
        }
        let line = format!("done {}", encode_path(&path));
        match parse_line(&line) {
            Some(Message::Done(p)) => assert_eq!(p, path, "round trip for {raw:?}"),
            other => panic!("expected Message::Done({path:?}), got {other:?}"),
        }
    }
}

#[test]
fn parse_line_refuses_anything_malformed() {
    assert_eq!(parse_line(""), None, "no verb, no path");
    assert_eq!(parse_line("temp"), None, "no path at all");
    assert_eq!(parse_line("frobnicate /tmp/x"), None, "unknown verb");
    assert_eq!(parse_line("temp a b"), None, "the path does not decode");
}

// ---- D-3: what the watchdog may remove ---------------------------------------------------------------

/// A fresh scratch directory for these tests alone, removed on drop even if a test panics first.
struct ScratchDir(std::path::PathBuf);

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl ScratchDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "brygge-watchdog-test-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn join(&self, name: impl AsRef<std::path::Path>) -> std::path::PathBuf {
        self.0.join(name)
    }
}

#[test]
fn why_not_removable_refuses_a_name_not_in_brygges_own_temporary_form() {
    let dir = ScratchDir::new("foreign-name");
    let path = dir.join("not-a-brygge-temp-name.txt");
    std::fs::write(&path, b"x").unwrap();
    assert!(why_not_removable(&path, 4242).is_some());
}

#[test]
fn why_not_removable_refuses_a_temp_name_carrying_a_different_pid() {
    let dir = ScratchDir::new("wrong-pid");
    let path = dir.join(".out.ir.brygge-tmp-1");
    std::fs::write(&path, b"x").unwrap();
    assert!(why_not_removable(&path, 4242).is_some());
}

#[test]
fn why_not_removable_refuses_a_path_that_only_shares_a_prefix_with_the_registered_suffix() {
    let dir = ScratchDir::new("prefix-only");
    // pid 12's suffix is `.brygge-tmp-12`; this name ends in `.brygge-tmp-123`, which is not that
    // suffix — `ends_with` on the literal trailing bytes must not be fooled by a numeric prefix match.
    let path = dir.join(".out.ir.brygge-tmp-123");
    std::fs::write(&path, b"x").unwrap();
    assert!(why_not_removable(&path, 12).is_some());
}

#[test]
fn why_not_removable_accepts_brygges_own_temporary_form_for_the_given_pid() {
    let dir = ScratchDir::new("accept");
    let path = dir.join(format!(".out.ir.brygge-tmp-{}", std::process::id()));
    std::fs::write(&path, b"x").unwrap();
    assert_eq!(why_not_removable(&path, std::process::id()), None);
}

#[test]
fn why_not_removable_accepts_a_path_already_gone() {
    let dir = ScratchDir::new("already-gone");
    let path = dir.join(format!(".out.ir.brygge-tmp-{}", std::process::id()));
    // Never created: `NotFound` is not a refusal — there is simply nothing left to remove.
    assert_eq!(why_not_removable(&path, std::process::id()), None);
}

#[cfg(unix)]
#[test]
fn why_not_removable_refuses_a_symbolic_link_even_in_brygges_own_temporary_form() {
    let dir = ScratchDir::new("symlink");
    let victim = dir.join("victim.txt");
    std::fs::write(&victim, b"precious").unwrap();
    let link = dir.join(format!(".out.ir.brygge-tmp-{}", std::process::id()));
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    let reason = why_not_removable(&link, std::process::id()).unwrap();
    assert!(reason.contains("symbolic link"), "{reason}");
}

// ---- apply_registrations: the loop's testable core ----------------------------------------------------

#[test]
fn apply_registrations_removes_only_what_is_still_registered_at_end_of_file() {
    let dir = ScratchDir::new("apply-removed");
    let pid = std::process::id();
    let path = dir.join(format!(".out.ir.brygge-tmp-{pid}"));
    std::fs::write(&path, b"x").unwrap();
    let lines = vec![format!("temp {}", encode_path(&path))].into_iter();
    let (removed, not_removed) =
        apply_registrations(lines, pid, |p: &Path| std::fs::remove_file(p));
    assert_eq!(removed, vec![path.clone()]);
    assert!(not_removed.is_empty());
    assert!(!path.exists());
}

#[test]
fn apply_registrations_leaves_a_done_path_alone() {
    let dir = ScratchDir::new("apply-done");
    let pid = std::process::id();
    let path = dir.join(format!(".out.ir.brygge-tmp-{pid}"));
    std::fs::write(&path, b"x").unwrap();
    let lines = vec![
        format!("temp {}", encode_path(&path)),
        format!("done {}", encode_path(&path)),
    ]
    .into_iter();
    let (removed, not_removed) =
        apply_registrations(lines, pid, |p: &Path| std::fs::remove_file(p));
    assert!(removed.is_empty());
    assert!(not_removed.is_empty());
    assert!(path.exists(), "released before end of file: never touched");
}

#[test]
fn apply_registrations_skips_a_malformed_line_and_touches_nothing_it_names() {
    let dir = ScratchDir::new("apply-malformed");
    let pid = std::process::id();
    let path = dir.join(format!(".out.ir.brygge-tmp-{pid}"));
    std::fs::write(&path, b"x").unwrap();
    // A malformed line naming the same path must not register it.
    let lines = vec![format!("frobnicate {}", encode_path(&path))].into_iter();
    let (removed, not_removed) =
        apply_registrations(lines, pid, |p: &Path| std::fs::remove_file(p));
    assert!(removed.is_empty());
    assert!(not_removed.is_empty());
    assert!(path.exists());
}

#[test]
fn apply_registrations_names_a_path_it_refuses_to_remove() {
    let dir = ScratchDir::new("apply-refuse");
    let pid = std::process::id();
    // Not one of brygge's own temporary forms: D-3 refuses it, by name alone.
    let path = dir.join("not-a-brygge-temp-name.txt");
    std::fs::write(&path, b"x").unwrap();
    let lines = vec![format!("temp {}", encode_path(&path))].into_iter();
    let (removed, not_removed) =
        apply_registrations(lines, pid, |p: &Path| std::fs::remove_file(p));
    assert!(removed.is_empty());
    assert_eq!(not_removed.len(), 1);
    assert_eq!(not_removed[0].0, path);
    assert!(path.exists());
}

// ---- the one sentence ---------------------------------------------------------------------------------

#[test]
fn sentence_names_one_removed_path() {
    let p = std::path::PathBuf::from("/tmp/.a.ir.brygge-tmp-1");
    assert_eq!(
        sentence(std::slice::from_ref(&p), &[]),
        format!("interrupted: removed {}; nothing was written", p.display())
    );
}

#[test]
fn sentence_names_several_removed_paths_joined_by_comma() {
    let p1 = std::path::PathBuf::from("/tmp/.a.ir.brygge-tmp-1");
    let p2 = std::path::PathBuf::from("/tmp/.b.ir.brygge-tmp-1");
    assert_eq!(
        sentence(&[p1.clone(), p2.clone()], &[]),
        format!(
            "interrupted: removed {}, {}; nothing was written",
            p1.display(),
            p2.display()
        )
    );
}

#[test]
fn sentence_names_a_path_it_did_not_remove() {
    let p = std::path::PathBuf::from("/tmp/victim.txt");
    assert_eq!(
        sentence(
            &[],
            &[(p.clone(), "a symbolic link, never followed".to_string())]
        ),
        format!(
            "interrupted: not removed: {} (a symbolic link, never followed); nothing was written",
            p.display()
        )
    );
}

#[test]
fn sentence_names_both_removed_and_not_removed_paths() {
    let removed = std::path::PathBuf::from("/tmp/.a.ir.brygge-tmp-1");
    let kept = std::path::PathBuf::from("/tmp/victim.txt");
    let s = sentence(
        std::slice::from_ref(&removed),
        &[(kept.clone(), "not a regular file".to_string())],
    );
    assert!(s.starts_with("interrupted: removed "));
    assert!(s.contains("; not removed: "));
    assert!(s.ends_with("; nothing was written"));
}

// ---- Guard::disabled(): never starts anything, and every method is a no-op ----------------------------

#[test]
fn guard_disabled_is_a_no_op_on_every_method() {
    let mut guard = Guard::disabled();
    guard.register(Path::new("/tmp/does-not-matter"));
    guard.release(Path::new("/tmp/does-not-matter"));
    guard.finish(); // must not panic, must not block.
}

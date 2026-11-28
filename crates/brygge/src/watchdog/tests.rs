#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::cli::{Format, SourceKind};

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
    assert_eq!(
        classify_internal_entry(std::ffi::OsStr::new("4242"), false),
        Ok(4242)
    );
    assert_eq!(
        classify_internal_entry(std::ffi::OsStr::new("  4242  "), false),
        Ok(4242)
    );
}

#[test]
fn classify_internal_entry_refuses_a_marker_that_is_not_a_decimal_pid() {
    assert_eq!(
        classify_internal_entry(std::ffi::OsStr::new("not-a-pid"), false),
        Err(exit::USAGE)
    );
    assert_eq!(
        classify_internal_entry(std::ffi::OsStr::new(""), false),
        Err(exit::USAGE)
    );
    assert_eq!(
        classify_internal_entry(std::ffi::OsStr::new("-1"), false),
        Err(exit::USAGE)
    );
    assert_eq!(
        classify_internal_entry(std::ffi::OsStr::new("4242.0"), false),
        Err(exit::USAGE)
    );
}

#[test]
fn classify_internal_entry_refuses_a_terminal_stdin_even_with_a_valid_pid() {
    assert_eq!(
        classify_internal_entry(std::ffi::OsStr::new("4242"), true),
        Err(exit::USAGE)
    );
}

// R-3: a marker that is present but not valid UTF-8 (in any encoding, under any interpretation) must
// still be refused — never treated as absent, which is what `env::var(...).ok()` used to do.
#[cfg(unix)]
#[test]
fn classify_internal_entry_refuses_a_marker_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    // 0xFF is never a valid UTF-8 lead or continuation byte, so this is not valid UTF-8 under any
    // interpretation.
    let invalid = std::ffi::OsStr::from_bytes(&[0xFF, 0xFE, b'4', b'2']);
    assert_eq!(classify_internal_entry(invalid, false), Err(exit::USAGE));
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
        let encoded =
            encode_path(&path).expect("every path built from a Rust &str is valid Unicode");
        let line = format!("temp {encoded}");
        match parse_line(&line) {
            Some(Message::Temp(p, Kind::File)) => assert_eq!(p, path, "round trip for {raw:?}"),
            other => panic!("expected Message::Temp({path:?}, Kind::File), got {other:?}"),
        }
        let dir_line = format!("tempdir {encoded}");
        match parse_line(&dir_line) {
            Some(Message::Temp(p, Kind::Dir)) => assert_eq!(p, path, "round trip for {raw:?}"),
            other => panic!("expected Message::Temp({path:?}, Kind::Dir), got {other:?}"),
        }
        let line = format!("done {encoded}");
        match parse_line(&line) {
            Some(Message::Done(p)) => assert_eq!(p, path, "round trip for {raw:?}"),
            other => panic!("expected Message::Done({path:?}), got {other:?}"),
        }
    }
}

// ---- R-4: encode_path/registration_line_or_notice, the Windows-only failure case -----------------------

#[test]
fn registration_line_or_notice_builds_a_temp_line_for_an_encodable_path() {
    let path = std::path::Path::new("plain.txt");
    let encoded = encode_path(path).expect("plain ASCII is always encodable");
    assert_eq!(
        registration_line_or_notice("temp", path),
        Ok(format!("temp {encoded}\n"))
    );
    assert_eq!(
        registration_line_or_notice("tempdir", path),
        Ok(format!("tempdir {encoded}\n"))
    );
}

// Only Windows's `encode_path` can ever return `None` (a path that is not valid Unicode); Unix paths are
// raw bytes and `encode_path` is `Some` unconditionally there (see its doc comment), so this case can
// only be exercised, honestly, on a Windows build. `OsString::from_wide` with a lone UTF-16 surrogate
// constructs a real Windows path that round-trips through the OS exactly, yet is not valid Unicode —
// `to_str()` on it is always `None` — so this is a real proof of the refusal, not a simulation of one.
#[cfg(windows)]
#[test]
fn encode_path_refuses_a_path_that_is_not_valid_unicode() {
    use std::os::windows::ffi::OsStringExt;
    let os_string = std::ffi::OsString::from_wide(&[0xD800]); // a lone (unpaired) high surrogate.
    let path = std::path::PathBuf::from(os_string);
    assert_eq!(encode_path(&path), None);
}

#[cfg(windows)]
#[test]
fn registration_line_or_notice_refuses_a_path_that_is_not_valid_unicode() {
    use std::os::windows::ffi::OsStringExt;
    let os_string = std::ffi::OsString::from_wide(&[0xD800]);
    let path = std::path::PathBuf::from(os_string);
    let Err(notice) = registration_line_or_notice("temp", &path) else {
        panic!("a non-Unicode Windows path must be refused, not registered");
    };
    assert!(
        notice.contains("not valid Unicode"),
        "the notice should say why: {notice}"
    );
}

#[test]
fn parse_line_refuses_anything_malformed() {
    assert_eq!(parse_line(""), None, "no verb, no path");
    assert_eq!(parse_line("temp"), None, "no path at all");
    assert_eq!(parse_line("frobnicate /tmp/x"), None, "unknown verb");
    assert_eq!(parse_line("temp a b"), None, "the path does not decode");
}

// ---- classify_temp_name: the three forms, and the lookalikes that must not match (W1, D-4) ------------

#[test]
fn classify_temp_name_accepts_the_atomic_write_form() {
    assert_eq!(classify_temp_name(".out.ir.brygge-tmp-4242"), Some(4242));
    assert_eq!(classify_temp_name(".a.brygge-tmp-1"), Some(1));
}

#[test]
fn classify_temp_name_accepts_the_transform_scratch_form() {
    assert_eq!(
        classify_temp_name(".brygge-transform-4242-999-0"),
        Some(4242)
    );
}

#[test]
fn classify_temp_name_accepts_the_encode_git_partial_form() {
    assert_eq!(classify_temp_name("out.partial-4242-0"), Some(4242));
    assert_eq!(
        classify_temp_name("out.brygge-report.partial-4242-3"),
        Some(4242)
    );
}

// Regression: CI run 46156356130 (Windows) found that `encode git`'s own private HOME, its
// GIT_CONFIG_GLOBAL, and its D-6 check's own spill directory were registered but never recognized by
// `classify_temp_name` at all, so a real, correctly registered stop left them named "not removed" —
// refused for a reason that was itself a bug, not a real symlink/type mismatch.
#[test]
fn classify_temp_name_accepts_encode_gits_own_env_and_check_spill_forms() {
    assert_eq!(
        classify_temp_name("brygge-encode-git-home-4242-1700000000000000000-0"),
        Some(4242)
    );
    assert_eq!(
        classify_temp_name("brygge-encode-git-gitconfig-4242-1700000000000000000-3"),
        Some(4242)
    );
    assert_eq!(
        classify_temp_name("brygge-encode-git-check-spill-4242-1700000000000000000-0"),
        Some(4242)
    );
}

#[test]
fn classify_temp_name_rejects_encode_git_env_lookalikes() {
    for name in [
        "brygge-encode-git-home-4242",                  // missing nanos/attempt
        "brygge-encode-git-home-4242-1700000000",       // missing attempt
        "brygge-encode-git-home-abc-1700000000-0",      // pid not decimal
        "brygge-encode-git-homebrew-4242-1700000000-0", // not actually the `home` label
        "brygge-encode-git-home-4242-1700000000-0-extra", // one group too many
    ] {
        assert_eq!(
            classify_temp_name(name),
            None,
            "{name:?} must not be recognized"
        );
    }
}

#[test]
fn why_not_removable_accepts_encode_gits_home_directory_for_the_given_pid() {
    let dir = ScratchDir::new("accept-encode-git-home");
    let pid = std::process::id();
    let path = dir.join(format!(
        "brygge-encode-git-home-{pid}-1700000000000000000-0"
    ));
    std::fs::create_dir_all(&path).unwrap();
    assert_eq!(why_not_removable(&path, pid, Kind::Dir), None);
}

#[test]
fn why_not_removable_accepts_encode_gits_gitconfig_file_for_the_given_pid() {
    let dir = ScratchDir::new("accept-encode-git-gitconfig");
    let pid = std::process::id();
    let path = dir.join(format!(
        "brygge-encode-git-gitconfig-{pid}-1700000000000000000-0"
    ));
    std::fs::write(&path, b"x").unwrap();
    assert_eq!(why_not_removable(&path, pid, Kind::File), None);
}

#[test]
fn classify_temp_name_rejects_names_that_only_resemble_a_form() {
    for name in [
        "not-a-brygge-temp-name.txt",
        ".brygge-tmp-4242",           // no leading dotted base before the suffix
        ".out.brygge-tmp-",           // no pid at all
        ".out.brygge-tmp-12x",        // not decimal
        ".brygge-transform-4242",     // missing nanos/attempt
        ".brygge-transform-4242-999", // missing attempt
        ".brygge-transform-4242-999-0-1", // one group too many
        ".brygge-transform-abc-999-0", // pid not decimal
        "out.partial-4242",           // missing attempt
        "out.partial-abc-0",          // pid not decimal
        ".partial-4242-0",            // empty base before `.partial-`
    ] {
        assert_eq!(
            classify_temp_name(name),
            None,
            "{name:?} must not be recognized as one of brygge's own forms"
        );
    }
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
    assert!(why_not_removable(&path, 4242, Kind::File).is_some());
}

#[test]
fn why_not_removable_refuses_a_temp_name_carrying_a_different_pid() {
    let dir = ScratchDir::new("wrong-pid");
    let path = dir.join(".out.ir.brygge-tmp-1");
    std::fs::write(&path, b"x").unwrap();
    assert!(why_not_removable(&path, 4242, Kind::File).is_some());
}

#[test]
fn why_not_removable_refuses_a_path_that_only_shares_a_prefix_with_the_registered_suffix() {
    let dir = ScratchDir::new("prefix-only");
    // pid 12's suffix is `.brygge-tmp-12`; this name ends in `.brygge-tmp-123`, which is not that
    // suffix — the parser must not be fooled by a numeric prefix match.
    let path = dir.join(".out.ir.brygge-tmp-123");
    std::fs::write(&path, b"x").unwrap();
    assert!(why_not_removable(&path, 12, Kind::File).is_some());
}

#[test]
fn why_not_removable_accepts_brygges_own_temporary_form_for_the_given_pid() {
    let dir = ScratchDir::new("accept");
    let path = dir.join(format!(".out.ir.brygge-tmp-{}", std::process::id()));
    std::fs::write(&path, b"x").unwrap();
    assert_eq!(
        why_not_removable(&path, std::process::id(), Kind::File),
        None
    );
}

#[test]
fn why_not_removable_accepts_a_path_already_gone() {
    let dir = ScratchDir::new("already-gone");
    let path = dir.join(format!(".out.ir.brygge-tmp-{}", std::process::id()));
    // Never created: `NotFound` is not a refusal — there is simply nothing left to remove.
    assert_eq!(
        why_not_removable(&path, std::process::id(), Kind::File),
        None
    );
}

#[cfg(unix)]
#[test]
fn why_not_removable_refuses_a_symbolic_link_even_in_brygges_own_temporary_form() {
    let dir = ScratchDir::new("symlink");
    let victim = dir.join("victim.txt");
    std::fs::write(&victim, b"precious").unwrap();
    let link = dir.join(format!(".out.ir.brygge-tmp-{}", std::process::id()));
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    let reason = why_not_removable(&link, std::process::id(), Kind::File).unwrap();
    assert!(reason.contains("symbolic link"), "{reason}");
}

// ---- D-3 widened to directories (W1) -------------------------------------------------------------------

#[test]
fn why_not_removable_accepts_a_registered_directory_for_the_given_pid() {
    let dir = ScratchDir::new("accept-dir");
    let pid = std::process::id();
    let path = dir.join(format!("out.partial-{pid}-0"));
    std::fs::create_dir_all(&path).unwrap();
    assert_eq!(why_not_removable(&path, pid, Kind::Dir), None);
}

#[test]
fn why_not_removable_refuses_a_regular_file_where_a_directory_was_registered() {
    let dir = ScratchDir::new("dir-mismatch-file");
    let pid = std::process::id();
    // `encode git`'s own name collision: this exact suffix form is also used for a file (the report's
    // temporary sibling) — a registration claiming `Kind::Dir` over an actual file must be refused.
    let path = dir.join(format!("out.partial-{pid}-0"));
    std::fs::write(&path, b"x").unwrap();
    let reason = why_not_removable(&path, pid, Kind::Dir).unwrap();
    assert!(reason.contains("not a directory"), "{reason}");
}

#[test]
fn why_not_removable_refuses_a_directory_where_a_regular_file_was_registered() {
    let dir = ScratchDir::new("file-mismatch-dir");
    let pid = std::process::id();
    let path = dir.join(format!("out.partial-{pid}-0"));
    std::fs::create_dir_all(&path).unwrap();
    let reason = why_not_removable(&path, pid, Kind::File).unwrap();
    assert!(reason.contains("not a regular file"), "{reason}");
}

#[cfg(unix)]
#[test]
fn why_not_removable_refuses_a_symbolic_link_registered_as_a_directory() {
    let dir = ScratchDir::new("symlink-dir");
    let victim = dir.join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    let pid = std::process::id();
    let link = dir.join(format!("out.partial-{pid}-0"));
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    let reason = why_not_removable(&link, pid, Kind::Dir).unwrap();
    assert!(reason.contains("symbolic link"), "{reason}");
}

#[test]
fn why_not_removable_accepts_the_transform_scratch_form_for_the_given_pid() {
    let dir = ScratchDir::new("accept-transform");
    let pid = std::process::id();
    let path = dir.join(format!(".brygge-transform-{pid}-12345-0"));
    std::fs::write(&path, b"x").unwrap();
    assert_eq!(why_not_removable(&path, pid, Kind::File), None);
}

// ---- stops_changing: D-3's rule, over an injected clock and snapshot (handoff §1, level 1) --------------
//
// Every case below runs with no real sleep at all: `poll` just pops the next prepared
// `(timestamp_ms, snapshot)` sample from a fixed sequence.

fn scripted(samples: Vec<(u64, u32)>) -> impl FnMut() -> (u64, u32) {
    let mut it = samples.into_iter();
    move || {
        it.next()
            .expect("stops_changing polled past the end of the scripted sequence")
    }
}

#[test]
fn stops_changing_removes_a_tree_unchanged_for_the_full_idle_time() {
    // Same snapshot (`0`) throughout; idle reached once 1000ms have elapsed since it was last seen to
    // change (at t=0, the start).
    let ok = stops_changing(
        (0, 0u32),
        scripted(vec![(500, 0), (1_000, 0)]),
        1_000,
        30_000,
    );
    assert!(ok, "unchanged for the full idle window must be removed");
}

#[test]
fn stops_changing_never_removes_a_tree_that_keeps_changing_past_the_ceiling() {
    // The snapshot changes every sample, right up to (and past) the 30s ceiling: never idle for even one
    // tick, so this must give up, not remove.
    let samples = vec![
        (1_000, 1),
        (5_000, 2),
        (10_000, 3),
        (20_000, 4),
        (29_999, 5),
        (30_000, 6),
    ];
    let ok = stops_changing((0, 0u32), scripted(samples), 1_000, 30_000);
    assert!(!ok, "a tree that never stops changing must not be removed");
}

#[test]
fn stops_changing_gives_up_at_exactly_the_ceiling_even_if_still_changing() {
    let ok = stops_changing(
        (0, 0u32),
        scripted(vec![(29_999, 1), (30_000, 2)]),
        1_000,
        30_000,
    );
    assert!(
        !ok,
        "the ceiling is reached before any idle window completes"
    );
}

#[test]
fn stops_changing_removes_once_idle_after_a_period_of_change() {
    // Changes a few times, then holds steady for the full idle window before the ceiling.
    let samples = vec![
        (1_000, 1), // changed
        (2_000, 2), // changed again
        (2_500, 2), // same — only 500ms idle so far
        (3_000, 2), // same — 1000ms idle since it last changed at t=2000: stable
    ];
    let ok = stops_changing((0, 0u32), scripted(samples), 1_000, 30_000);
    assert!(
        ok,
        "a tree that settles down before the ceiling must be removed"
    );
}

// ---- apply_registrations: the loop's testable core ----------------------------------------------------

/// A valid-Unicode path's `temp`/`tempdir`/`done` line, for tests that only care about well-formed
/// input. `encode_path` only ever returns `None` for a non-Unicode Windows path (R-4), which none of
/// these are.
fn well_formed_line(verb: &str, path: &Path) -> String {
    format!(
        "{verb} {}",
        encode_path(path).expect("a plain test path is always encodable")
    )
}

fn never_called_dir(_: &Path) -> DirOutcome {
    panic!("a directory-removal closure must not be called for a file-kind registration")
}

#[test]
fn apply_registrations_removes_only_what_is_still_registered_at_end_of_file() {
    let dir = ScratchDir::new("apply-removed");
    let pid = std::process::id();
    let path = dir.join(format!(".out.ir.brygge-tmp-{pid}"));
    std::fs::write(&path, b"x").unwrap();
    let lines = vec![well_formed_line("temp", &path)].into_iter();
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        pid,
        |p: &Path| std::fs::remove_file(p),
        never_called_dir,
    );
    assert_eq!(removed, vec![path.clone()]);
    assert!(not_removed.is_empty());
    assert_eq!(malformed, 0);
    assert!(!path.exists());
}

#[test]
fn apply_registrations_leaves_a_done_path_alone() {
    let dir = ScratchDir::new("apply-done");
    let pid = std::process::id();
    let path = dir.join(format!(".out.ir.brygge-tmp-{pid}"));
    std::fs::write(&path, b"x").unwrap();
    let lines = vec![
        well_formed_line("temp", &path),
        well_formed_line("done", &path),
    ]
    .into_iter();
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        pid,
        |p: &Path| std::fs::remove_file(p),
        never_called_dir,
    );
    assert!(removed.is_empty());
    assert!(not_removed.is_empty());
    assert_eq!(malformed, 0);
    assert!(path.exists(), "released before end of file: never touched");
}

#[test]
fn apply_registrations_skips_a_malformed_line_and_touches_nothing_it_names() {
    let dir = ScratchDir::new("apply-malformed");
    let pid = std::process::id();
    let path = dir.join(format!(".out.ir.brygge-tmp-{pid}"));
    std::fs::write(&path, b"x").unwrap();
    // A malformed line naming the same path must not register it.
    let lines = vec![well_formed_line("frobnicate", &path)].into_iter();
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        pid,
        |p: &Path| std::fs::remove_file(p),
        never_called_dir,
    );
    assert!(removed.is_empty());
    assert!(not_removed.is_empty());
    // R-2: a malformed line is still skipped, but no longer silently — it is counted.
    assert_eq!(malformed, 1);
    assert!(path.exists());
}

// R-2: a malformed line is counted even when it does not merely rename something that already exists —
// the count tracks every line `parse_line` refuses, regardless of what came before or after it.
#[test]
fn apply_registrations_counts_every_malformed_line() {
    let lines = vec![
        "frobnicate /tmp/x".to_string(),
        "temp".to_string(), // no path at all.
        "also not a real line".to_string(),
    ]
    .into_iter();
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        std::process::id(),
        |p: &Path| std::fs::remove_file(p),
        never_called_dir,
    );
    assert!(removed.is_empty());
    assert!(not_removed.is_empty());
    assert_eq!(malformed, 3);
}

#[test]
fn apply_registrations_names_a_path_it_refuses_to_remove() {
    let dir = ScratchDir::new("apply-refuse");
    let pid = std::process::id();
    // Not one of brygge's own temporary forms: D-3 refuses it, by name alone.
    let path = dir.join("not-a-brygge-temp-name.txt");
    std::fs::write(&path, b"x").unwrap();
    let lines = vec![well_formed_line("temp", &path)].into_iter();
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        pid,
        |p: &Path| std::fs::remove_file(p),
        never_called_dir,
    );
    assert!(removed.is_empty());
    assert_eq!(not_removed.len(), 1);
    assert_eq!(not_removed[0].0, path);
    assert_eq!(malformed, 0);
    assert!(path.exists());
}

#[test]
fn apply_registrations_removes_a_registered_directory_via_the_injected_closure() {
    let dir = ScratchDir::new("apply-dir-removed");
    let pid = std::process::id();
    let path = dir.join(format!("out.partial-{pid}-0"));
    std::fs::create_dir_all(&path).unwrap();
    let lines = vec![well_formed_line("tempdir", &path)].into_iter();
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        pid,
        |_: &Path| panic!("a file-removal closure must not be called for a dir-kind registration"),
        |p: &Path| {
            assert_eq!(p, path);
            std::fs::remove_dir_all(p).unwrap();
            DirOutcome::Removed
        },
    );
    assert_eq!(removed, vec![path]);
    assert!(not_removed.is_empty());
    assert_eq!(malformed, 0);
}

#[test]
fn apply_registrations_names_a_directory_still_changing_and_leaves_it() {
    let dir = ScratchDir::new("apply-dir-still-changing");
    let pid = std::process::id();
    let path = dir.join(format!("out.partial-{pid}-0"));
    std::fs::create_dir_all(&path).unwrap();
    let lines = vec![well_formed_line("tempdir", &path)].into_iter();
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        pid,
        |_: &Path| panic!("not reached"),
        |_: &Path| DirOutcome::StillChanging,
    );
    assert!(removed.is_empty());
    assert_eq!(not_removed.len(), 1);
    assert_eq!(not_removed[0].0, path);
    assert!(
        not_removed[0].1.contains("still changing"),
        "{}",
        not_removed[0].1
    );
    assert_eq!(malformed, 0);
    assert!(path.exists(), "a still-changing directory is left in place");
}

#[test]
fn apply_registrations_treats_an_already_gone_directory_as_neither_removed_nor_refused() {
    let dir = ScratchDir::new("apply-dir-already-gone");
    let pid = std::process::id();
    let path = dir.join(format!("out.partial-{pid}-0"));
    // Never created: the registered directory is simply already gone.
    let lines = vec![well_formed_line("tempdir", &path)].into_iter();
    let (removed, not_removed, malformed) = apply_registrations(
        lines,
        pid,
        |_: &Path| panic!("not reached"),
        |_: &Path| DirOutcome::AlreadyGone,
    );
    assert!(removed.is_empty());
    assert!(not_removed.is_empty());
    assert_eq!(malformed, 0);
}

// ---- R-2: the one-line notice (or its deliberate absence) ----------------------------------------------

#[test]
fn malformed_registrations_notice_is_none_when_nothing_was_malformed() {
    assert_eq!(malformed_registrations_notice(0), None);
}

#[test]
fn malformed_registrations_notice_names_the_count_when_above_zero() {
    assert_eq!(
        malformed_registrations_notice(1),
        Some("watchdog: 1 registration line(s) could not be read and were ignored".to_string())
    );
    assert_eq!(
        malformed_registrations_notice(3),
        Some("watchdog: 3 registration line(s) could not be read and were ignored".to_string())
    );
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
    guard.register_dir(Path::new("/tmp/does-not-matter-either"));
    guard.release(Path::new("/tmp/does-not-matter"));
    guard.finish(); // must not panic, must not block.
}

// ---- D-4: the leftover scan's pure message (handoff §1/§2) ---------------------------------------------

#[test]
fn leftover_notice_names_a_file_with_its_size_and_the_platform_remove_command() {
    let path = Path::new("/out/.a.ir.brygge-tmp-4242");
    let msg = leftover_notice(path, 4242, &LeftoverKind::File { size: 17 });
    assert!(msg.starts_with("found /out/.a.ir.brygge-tmp-4242 (17), a temporary file of brygge process 4242; if no brygge is running, remove it with: "));
    if cfg!(windows) {
        assert!(msg.ends_with("del /out/.a.ir.brygge-tmp-4242"), "{msg}");
    } else {
        assert!(msg.ends_with("rm /out/.a.ir.brygge-tmp-4242"), "{msg}");
    }
}

#[test]
fn leftover_notice_names_a_directory_with_no_size_and_the_recursive_remove_command() {
    let path = Path::new("/out/x.partial-4242-0");
    let msg = leftover_notice(path, 4242, &LeftoverKind::Dir);
    assert!(
        !msg.starts_with("found /out/x.partial-4242-0 ("),
        "a directory must carry no size: {msg}"
    );
    assert!(msg.starts_with(
        "found /out/x.partial-4242-0, a temporary directory of brygge process 4242; if no brygge \
         is running, remove it with: "
    ));
    if cfg!(windows) {
        assert!(msg.ends_with("rmdir /s /out/x.partial-4242-0"), "{msg}");
    } else {
        assert!(msg.ends_with("rm -r /out/x.partial-4242-0"), "{msg}");
    }
}

#[test]
fn scan_for_leftovers_names_another_pids_form_and_never_deletes() {
    let dir = ScratchDir::new("scan-leftover");
    let own_pid = std::process::id();
    let other_pid = own_pid.wrapping_add(1).max(1);
    let leftover = dir.join(format!(".x.ir.brygge-tmp-{other_pid}"));
    std::fs::write(&leftover, b"leftover").unwrap();
    scan_for_leftovers(dir.0.as_path(), own_pid);
    // Purely informational: nothing is ever deleted by the scan.
    assert!(leftover.exists());
}

#[test]
fn scan_for_leftovers_names_nothing_for_this_runs_own_pid() {
    let dir = ScratchDir::new("scan-own-pid");
    let own_pid = std::process::id();
    let mine = dir.join(format!(".x.ir.brygge-tmp-{own_pid}"));
    std::fs::write(&mine, b"mine").unwrap();
    // No direct assertion is possible on stderr content here (output::stderr is not test-observable
    // from this module); the behavioral proof that this run's own pid is excluded is
    // `classify_temp_name`'s pid match in `scan_for_leftovers`'s own `if found_pid == own_pid` guard,
    // already covered directly above, plus the CI-level `stop_integration.rs` case for another pid's
    // form. This test exists to prove the scan does not panic or misbehave on its own pid's form, and
    // that — exactly as every other D-4 behavior — it never deletes it.
    scan_for_leftovers(dir.0.as_path(), own_pid);
    assert!(mine.exists());
}

#[test]
fn scan_for_leftovers_ignores_a_path_that_only_resembles_a_form() {
    let dir = ScratchDir::new("scan-lookalike");
    let own_pid = std::process::id();
    let lookalike = dir.join("not-a-brygge-temp-name.txt");
    std::fs::write(&lookalike, b"x").unwrap();
    // No panic, and (as always) nothing deleted; the real proof that nothing is *named* for it belongs
    // to `classify_temp_name_rejects_names_that_only_resemble_a_form`, which this scan relies on
    // directly.
    scan_for_leftovers(dir.0.as_path(), own_pid);
    assert!(lookalike.exists());
}

// ---- command_needs_watchdog: a pure function of the parsed command (handoff §1, level 1) ----------------

fn decode_cmd() -> Command {
    Command::Decode {
        kind: SourceKind::Git,
        source: PathBuf::from("repo"),
        out: PathBuf::from("out.ir"),
        infer_renames: false,
        reconstruct_refs: false,
        branch_lines: false,
        layout_trunk: None,
        layout_branches: None,
        layout_tags: None,
        spill_dir: None,
        format: Format::Human,
    }
}

fn verify_cmd(against_source: Option<PathBuf>) -> Command {
    Command::Verify {
        artifact: PathBuf::from("out.ir"),
        against_source,
        against_target: None,
        spill_dir: None,
        format: Format::Human,
    }
}

#[test]
fn command_needs_watchdog_is_true_for_decode() {
    assert!(command_needs_watchdog(&decode_cmd()));
}

#[test]
fn command_needs_watchdog_is_true_for_encode_git() {
    assert!(command_needs_watchdog(&Command::EncodeGit {
        artifact: PathBuf::from("out.ir"),
        out: PathBuf::from("repo.git"),
        format: Format::Human,
    }));
}

#[test]
fn command_needs_watchdog_is_true_for_transform() {
    assert!(command_needs_watchdog(&Command::Transform {
        name: "mailmap-example".to_string(),
        artifact: PathBuf::from("out.ir"),
        out: PathBuf::from("out2.ir"),
        plugin_args: Vec::new(),
        format: Format::Human,
    }));
}

#[test]
fn command_needs_watchdog_is_true_for_the_built_in_transform_authors() {
    assert!(command_needs_watchdog(&Command::TransformAuthors {
        artifact: PathBuf::from("out.ir"),
        map: PathBuf::from("map.txt"),
        out: PathBuf::from("out2.ir"),
        require_all: false,
        format: Format::Human,
    }));
}

#[test]
fn command_needs_watchdog_is_true_for_verify_with_against_source() {
    assert!(command_needs_watchdog(&verify_cmd(Some(PathBuf::from(
        "repo"
    )))));
}

#[test]
fn command_needs_watchdog_is_false_for_verify_without_against_source() {
    assert!(!command_needs_watchdog(&verify_cmd(None)));
}

#[test]
fn command_needs_watchdog_is_false_for_every_read_only_command() {
    let read_only = [
        Command::Inspect {
            artifact: PathBuf::from("out.ir"),
            atoms: false,
            format: Format::Human,
        },
        Command::InspectAuthors {
            artifact: PathBuf::from("out.ir"),
            format: Format::Human,
        },
        Command::Encode {
            target: "example".to_string(),
            artifact: PathBuf::from("out.ir"),
            plugin_args: Vec::new(),
            format: Format::Human,
        },
        Command::EncodeList {
            format: Format::Human,
        },
        Command::TransformList {
            format: Format::Human,
        },
        Command::Version,
        Command::Help,
    ];
    for cmd in &read_only {
        assert!(
            !command_needs_watchdog(cmd),
            "{cmd:?} must start no watchdog"
        );
    }
}

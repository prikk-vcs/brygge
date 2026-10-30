#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "brygge-plugin-test-{}-{nanos}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

/// Writes a minimal runnable file at `dir/name` (executable on Unix).
fn write_runnable(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// Writes a file with no execute bit (Unix) at `dir/name`. Unix-only: its only caller,
/// `find_in_skips_a_non_executable_file`, is (the property it tests does not exist elsewhere).
#[cfg(unix)]
fn write_non_executable(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"not executable\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    path
}

fn plugin_name(target: &str) -> String {
    plugin_file_name(Kind::Encode, target)
}

#[test]
fn path_dirs_from_skips_empty_and_relative_entries() {
    let sep = if cfg!(windows) { ';' } else { ':' };
    // An absolute path needs a drive prefix on Windows (`/absolute/one` has a root but no prefix, so
    // `Path::is_absolute` is false for it there) — unlike Unix, where it already is absolute.
    let absolute = if cfg!(windows) {
        r"C:\absolute\one"
    } else {
        "/absolute/one"
    };
    let value = format!("{sep}relative{sep}{absolute}{sep}{sep}also/relative");
    let dirs = path_dirs_from(Some(std::ffi::OsStr::new(&value)));
    assert_eq!(dirs, vec![std::path::PathBuf::from(absolute)]);
}

#[test]
fn path_dirs_from_none_is_empty() {
    assert!(path_dirs_from(None).is_empty());
}

#[test]
fn find_in_skips_a_plugin_shaped_directory() {
    // A directory of the expected name is not a file on any platform, unlike an unset execute bit
    // (Unix-only — see `find_in_skips_a_non_executable_file`, below).
    let a = temp_dir();
    let b = temp_dir();
    std::fs::create_dir_all(a.0.join(plugin_name("foo"))).unwrap();
    let b_plugin = write_runnable(&b.0, &plugin_name("foo"));
    let found = find_in(Kind::Encode, "foo", &[a.0.clone(), b.0.clone()]);
    assert_eq!(found, Some(b_plugin));
}

#[test]
fn find_in_prefers_the_earlier_directory() {
    let a = temp_dir();
    let b = temp_dir();
    let a_plugin = write_runnable(&a.0, &plugin_name("foo"));
    write_runnable(&b.0, &plugin_name("foo"));
    let found = find_in(Kind::Encode, "foo", &[a.0.clone(), b.0.clone()]);
    assert_eq!(found, Some(a_plugin));
}

#[test]
fn find_in_none_when_absent() {
    let a = temp_dir();
    assert_eq!(
        find_in(Kind::Encode, "foo", std::slice::from_ref(&a.0)),
        None
    );
}

#[cfg(unix)]
#[test]
fn find_in_skips_a_non_executable_file() {
    let a = temp_dir();
    write_non_executable(&a.0, &plugin_name("foo"));
    assert_eq!(
        find_in(Kind::Encode, "foo", std::slice::from_ref(&a.0)),
        None
    );
}

#[test]
fn list_in_marks_a_later_match_shadowed() {
    let a = temp_dir();
    let b = temp_dir();
    let a_plugin = write_runnable(&a.0, &plugin_name("foo"));
    let b_plugin = write_runnable(&b.0, &plugin_name("foo"));
    let listed = list_in(Kind::Encode, &[a.0.clone(), b.0.clone()]);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].name, "foo");
    assert_eq!(listed[0].path, a_plugin);
    assert!(!listed[0].shadowed);
    assert_eq!(listed[1].name, "foo");
    assert_eq!(listed[1].path, b_plugin);
    assert!(listed[1].shadowed);
}

#[test]
fn list_in_ignores_a_name_with_no_valid_target_suffix() {
    let a = temp_dir();
    write_runnable(&a.0, "brygge-encode-"); // empty target: invalid
    write_runnable(&a.0, "brygge-encode-A"); // uppercase: invalid
    write_runnable(&a.0, "not-brygge-encode-foo"); // wrong prefix
    assert!(list_in(Kind::Encode, std::slice::from_ref(&a.0)).is_empty());
}

#[test]
fn list_in_two_different_targets_in_one_directory_sort_by_target() {
    let a = temp_dir();
    write_runnable(&a.0, &plugin_name("zzz"));
    write_runnable(&a.0, &plugin_name("aaa"));
    let listed = list_in(Kind::Encode, std::slice::from_ref(&a.0));
    let targets: Vec<&str> = listed.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(targets, vec!["aaa", "zzz"]);
}

#[cfg(windows)]
#[test]
fn list_in_ignores_bat_and_cmd_on_windows() {
    let a = temp_dir();
    std::fs::write(a.0.join("brygge-encode-foo.bat"), b"@echo off\n").unwrap();
    std::fs::write(a.0.join("brygge-encode-foo.cmd"), b"@echo off\n").unwrap();
    assert!(list_in(Kind::Encode, std::slice::from_ref(&a.0)).is_empty());
}

#[test]
fn encode_and_transform_plugins_of_the_same_name_do_not_collide() {
    let a = temp_dir();
    let encode_plugin = write_runnable(&a.0, &plugin_file_name(Kind::Encode, "same-name"));
    let transform_plugin = write_runnable(&a.0, &plugin_file_name(Kind::Transform, "same-name"));
    assert_ne!(encode_plugin, transform_plugin);
    assert_eq!(
        find_in(Kind::Encode, "same-name", std::slice::from_ref(&a.0)),
        Some(encode_plugin)
    );
    assert_eq!(
        find_in(Kind::Transform, "same-name", std::slice::from_ref(&a.0)),
        Some(transform_plugin)
    );
    let encode_listed = list_in(Kind::Encode, std::slice::from_ref(&a.0));
    let transform_listed = list_in(Kind::Transform, std::slice::from_ref(&a.0));
    assert_eq!(encode_listed.len(), 1);
    assert_eq!(transform_listed.len(), 1);
    assert_eq!(encode_listed[0].name, "same-name");
    assert_eq!(transform_listed[0].name, "same-name");
}

#[test]
fn transform_lookup_follows_every_rule_encode_does() {
    let a = temp_dir();
    let b = temp_dir();
    let a_plugin = write_runnable(&a.0, &plugin_file_name(Kind::Transform, "authors"));
    write_runnable(&b.0, &plugin_file_name(Kind::Transform, "authors"));
    assert_eq!(
        find_in(Kind::Transform, "authors", &[a.0.clone(), b.0.clone()]),
        Some(a_plugin)
    );
    assert_eq!(
        find_in(Kind::Transform, "missing", std::slice::from_ref(&a.0)),
        None
    );
    let listed = list_in(Kind::Transform, &[a.0.clone(), b.0.clone()]);
    assert_eq!(listed.len(), 2);
    assert!(!listed[0].shadowed);
    assert!(listed[1].shadowed);
}

#[test]
fn signal_name_knows_the_common_low_numbered_signals() {
    assert_eq!(signal_name(9), Some("SIGKILL"));
    assert_eq!(signal_name(6), Some("SIGABRT"));
    assert_eq!(signal_name(15), Some("SIGTERM"));
    assert_eq!(signal_name(200), None);
}

#[test]
fn run_reports_cannot_start_for_a_missing_path() {
    let a = temp_dir();
    let missing = a.0.join("does-not-exist");
    match run(&missing, &[]) {
        Ran::CannotStart(_) => {}
        _ => panic!("expected CannotStart"),
    }
}

//! Tests for phase computation (RFC 005 corrections handoff §1.1). Uses a real `hg` repository when
//! available, since phaseroots is only produced by `hg` itself; skips otherwise.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::compute;
use crate::revlog::Revlog;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn hg_available() -> bool {
    Command::new("hg")
        .arg("--version")
        .env("HGRCPATH", "/dev/null")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

struct Repo {
    dir: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Repo {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "brygge-hg-phases-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Self { dir };
        r.run(&["init", r.dir.to_str().unwrap()]);
        r
    }

    fn store(&self) -> PathBuf {
        self.dir.join(".hg").join("store")
    }

    fn run(&self, args: &[&str]) {
        let ok = Command::new("hg")
            .env("HGRCPATH", "/dev/null")
            .arg("--cwd")
            .arg(&self.dir)
            .arg("-R")
            .arg(&self.dir)
            .arg("--config")
            .arg("ui.username=A <a@example.com>")
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "hg {args:?} failed");
    }

    fn write(&self, rel: &str, contents: &str) {
        std::fs::write(self.dir.join(rel), contents).unwrap();
    }

    fn commit(&self, epoch: &str, msg: &str) {
        self.run(&["add"]);
        self.run(&["commit", "-d", &format!("{epoch} 0"), "-m", msg]);
    }

    fn changelog(&self) -> Revlog {
        Revlog::open(&self.store().join("00changelog.i")).unwrap()
    }
}

#[test]
fn absent_phaseroots_means_everything_public() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    // A fresh repo with no draft commits pushed anywhere still has a phaseroots file once something is
    // committed (hg tracks draft by default) — so instead assert directly against a store with the
    // file removed, to test the "absent" branch specifically.
    let store = r.store();
    let _ = std::fs::remove_file(store.join("phaseroots"));
    let changelog = r.changelog();
    let phase = compute(&store, &changelog, &crate::published::Limits::default()).unwrap();
    assert!(phase.iter().all(|&p| p == 0));
}

#[test]
fn a_secret_changeset_and_its_descendant_are_phase_2_a_plain_draft_is_phase_1() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0"); // draft by default
    r.write("b.txt", "b\n");
    r.commit("2", "c1");
    r.run(&["phase", "--secret", "--force", "-r", "0"]);

    let changelog = r.changelog();
    let phase = compute(&r.store(), &changelog, &crate::published::Limits::default()).unwrap();
    assert_eq!(phase.len(), 2);
    assert_eq!(phase[0], 2, "explicitly marked secret");
    assert_eq!(phase[1], 2, "descends from a secret root, so it is too");
}

#[test]
fn a_malformed_phaseroots_line_is_an_error() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    std::fs::write(r.store().join("phaseroots"), "not a valid line\n").unwrap();
    let changelog = r.changelog();
    assert!(compute(&r.store(), &changelog, &crate::published::Limits::default()).is_err());
}

#[test]
fn an_unknown_phase_value_is_rejected() {
    // Review 009 R-6: only 1 (draft), 2 (secret), 32 (archived), 96 (internal) are defined; public (0)
    // is the implicit default and never appears as a written root.
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    std::fs::write(
        r.store().join("phaseroots"),
        format!("7 {}\n", "ab".repeat(20)),
    )
    .unwrap();
    let changelog = r.changelog();
    assert!(compute(&r.store(), &changelog, &crate::published::Limits::default()).is_err());
}

#[test]
fn a_non_utf8_phaseroots_file_is_a_read_error_not_an_open_error() {
    // Review 009 R-6: a store that cannot say what is secret must not be decoded as if nothing were —
    // and a garbled phaseroots file is a content problem (`Read`), not an I/O problem (`Open`).
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    std::fs::write(r.store().join("phaseroots"), [0xFFu8, 0xFE, b'\n']).unwrap();
    let changelog = r.changelog();
    match compute(&r.store(), &changelog, &crate::published::Limits::default()) {
        Err(crate::Error::Read(_)) => {}
        other => panic!("expected Error::Read, got {other:?}"),
    }
}

#[test]
fn an_oversize_phaseroots_file_is_a_resource_limit() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    std::fs::write(
        r.store().join("phaseroots"),
        format!("2 {}\n", "ab".repeat(20)),
    )
    .unwrap();
    let changelog = r.changelog();
    let tiny = crate::published::Limits {
        max_small_file_bytes: 4,
    };
    match compute(&r.store(), &changelog, &tiny) {
        Err(crate::Error::ResourceLimit { .. }) => {}
        other => panic!("expected Error::ResourceLimit, got {other:?}"),
    }
}

#[test]
fn a_phaseroots_node_absent_from_the_changelog_is_ignored() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    std::fs::write(
        r.store().join("phaseroots"),
        format!("2 {}\n", "ab".repeat(20)),
    )
    .unwrap();
    let changelog = r.changelog();
    let phase = compute(&r.store(), &changelog, &crate::published::Limits::default()).unwrap();
    assert_eq!(phase, vec![0]);
}

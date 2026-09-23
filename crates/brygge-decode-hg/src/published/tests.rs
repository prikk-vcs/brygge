//! Tests for the published-view computation (RFC 005 corrections handoff §1). Needs a real `hg` with
//! core evolution enabled to produce obsolescence markers; skips when `hg` is absent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::compute;
use crate::decode::manifest_of;
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
            "brygge-hg-published-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Self { dir };
        r.run(&["init", r.dir.to_str().unwrap()]);
        std::fs::write(
            r.dir.join(".hg").join("hgrc"),
            "[experimental]\nevolution = all\nevolution.createmarkers = true\n",
        )
        .unwrap();
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

    fn manifest_log(&self) -> Revlog {
        Revlog::open(&self.store().join("00manifest.i")).unwrap()
    }
}

fn manifest_node_to_rev(manifest_log: &Revlog) -> HashMap<[u8; 20], usize> {
    let mut m = HashMap::new();
    for rev in 0..manifest_log.len() {
        if let Some(e) = manifest_log.entry(rev) {
            m.insert(e.node, rev);
        }
    }
    m
}

#[test]
fn an_amended_changeset_is_hidden_and_not_served() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "first");
    r.write("a.txt", "a a\n");
    r.run(&["commit", "--amend", "-d", "2 0", "-m", "first amended"]);

    let changelog = r.changelog();
    let view = compute(&r.dir, &r.store(), &changelog).unwrap();

    // Two changesets total in the store (the pre-amend original plus the amended one); only one served.
    assert_eq!(changelog.len(), 2);
    assert_eq!(view.hidden, 1);
    assert_eq!(view.served.iter().filter(|&&s| s).count(), 1);
    // The amended (second, tip) changeset is the one served.
    assert!(!view.served[0]);
    assert!(view.served[1]);
}

#[test]
fn an_obsolete_changeset_with_a_non_obsolete_descendant_stays_visible() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.write("b.txt", "b\n");
    r.commit("2", "c1"); // depends on c0
    r.run(&["update", "0"]);
    r.write("a.txt", "a a\n");
    // amend c0: this creates a NEW, disconnected changeset (obsoleting the original c0 via a marker,
    // not a parent link), and c1 — whose parent link still names the *old*, now-obsolete c0 — becomes
    // an orphan. c1 itself is not obsolete, so it stays visible; and because c1's own replay needs the
    // old c0's tree, the old (obsolete) c0 must stay visible too — an obsolete ancestor of anything
    // visible is never hidden, by the handoff's own definition (§1.3).
    r.run(&["commit", "--amend", "-d", "3 0", "-m", "c0 amended"]);

    let changelog = r.changelog();
    let view = compute(&r.dir, &r.store(), &changelog).unwrap();

    assert_eq!(
        view.hidden, 0,
        "the old c0 is obsolete but is an ancestor of the still-visible orphan c1, so it is not hidden"
    );
    assert!(
        view.served.iter().all(|&s| s),
        "all three changesets are served: {:?}",
        view.served
    );
}

#[test]
fn a_bookmarked_obsolete_changeset_is_pinned_and_stays_visible() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.run(&["bookmark", "keep-me", "-r", "0"]);
    r.write("a.txt", "a a\n");
    // amend, but move the bookmark back onto the (now obsolete) original with `hg bookmark -r`. Simpler
    // and robust across hg versions: just re-point the bookmark at the precursor node explicitly.
    let precursor_node = {
        let changelog = r.changelog();
        changelog.entry(0).unwrap().node
    };
    let precursor_hex = precursor_node
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    r.run(&["commit", "--amend", "-d", "2 0", "-m", "c0 amended"]);
    r.run(&[
        "bookmark",
        "--hidden",
        "-r",
        &precursor_hex,
        "-f",
        "keep-me",
    ]);

    let changelog = r.changelog();
    let view = compute(&r.dir, &r.store(), &changelog).unwrap();

    assert_eq!(view.hidden, 0, "pinned by the bookmark, so not hidden");
    assert!(view.served.iter().all(|&s| s));
}

#[test]
fn manifest_of_reads_the_null_manifest_as_empty() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    let manifest_log = r.manifest_log();
    let node_to_rev = manifest_node_to_rev(&manifest_log);
    let m = manifest_of(&manifest_log, &node_to_rev, &[0u8; 20]).unwrap();
    assert!(m.is_empty());
}

// ---- review 009 F-2 / F-3: bounded, strict small-file reads (no `hg` needed) ------------------------

fn scratch_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("brygge-hg-pub-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn read_bounded_accepts_exactly_the_ceiling_and_refuses_one_byte_more() {
    let dir = scratch_dir("bounded");
    let limits = super::Limits {
        max_small_file_bytes: 8,
    };
    std::fs::write(dir.join("at"), b"12345678").unwrap();
    std::fs::write(dir.join("over"), b"123456789").unwrap();
    assert_eq!(
        super::read_bounded(&dir.join("at"), "the test file", &limits)
            .unwrap()
            .unwrap(),
        b"12345678"
    );
    match super::read_bounded(&dir.join("over"), "the test file", &limits) {
        Err(crate::Error::ResourceLimit { what, .. }) => assert_eq!(what, "the test file"),
        other => panic!("expected ResourceLimit, got {other:?}"),
    }
    assert!(
        super::read_bounded(&dir.join("absent"), "the test file", &limits)
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn dirstate_result(bytes: Option<&[u8]>) -> Result<Vec<bool>, crate::Error> {
    let dir = scratch_dir("dirstate");
    std::fs::create_dir_all(dir.join(".hg")).unwrap();
    if let Some(b) = bytes {
        std::fs::write(dir.join(".hg").join("dirstate"), b).unwrap();
    }
    let mut node_to_rev = HashMap::new();
    node_to_rev.insert([1u8; 20], 0usize);
    let mut pinned = vec![false];
    let r = super::mark_pinned_dirstate(&dir, &node_to_rev, &mut pinned).map(|()| pinned);
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[test]
fn a_truncated_dirstate_is_a_read_error_but_empty_or_absent_means_no_parents() {
    // 1..=39 bytes: a truncated header — refused, never read as "pins nothing".
    for len in [1usize, 19, 20, 39] {
        let bytes = vec![1u8; len];
        match dirstate_result(Some(&bytes)) {
            Err(crate::Error::Read(_)) => {}
            other => panic!("{len} bytes: expected Read, got {other:?}"),
        }
    }
    // Empty and absent: no parents.
    assert_eq!(dirstate_result(Some(&[])).unwrap(), vec![false]);
    assert_eq!(dirstate_result(None).unwrap(), vec![false]);
    // A full 40-byte header pins p1 (and ignores the null p2).
    let mut full = vec![1u8; 20];
    full.extend_from_slice(&[0u8; 20]);
    assert_eq!(dirstate_result(Some(&full)).unwrap(), vec![true]);
}

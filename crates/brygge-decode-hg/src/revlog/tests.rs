//! Tests for the revlog reader (RFC 005), validated against **ground truth**: a real Mercurial repo
//! built with the `hg` CLI, then every reconstructed revision compared byte-for-byte against
//! `hg debugdata` and every node against `hg log`. Skips if `hg` is not on `PATH`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::Revlog;

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
        let dir =
            std::env::temp_dir().join(format!("brygge-hg-test-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Self { dir };
        r.run(&["init", r.dir.to_str().unwrap()]);
        r
    }

    fn store(&self) -> PathBuf {
        self.dir.join(".hg").join("store")
    }

    fn run(&self, args: &[&str]) -> Vec<u8> {
        let out = Command::new("hg")
            .env("HGRCPATH", "/dev/null")
            .arg("--cwd")
            .arg(&self.dir)
            .arg("-R")
            .arg(&self.dir)
            .arg("--config")
            .arg("ui.username=A U Thor <a@example.com>")
            .args(args)
            .output()
            .expect("run hg");
        assert!(
            out.status.success(),
            "hg {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn write(&self, rel: &str, contents: &str) {
        std::fs::write(self.dir.join(rel), contents).unwrap();
    }

    fn commit(&self, epoch: &str, msg: &str) {
        self.run(&["add"]);
        self.run(&["commit", "-d", &format!("{epoch} 0"), "-m", msg]);
    }
}

fn hex20(node: &[u8; 20]) -> String {
    node.iter().map(|b| format!("{b:02x}")).collect()
}

/// A repo exercising every reader path: a small ('u') file, a large compressible (zstd) file, a delta
/// (append), a modification, and a rename (`hg mv`, filelog copy metadata).
fn build_repo() -> Repo {
    let r = Repo::new();
    r.write("readme.txt", "hello\n");
    r.write("big.txt", &"abcdefgh\n".repeat(4000)); // large + compressible -> zstd chunk
    r.commit("1136239445", "initial");

    r.write("readme.txt", "hello world\n"); // modify
    r.write("big.txt", &("abcdefgh\n".repeat(4000) + "one more line\n")); // append -> delta
    r.commit("1136239446", "grow");

    r.run(&["mv", "readme.txt", "docs.txt"]); // rename -> filelog copy metadata
    r.commit("1136239447", "rename readme");
    r
}

/// Compare every revision of one revlog against `hg debugdata`.
fn assert_revlog_matches(
    r: &Repo,
    index_path: &Path,
    debugdata_args: &dyn Fn(usize) -> Vec<String>,
) {
    let rl = Revlog::open(index_path).expect("open revlog");
    assert!(!rl.is_empty(), "revlog {index_path:?} has no revisions");
    for rev in 0..rl.len() {
        let mine = rl.revision(rev).expect("reconstruct revision");
        let args: Vec<String> = debugdata_args(rev);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let truth = r.run(&refs);
        assert_eq!(
            mine, truth,
            "revision {rev} of {index_path:?} does not match hg debugdata"
        );
    }
}

#[test]
fn changelog_manifest_and_filelogs_match_hg_debugdata() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let store = r.store();

    // Changelog (split: 00changelog.i + .d) — full snapshots and any deltas.
    assert_revlog_matches(&r, &store.join("00changelog.i"), &|rev| {
        vec!["debugdata".into(), "-c".into(), rev.to_string()]
    });

    // Manifest (inline).
    assert_revlog_matches(&r, &store.join("00manifest.i"), &|rev| {
        vec!["debugdata".into(), "-m".into(), rev.to_string()]
    });

    // Filelogs: big.txt exercises a zstd snapshot (rev0) + a delta (rev1); readme/docs exercise the
    // rename's copy-metadata header (returned verbatim in the revision text).
    for (path, store_name) in [
        ("big.txt", "data/big.txt.i"),
        ("readme.txt", "data/readme.txt.i"),
        ("docs.txt", "data/docs.txt.i"),
    ] {
        assert_revlog_matches(&r, &store.join(store_name), &move |rev| {
            vec!["debugdata".into(), path.to_string(), rev.to_string()]
        });
    }
}

#[test]
fn nodes_and_parents_match_hg_log() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let rl = Revlog::open(&r.store().join("00changelog.i")).expect("open changelog");
    assert_eq!(rl.len(), 3, "three changesets");
    for rev in 0..rl.len() {
        let entry = rl.entry(rev).expect("entry");
        let truth =
            String::from_utf8(r.run(&["log", "-r", &rev.to_string(), "-T", "{node}"])).unwrap();
        assert_eq!(hex20(&entry.node), truth.trim(), "changelog node {rev}");
    }
    // The first changeset is a root (both parents null); the second's p1 is rev 0.
    assert_eq!(rl.entry(0).unwrap().p1, super::NULL_REV);
    assert_eq!(rl.entry(1).unwrap().p1, 0);
}

#[test]
fn a_big_file_uses_zstd_and_a_delta_that_reconstruct() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let rl = Revlog::open(&r.store().join("data/big.txt.i")).expect("open big.txt filelog");
    assert_eq!(rl.len(), 2, "two file revisions");
    // rev1 is a delta on rev0; reconstructing it must reproduce the appended content exactly.
    let rev1 = rl.revision(1).expect("reconstruct delta revision");
    let expected = "abcdefgh\n".repeat(4000) + "one more line\n";
    assert_eq!(
        rev1,
        expected.as_bytes(),
        "delta+zstd reconstruction is exact"
    );
}

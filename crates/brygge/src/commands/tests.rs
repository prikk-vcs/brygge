//! Command integration tests (handoff acceptance): build a fixture repo, then
//! decode → inspect → verify (internal + against-source) → summary, asserting outcomes and exit classes.
//! Skips if `git` is not on `PATH`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command as PCommand;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::cli::{Format, SourceKind};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn git_available() -> bool {
    PCommand::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

struct TempRepo {
    dir: PathBuf,
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl TempRepo {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "brygge-cli-test-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Self { dir };
        r.git(&["init", "-q", "--initial-branch=main"]);
        r
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn git(&self, args: &[&str]) -> String {
        let out = PCommand::new("git")
            .current_dir(&self.dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "A U Thor")
            .env("GIT_AUTHOR_EMAIL", "author@example.com")
            .env("GIT_AUTHOR_DATE", "2005-04-07T22:13:13 +0000")
            .env("GIT_COMMITTER_NAME", "C O Mitter")
            .env("GIT_COMMITTER_EMAIL", "committer@example.com")
            .env("GIT_COMMITTER_DATE", "2005-04-07T22:13:13 +0000")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(&self, file: &str, contents: &str, msg: &str) {
        std::fs::write(self.dir.join(file), contents).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", msg]);
    }
}

fn simple_repo(seed: &str) -> TempRepo {
    let r = TempRepo::new();
    r.commit("a.txt", &format!("{seed} one\n"), "c1");
    r.commit("a.txt", &format!("{seed} two\n"), "c2");
    r
}

#[test]
fn decode_inspect_verify_summary_roundtrip() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("x");
    let out = repo.dir.join("out.ir");

    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            Some(&out),
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    assert!(out.exists(), "the artifact was written");
    assert_eq!(run_inspect(&out, Format::Human), exit::CLEAN);
    assert_eq!(run_summary(&out, Format::Machine), exit::CLEAN);
    assert_eq!(run_verify_internal(&out, Format::Machine), exit::CLEAN);
    assert_eq!(
        run_verify_against_source(repo.path(), &out, Format::Human),
        exit::CLEAN,
        "the artifact corresponds to its source (VF-2)"
    );
}

#[test]
fn verify_internal_catches_tamper_with_no_source() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("y");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            Some(&out),
            false,
            Format::Machine
        ),
        exit::CLEAN
    );

    let mut bytes = std::fs::read(&out).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff; // corrupt the final blob byte
    std::fs::write(&out, &bytes).unwrap();

    assert_eq!(
        run_verify_internal(&out, Format::Machine),
        exit::VERIFY_FAILED,
        "a tampered artifact fails integrity, no source needed (VF-3)"
    );
}

#[test]
fn verify_against_source_detects_a_mismatch() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("x");
    let other = simple_repo("z"); // different content and history
    let other_out = other.dir.join("o.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            other.path(),
            Some(&other_out),
            false,
            Format::Machine
        ),
        exit::CLEAN
    );

    // Verifying one repo's artifact against a different source must fail.
    assert_eq!(
        run_verify_against_source(repo.path(), &other_out, Format::Human),
        exit::VERIFY_FAILED
    );
}

#[test]
fn decode_of_a_submodule_exits_floor_refusal() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("s");
    let head = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},sub"),
    ]);
    repo.git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "add gitlink",
    ]);
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            Some(&out),
            false,
            Format::Human
        ),
        exit::FLOOR_REFUSAL
    );
    assert!(!out.exists(), "no artifact is written on a refusal");
}

#[test]
fn missing_artifact_is_a_plain_failure() {
    let missing = std::env::temp_dir().join("brygge-cli-nope-does-not-exist.ir");
    assert_eq!(run_inspect(&missing, Format::Human), exit::FAILURE);
    assert_eq!(run_summary(&missing, Format::Machine), exit::FAILURE);
}

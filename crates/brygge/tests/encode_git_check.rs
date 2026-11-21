//! Level 2 (RFC 019 D-6, batch G2): `verify --against-target git <repository>` against a real
//! repository `brygge encode git` wrote, then tampered one way at a time. Every successful `encode git`
//! in `encode_git_fast_import.rs` already exercises the in-process check on the happy path; this file is
//! the standalone command, and the refusal side.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_brygge")
}

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "brygge-encode-git-check-test-{label}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git_available() -> bool {
    Command::new("git")
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
        let dir = scratch_dir("source");
        let r = Self { dir };
        r.git(&["init", "-q", "--initial-branch=main"]);
        r
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
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
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(&self, rel: &str, contents: &str) {
        std::fs::write(self.dir.join(rel), contents).unwrap();
    }

    fn commit(&self, msg: &str) {
        self.git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", msg]);
    }
}

fn write_artifact(ir: &brygge_ir::Ir) -> PathBuf {
    let dir = scratch_dir("artifact");
    let path = dir.join("artifact.ir");
    std::fs::write(&path, brygge_ir::to_bytes(ir)).unwrap();
    path
}

/// A fixture with two commits and one tag, encoded once. Returns the artifact and the encoded
/// repository's path (never `--out`'s own temporary: the caller tampers a *copy*).
fn encoded_fixture() -> (PathBuf, PathBuf) {
    let source = TempRepo::new();
    source.write("a.txt", "a\n");
    source.git(&["add", "a.txt"]);
    source.commit("first");
    source.write("a.txt", "a changed\n");
    source.git(&["add", "a.txt"]);
    source.commit("second");
    source.git(&["tag", "-a", "v1", "-m", "release one"]);

    let ir = brygge_decode_git::decode(source.path(), &brygge_decode_git::Options::default())
        .expect("decode the fixture");
    let artifact = write_artifact(&ir);
    let out = scratch_dir("encoded").join("repo.git");
    let output = Command::new(bin())
        .args(["encode", "git"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("run brygge encode git");
    assert!(
        output.status.success(),
        "encode git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (artifact, out)
}

fn run_against_target(artifact: &Path, repo: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("verify")
        .arg(artifact)
        .args(["--against-target", "git"])
        .arg(repo)
        .args(["--format", "machine"])
        .output()
        .expect("run brygge verify --against-target")
}

fn copy_repo(src: &Path) -> PathBuf {
    let dest = scratch_dir("tampered");
    let status = Command::new("cp")
        .args(["-r"])
        .arg(src)
        .arg(&dest)
        .status()
        .expect("cp -r");
    assert!(status.success());
    dest.join(src.file_name().unwrap())
}

fn git_in(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(repo)
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
        .expect("run git")
}

#[test]
fn a_real_encoded_repository_corresponds() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let (artifact, repo) = encoded_fixture();
    let out = run_against_target(&artifact, &repo);
    assert!(out.status.success(), "{:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("verify.against_target=corresponds"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn an_amended_commit_message_does_not_correspond() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let (artifact, repo) = encoded_fixture();
    let tampered = copy_repo(&repo);
    // A brand-new commit with the same tree and parent as the current HEAD, but a different message —
    // the tamper is exactly one atom's message, nothing else.
    let new_commit = git_in(
        &tampered,
        &[
            "commit-tree",
            "-p",
            "HEAD^",
            "-m",
            "tampered message",
            "HEAD^{tree}",
        ],
    );
    assert!(
        new_commit.status.success(),
        "commit-tree failed: {}",
        String::from_utf8_lossy(&new_commit.stderr)
    );
    let new_oid = String::from_utf8_lossy(&new_commit.stdout)
        .trim()
        .to_string();
    let status = git_in(&tampered, &["update-ref", "refs/heads/main", &new_oid]);
    assert!(status.status.success());

    let out = run_against_target(&artifact, &tampered);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("verify.against_target=does-not-correspond"),
        "{stdout}"
    );
    assert_eq!(out.status.code(), Some(50));
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&tampered);
}

#[test]
fn an_added_branch_does_not_correspond_nothing_extra() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let (artifact, repo) = encoded_fixture();
    let tampered = copy_repo(&repo);
    let head = git_in(&tampered, &["rev-parse", "HEAD"]);
    let head_oid = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let status = git_in(&tampered, &["update-ref", "refs/heads/extra", &head_oid]);
    assert!(status.status.success());

    let out = run_against_target(&artifact, &tampered);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("verify.against_target=does-not-correspond"),
        "{stdout}"
    );
    assert!(
        stdout.contains("extra") || stdout.contains("nothing+extra"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&tampered);
}

#[test]
fn a_moved_tag_does_not_correspond() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let (artifact, repo) = encoded_fixture();
    let tampered = copy_repo(&repo);
    let parent = git_in(&tampered, &["rev-parse", "HEAD^"]);
    let parent_oid = String::from_utf8_lossy(&parent.stdout).trim().to_string();
    // Move the lightweight ref the tag's own ref points at (force it onto the parent commit instead).
    let tag_ref = git_in(&tampered, &["rev-parse", "refs/tags/v1"]);
    assert!(tag_ref.status.success(), "{:?}", tag_ref);
    let status = git_in(&tampered, &["update-ref", "refs/tags/v1", &parent_oid]);
    assert!(status.status.success());

    let out = run_against_target(&artifact, &tampered);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("verify.against_target=does-not-correspond"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&tampered);
}

#[test]
fn a_changed_head_does_not_correspond() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let (artifact, repo) = encoded_fixture();
    let tampered = copy_repo(&repo);
    // Detach HEAD, so it no longer symbolically names the default branch.
    let head = git_in(&tampered, &["rev-parse", "HEAD"]);
    let head_oid = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let status = Command::new("git")
        .current_dir(&tampered)
        .args(["symbolic-ref", "HEAD", "refs/heads/other"])
        .output()
        .expect("git symbolic-ref");
    assert!(status.status.success());
    let status = git_in(&tampered, &["update-ref", "refs/heads/other", &head_oid]);
    assert!(status.status.success());

    let out = run_against_target(&artifact, &tampered);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("verify.against_target=does-not-correspond"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&tampered);
}

#[test]
fn against_source_and_against_target_together_is_a_usage_error() {
    let artifact = {
        let dir = scratch_dir("artifact-usage");
        let path = dir.join("artifact.ir");
        std::fs::write(&path, b"not a real artifact").unwrap();
        path
    };
    let out = Command::new(bin())
        .arg("verify")
        .arg(&artifact)
        .args(["--against-source", "."])
        .args(["--against-target", "git", "."])
        .output()
        .expect("run brygge verify");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("one question per run"), "{stderr}");
}

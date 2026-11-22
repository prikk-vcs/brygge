//! Level 2 (RFC 019 Amendment A, D-9, request 074): `decode svn --branch-lines` through a real
//! `svnadmin`-built repository, then the real `brygge encode git`, checked with real `git`. Also the
//! handoff's second stop-and-report condition: whether a deleted/earlier-life ref's `@` separator
//! passes real `git check-ref-format`.

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
        "brygge-svn-branch-lines-test-{label}-{}-{n}",
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

fn svnadmin_available() -> bool {
    Command::new("svnadmin")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A real local SVN repository, built with real `svnadmin`/`svn`: one trunk commit, a branch copied
/// from it, an edit on the branch, then the branch deleted — so the dump has both a live line (trunk)
/// and a dead one (the branch), exactly what D-9's ref rules need to prove for real.
fn build_svn_repo() -> PathBuf {
    let root = scratch_dir("svn-repo");
    let repo = root.join("repo");
    assert!(
        Command::new("svnadmin")
            .args(["create", repo.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    let url = format!("file://{}", repo.display());
    let wc = root.join("wc");
    assert!(
        Command::new("svn")
            .args(["checkout", "-q", &url, wc.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    let commit = |args: &[&str], msg: &str| {
        assert!(
            Command::new("svn")
                .args(args)
                .current_dir(&wc)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("svn")
                .args(["commit", "-q", "-m", msg])
                .current_dir(&wc)
                .status()
                .unwrap()
                .success()
        );
    };
    std::fs::create_dir_all(wc.join("trunk")).unwrap();
    std::fs::create_dir_all(wc.join("branches")).unwrap();
    std::fs::create_dir_all(wc.join("tags")).unwrap();
    std::fs::write(wc.join("trunk/a.txt"), "one\n").unwrap();
    commit(
        &["add", "trunk", "branches", "tags"],
        "lay out trunk/branches/tags",
    );
    commit(
        &["copy", "trunk", "branches/stable"],
        "branch stable from trunk",
    );
    std::fs::write(wc.join("branches/stable/a.txt"), "edited on stable\n").unwrap();
    assert!(
        Command::new("svn")
            .args(["commit", "-q", "-m", "edit on stable"])
            .current_dir(&wc)
            .status()
            .unwrap()
            .success()
    );
    // Delete the branch to exercise D-9's dead-line/ref rule.
    assert!(
        Command::new("svn")
            .args(["update", "-q"])
            .current_dir(&wc)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("svn")
            .args(["delete", "branches/stable"])
            .current_dir(&wc)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("svn")
            .args(["commit", "-q", "-m", "prune stable"])
            .current_dir(&wc)
            .status()
            .unwrap()
            .success()
    );
    repo
}

fn run_decode_branch_lines(repo: &Path, out: &Path) -> std::process::Output {
    Command::new(bin())
        .args(["decode", "svn"])
        .arg(repo)
        .arg("--out")
        .arg(out)
        .arg("--branch-lines")
        .output()
        .expect("run brygge decode svn --branch-lines")
}

fn run_encode_git(artifact: &Path, out: &Path) -> std::process::Output {
    Command::new(bin())
        .args(["encode", "git"])
        .arg(artifact)
        .arg("--out")
        .arg(out)
        .output()
        .expect("run brygge encode git")
}

#[test]
fn a_real_svn_repository_with_a_deleted_branch_encodes_with_both_live_and_dead_refs() {
    if !svnadmin_available() {
        eprintln!("skipping: svnadmin not on PATH");
        return;
    }
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = build_svn_repo();
    let artifact = scratch_dir("artifact").join("repo.ir");
    let decode_result = run_decode_branch_lines(&repo, &artifact);
    assert!(
        decode_result.status.success() || decode_result.status.code() == Some(10),
        "decode should succeed (plain, or exit 10 for flagged fidelity): {}",
        String::from_utf8_lossy(&decode_result.stderr)
    );
    assert!(artifact.exists());

    let out = scratch_dir("result").join("repo.git");
    let encode_result = run_encode_git(&artifact, &out);
    assert!(
        encode_result.status.success()
            || [10, 30].contains(&encode_result.status.code().unwrap_or(-1)),
        "encode should succeed: {}",
        String::from_utf8_lossy(&encode_result.stderr)
    );
    assert!(out.exists());

    // `git fsck --strict` on the result: the usual structural sanity check.
    let fsck = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .args(["fsck", "--strict"])
        .output()
        .unwrap();
    assert!(
        fsck.status.success(),
        "git fsck --strict: {}",
        String::from_utf8_lossy(&fsck.stderr)
    );

    // The live line (trunk) is an ordinary branch; the dead line (stable) lives under
    // refs/brygge/other/, named with its last revision.
    let refs = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .args(["for-each-ref", "--format=%(refname)"])
        .output()
        .unwrap();
    let refs_text = String::from_utf8_lossy(&refs.stdout);
    assert!(
        refs_text.lines().any(|l| l == "refs/heads/trunk"),
        "{refs_text}"
    );
    let dead_ref = refs_text
        .lines()
        .find(|l| l.starts_with("refs/brygge/other/branches/stable@r"))
        .unwrap_or_else(|| panic!("no dead-life ref for branches/stable in:\n{refs_text}"));

    // The second stop-and-report condition (D-9 §2): does `@` in a ref name upset real
    // `git check-ref-format`? Answered here, against the real name this run actually produced.
    let check = Command::new("git")
        .args(["check-ref-format", dead_ref])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "git check-ref-format rejected {dead_ref}: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn at_sign_in_a_ref_name_passes_git_check_ref_format() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    for name in [
        "refs/brygge/other/branches/stable@r5",
        "refs/brygge/other/tags/release-1.0@r812",
    ] {
        let check = Command::new("git")
            .args(["check-ref-format", name])
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "git check-ref-format rejected {name}: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

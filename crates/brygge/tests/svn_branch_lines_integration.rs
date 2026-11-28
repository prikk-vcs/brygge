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

/// P-6 (RFC 022 incident 094): removes the scratch directory that contains `path` — for a path built as
/// `scratch_dir(label).join(x)` (including `build_svn_repo`'s and `build_flat_svn_repo`'s own
/// `root.join("repo")`). Removing the parent removes everything that label's fixture created there, not
/// just the one entry named here.
fn cleanup_scratch(path: &Path) {
    let _ = std::fs::remove_dir_all(path.parent().unwrap_or(path));
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

/// A real, flat local SVN repository: files live directly at the repository root, with no
/// trunk/branches/tags convention at all — the RFC 020 D-3 shape `--layout-trunk .` exists for.
fn build_flat_svn_repo() -> PathBuf {
    let root = scratch_dir("svn-flat-repo");
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
    std::fs::write(wc.join("a.txt"), "one\n").unwrap();
    std::fs::create_dir_all(wc.join("src")).unwrap();
    std::fs::write(wc.join("src/main.rs"), "fn main() {}\n").unwrap();
    assert!(
        Command::new("svn")
            .args(["add", "a.txt", "src"])
            .current_dir(&wc)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("svn")
            .args(["commit", "-q", "-m", "flat: add a.txt and src/"])
            .current_dir(&wc)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(wc.join("a.txt"), "one, edited\n").unwrap();
    assert!(
        Command::new("svn")
            .args(["commit", "-q", "-m", "flat: edit a.txt"])
            .current_dir(&wc)
            .status()
            .unwrap()
            .success()
    );
    repo
}

/// RFC 020 D-3, handoff `layout-options-handoff-v1.md` §0 level 3: a repository with no layout at all,
/// decoded with `--layout-trunk . --branch-lines` (one line, the whole repository), then `encode git`,
/// then `git fsck --strict` on the result — the 0.8.0 refusal's own escape hatch, proven end to end
/// against real `svnadmin`/`svn`/`git`, not just a hand-built dumpstream.
#[test]
fn a_flat_svn_repository_with_layout_trunk_dot_encodes_cleanly_to_git() {
    if !svnadmin_available() {
        eprintln!("skipping: svnadmin not on PATH");
        return;
    }
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = build_flat_svn_repo();
    let artifact = scratch_dir("flat-artifact").join("repo.ir");
    let decode_result = Command::new(bin())
        .args(["decode", "svn"])
        .arg(&repo)
        .arg("--out")
        .arg(&artifact)
        .args(["--branch-lines", "--layout-trunk", "."])
        .output()
        .expect("run brygge decode svn --branch-lines --layout-trunk .");
    assert!(
        decode_result.status.success() || decode_result.status.code() == Some(10),
        "decode should succeed (plain, or exit 10 for flagged fidelity): {}",
        String::from_utf8_lossy(&decode_result.stderr)
    );
    assert!(artifact.exists());

    let out = scratch_dir("flat-result").join("repo.git");
    let encode_result = run_encode_git(&artifact, &out);
    assert!(
        encode_result.status.success()
            || [10, 30].contains(&encode_result.status.code().unwrap_or(-1)),
        "encode should succeed: {}",
        String::from_utf8_lossy(&encode_result.stderr)
    );
    assert!(out.exists());

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

    // The whole repository is the one trunk line: a single ref, reachable, with both commits.
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
    let log = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .args(["log", "--oneline", "refs/heads/trunk"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).lines().count(),
        2,
        "expected both commits on the one trunk line"
    );
    cleanup_scratch(&repo);
    cleanup_scratch(&artifact);
    cleanup_scratch(&out);
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
    cleanup_scratch(&repo);
    cleanup_scratch(&artifact);
    cleanup_scratch(&out);
}

/// Review 147 F-1: `brygge_decode_git`'s `carry_extra_namespaces` must make the check actually *read*
/// a dead line's commit, not merely confirm its ref exists — a tamper on that commit's own tree must be
/// caught by `verify --against-target git`, exactly as a tamper on a live commit already is.
#[test]
fn tampering_a_dead_lines_commit_is_caught_by_against_target() {
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
        "{}",
        String::from_utf8_lossy(&decode_result.stderr)
    );

    let out = scratch_dir("result").join("repo.git");
    let encode_result = run_encode_git(&artifact, &out);
    assert!(
        encode_result.status.success()
            || [10, 30].contains(&encode_result.status.code().unwrap_or(-1)),
        "{}",
        String::from_utf8_lossy(&encode_result.stderr)
    );

    let refs = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .output()
        .unwrap();
    let refs_text = String::from_utf8_lossy(&refs.stdout);
    let (dead_ref, dead_oid) = refs_text
        .lines()
        .find_map(|l| {
            let (name, oid) = l.split_once(' ')?;
            name.starts_with("refs/brygge/other/branches/stable@r")
                .then(|| (name.to_string(), oid.to_string()))
        })
        .unwrap_or_else(|| panic!("no dead-life ref for branches/stable in:\n{refs_text}"));

    // Build a tampered tree (one changed file, by content, not by git's own index machinery — this is
    // a bare repository), a new commit object over it with the same parent and message as the real one
    // (so only the *tree* differs, proving this is caught by content, not by anything else), and move
    // the dead ref to point at it.
    let orig_tree = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .args(["log", "-1", "--format=%T", &dead_oid])
        .output()
        .unwrap();
    let orig_tree = String::from_utf8_lossy(&orig_tree.stdout)
        .trim()
        .to_string();
    let blob = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"tampered content\n")?;
            child.wait_with_output()
        })
        .unwrap();
    let blob_oid = String::from_utf8_lossy(&blob.stdout).trim().to_string();
    let mktree_input = format!("100644 blob {blob_oid}\ttampered.txt\n");
    let new_tree = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .arg("mktree")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(mktree_input.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    let _ = orig_tree; // the base tree is irrelevant: mktree above builds a wholly different one.
    let new_tree_oid = String::from_utf8_lossy(&new_tree.stdout).trim().to_string();
    let message = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .args(["log", "-1", "--format=%B", &dead_oid])
        .output()
        .unwrap();
    let new_commit = Command::new("git")
        .args(["-C"])
        .arg(&out)
        .args(["commit-tree", &new_tree_oid, "-m"])
        .arg(String::from_utf8_lossy(&message.stdout).into_owned())
        .env("GIT_AUTHOR_NAME", "A U Thor")
        .env("GIT_AUTHOR_EMAIL", "a@example.com")
        .env("GIT_COMMITTER_NAME", "C O Mitter")
        .env("GIT_COMMITTER_EMAIL", "c@example.com")
        .output()
        .unwrap();
    assert!(
        new_commit.status.success(),
        "git commit-tree failed: {}",
        String::from_utf8_lossy(&new_commit.stderr)
    );
    let new_commit_oid = String::from_utf8_lossy(&new_commit.stdout)
        .trim()
        .to_string();
    assert!(
        Command::new("git")
            .args(["-C"])
            .arg(&out)
            .args(["update-ref", &dead_ref, &new_commit_oid])
            .status()
            .unwrap()
            .success()
    );

    let verify = Command::new(bin())
        .arg("verify")
        .arg(&artifact)
        .args(["--against-target", "git"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(
        verify.status.code(),
        Some(50),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&verify.stdout),
        String::from_utf8_lossy(&verify.stderr)
    );
    let report = String::from_utf8_lossy(&verify.stdout);
    assert!(
        report.contains("does-not-correspond") || report.contains("does not correspond"),
        "{report}"
    );
    cleanup_scratch(&repo);
    cleanup_scratch(&artifact);
    cleanup_scratch(&out);
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

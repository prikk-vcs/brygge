//! End-to-end subprocess tests against the real compiled binary (handoff `cli-and-verify-handoff-v2.md`
//! §5.2): properties only observable at the process boundary — real stderr, real exit codes — that a
//! unit test inside `src/` cannot see (`CARGO_BIN_EXE_brygge` is only set for `tests/*.rs`). Skips
//! without `git` on `PATH`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const USAGE: i32 = 2;
const FLOOR_REFUSAL: i32 = 20;

static COUNTER: AtomicU64 = AtomicU64::new(0);

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
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "brygge-cli-itest-{}-{nanos}-{n}",
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
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(&self, file: &str, contents: &str, msg: &str) {
        std::fs::write(self.dir.join(file), contents).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", msg]);
    }
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_brygge")
}

#[test]
fn faithfulness_statement_reaches_stderr_even_when_decode_then_fails() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = TempRepo::new();
    repo.commit("a.txt", "one\n", "c1");
    // A submodule triggers a floor refusal (decode fails) after the statement must already be printed.
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
    let output = Command::new(bin())
        .args(["decode", "git"])
        .arg(repo.path())
        .arg("--out")
        .arg(&out)
        .output()
        .expect("run brygge decode");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(FLOOR_REFUSAL),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(
            "Git: content, history and messages are carried as the source recorded them."
        ),
        "the faithfulness statement must print even though decode then fails: {stderr}"
    );
    assert!(!out.exists(), "no artifact is written on a refusal");
}

#[test]
fn faithfulness_statement_reaches_stderr_on_a_successful_decode_too() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = TempRepo::new();
    repo.commit("a.txt", "one\n", "c1");
    let out = repo.dir.join("out.ir");
    let output = Command::new(bin())
        .args(["decode", "git"])
        .arg(repo.path())
        .arg("--out")
        .arg(&out)
        .output()
        .expect("run brygge decode");
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Git: content, history and messages"));
    assert!(stderr.contains(&format!("wrote {}", out.display())));
}

#[test]
fn a_usage_error_exits_2_and_names_the_problem() {
    let output = Command::new(bin())
        .args(["decode", "git", "/does-not-matter"])
        .output()
        .expect("run brygge decode without --out");
    assert_eq!(output.status.code(), Some(USAGE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--out"), "{stderr}");
    assert!(stderr.contains("brygge decode --help"), "{stderr}");
}

#[test]
fn an_inapplicable_option_exits_2_with_the_exact_message() {
    let output = Command::new(bin())
        .args([
            "decode",
            "svn",
            "/does-not-matter",
            "--out",
            "o.ir",
            "--infer-renames",
        ])
        .output()
        .expect("run brygge decode");
    assert_eq!(output.status.code(), Some(USAGE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--infer-renames applies to git, hg, not to svn"),
        "{stderr}"
    );
}

#[test]
fn encode_exits_2_with_its_own_message() {
    let output = Command::new(bin())
        .arg("encode")
        .output()
        .expect("run brygge encode");
    assert_eq!(output.status.code(), Some(USAGE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("'encode' is not available yet"), "{stderr}");
}

#[test]
fn help_lists_only_existing_commands() {
    let output = Command::new(bin())
        .arg("--help")
        .output()
        .expect("run brygge --help");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("decode"));
    assert!(stdout.contains("inspect"));
    assert!(stdout.contains("verify"));
    assert!(
        !stdout.contains("summary"),
        "the removed verb must not be listed"
    );
    assert!(
        !stdout.contains("encode"),
        "no placeholder for a command that does not exist yet (review 003 R-4, CL-03/CL-06)"
    );
}

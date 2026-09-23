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
        stderr.contains("--infer-renames applies to git, not to svn"),
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

#[test]
fn infer_renames_for_hg_exits_2_because_mercurial_records_its_renames() {
    let output = Command::new(bin())
        .args([
            "decode",
            "hg",
            "/does-not-matter",
            "--out",
            "o.ir",
            "--infer-renames",
        ])
        .output()
        .expect("run brygge decode hg --infer-renames");
    assert_eq!(output.status.code(), Some(USAGE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--infer-renames applies to git, not to hg"),
        "{stderr}"
    );
    assert!(stderr.contains("brygge decode --help"), "{stderr}");
    // ... and the help says so too.
    let help = Command::new(bin()).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("--infer-renames (git)"), "{help}");
}

/// Every machine line is `key=value` with a key of dot-separated `[a-z0-9_]` segments.
fn assert_keys_follow_the_rule(stdout: &str, what: &str) {
    for line in stdout.lines() {
        let (key, _) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("{what}: a machine line without `=`: {line:?}"));
        assert!(!key.is_empty(), "{what}: empty key in {line:?}");
        for seg in key.split('.') {
            assert!(
                !seg.is_empty()
                    && seg
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'),
                "{what}: key {key:?} breaks `^[a-z0-9_]+(\\.[a-z0-9_]+)*$`"
            );
        }
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap(), 16).unwrap(),
            );
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

#[test]
fn a_real_run_prints_only_rule_following_keys_one_atoms_line_and_the_exact_message_bytes() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = TempRepo::new();
    repo.commit("old.txt", "some content that will move\n", "c1");
    // A non-UTF-8 commit message (Latin-1 é). Git converts an invalid-UTF-8 message to UTF-8 unless the
    // repository declares its commit encoding, so declare Latin-1: the bytes are then stored as given.
    std::fs::rename(repo.dir.join("old.txt"), repo.dir.join("new.txt")).unwrap();
    repo.git(&["add", "-A"]);
    let msg_file = repo
        .dir
        .join("..")
        .join(format!("brygge-msg-{}.txt", std::process::id()));
    std::fs::write(&msg_file, b"caf\xe9 100%\n").unwrap();
    repo.git(&[
        "-c",
        "commit.gpgsign=false",
        "-c",
        "i18n.commitEncoding=ISO-8859-1",
        "commit",
        "-q",
        "-F",
        msg_file.to_str().unwrap(),
    ]);
    let _ = std::fs::remove_file(&msg_file);

    let out = repo.dir.join("out.ir");
    let decode = Command::new(bin())
        .args(["decode", "git"])
        .arg(repo.path())
        .arg("--out")
        .arg(&out)
        .args(["--infer-renames", "--format", "machine"])
        .output()
        .unwrap();
    let decode_out = String::from_utf8(decode.stdout).unwrap();
    assert_keys_follow_the_rule(&decode_out, "decode");
    assert!(
        decode_out.lines().any(|l| l == "report_version=3"),
        "{decode_out}"
    );
    assert!(
        decode_out
            .lines()
            .any(|l| l == "skipped_non_critical_fields=0")
    );
    assert!(
        decode_out
            .lines()
            .any(|l| l.starts_with("derived.inferred_rename=")),
        "{decode_out}"
    );

    let inspect = Command::new(bin())
        .args(["inspect"])
        .arg(&out)
        .args(["--atoms", "--format", "machine"])
        .output()
        .unwrap();
    assert_eq!(inspect.status.code(), Some(0));
    let inspect_out = String::from_utf8(inspect.stdout).unwrap();
    assert_keys_follow_the_rule(&inspect_out, "inspect --atoms");
    assert_eq!(
        inspect_out
            .lines()
            .filter(|l| l.starts_with("atoms="))
            .count(),
        1,
        "one key, one line:\n{inspect_out}"
    );
    assert!(inspect_out.lines().any(|l| l == "inspect_version=4"));
    let message = inspect_out
        .lines()
        .filter_map(|l| l.strip_prefix("atom.1.message="))
        .next()
        .unwrap_or_else(|| panic!("no atom.1.message in\n{inspect_out}"));
    assert_eq!(
        percent_decode(message),
        b"caf\xe9 100%\n",
        "the exact bytes"
    );

    let verify = Command::new(bin())
        .args(["verify"])
        .arg(&out)
        .args(["--format", "machine"])
        .output()
        .unwrap();
    assert_eq!(verify.status.code(), Some(0));
    let verify_out = String::from_utf8(verify.stdout).unwrap();
    assert_keys_follow_the_rule(&verify_out, "verify");
    for line in [
        "verify_version=4",
        "verify.skipped_non_critical_fields=0",
        "verify.check.source_invariants=pass",
        "verify.check.loss_boundary=pass",
        "verify.result=pass",
    ] {
        assert!(
            verify_out.lines().any(|l| l == line),
            "{line} in\n{verify_out}"
        );
    }
}

#[test]
fn a_check_that_could_not_run_says_not_checked_in_both_forms_and_its_detail_follows_its_line() {
    // An artifact that fails to decode: `integrity` fails, and every other check cannot run.
    let path = std::env::temp_dir().join(format!("brygge-garbage-{}.ir", std::process::id()));
    std::fs::write(&path, b"this is not an artifact").unwrap();

    let machine = Command::new(bin())
        .args(["verify"])
        .arg(&path)
        .args(["--format", "machine"])
        .output()
        .unwrap();
    assert_eq!(machine.status.code(), Some(50));
    let m = String::from_utf8(machine.stdout).unwrap();
    assert_keys_follow_the_rule(&m, "verify (garbage)");
    let lines: Vec<&str> = m.lines().collect();
    let structure = lines
        .iter()
        .position(|l| *l == "verify.check.structure=not-checked")
        .unwrap_or_else(|| panic!("{m}"));
    assert!(
        lines[structure + 1].starts_with("verify.check.structure.detail="),
        "{m}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("verify.check.integrity.detail="))
    );
    assert!(!m.contains("n/a"), "{m}");

    let human = Command::new(bin())
        .args(["verify"])
        .arg(&path)
        .output()
        .unwrap();
    let h = String::from_utf8(human.stdout).unwrap();
    assert!(h.contains("[not-checked] structure"), "{h}");
    assert!(!h.contains("n/a"), "{h}");
    let _ = std::fs::remove_file(&path);
}

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

// ---- Subversion (RFC 006) -------------------------------------------------------------------------

fn svnadmin_available() -> bool {
    PCommand::new("svnadmin")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A property block (`K <len>\n<key>\nV <len>\n<value>\n...PROPS-END\n`).
fn svn_props(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in pairs {
        b.extend(format!("K {}\n", k.len()).into_bytes());
        b.extend(k.as_bytes());
        b.push(b'\n');
        b.extend(format!("V {}\n", v.len()).into_bytes());
        b.extend(v.as_bytes());
        b.push(b'\n');
    }
    b.extend(b"PROPS-END\n");
    b
}

fn svn_revision(buf: &mut Vec<u8>, num: u64, props: &[(&str, &str)]) {
    let p = svn_props(props);
    buf.extend(format!("Revision-number: {num}\n").into_bytes());
    buf.extend(format!("Prop-content-length: {}\n", p.len()).into_bytes());
    buf.extend(format!("Content-length: {}\n\n", p.len()).into_bytes());
    buf.extend(p);
    buf.extend(b"\n");
}

fn svn_add_file(buf: &mut Vec<u8>, path: &str, content: &[u8]) {
    let p = svn_props(&[]);
    buf.extend(format!("Node-path: {path}\nNode-kind: file\nNode-action: add\n").into_bytes());
    buf.extend(format!("Prop-content-length: {}\n", p.len()).into_bytes());
    buf.extend(format!("Text-content-length: {}\n", content.len()).into_bytes());
    buf.extend(format!("Content-length: {}\n\n", p.len() + content.len()).into_bytes());
    buf.extend(p);
    buf.extend(content);
    buf.extend(b"\n\n");
}

fn svn_add_dir(buf: &mut Vec<u8>, path: &str, copyfrom: Option<(u64, &str)>) {
    buf.extend(format!("Node-path: {path}\nNode-kind: dir\nNode-action: add\n").into_bytes());
    if let Some((r, p)) = copyfrom {
        buf.extend(format!("Node-copyfrom-rev: {r}\nNode-copyfrom-path: {p}\n").into_bytes());
    }
    buf.extend(b"\n\n");
}

/// A dump with a trunk and a branch copy (so ref reconstruction has something to find).
fn svn_dump_with_branch() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(b"SVN-fs-dump-format-version: 2\n\n");
    b.extend(b"UUID: 33333333-4444-5555-6666-777777777777\n\n");
    svn_revision(&mut b, 0, &[("svn:date", "2024-02-01T00:00:00.000000Z")]);
    svn_revision(
        &mut b,
        1,
        &[
            ("svn:author", "carol"),
            ("svn:date", "2024-02-02T00:00:00.000000Z"),
            ("svn:log", "trunk"),
        ],
    );
    svn_add_dir(&mut b, "trunk", None);
    svn_add_file(&mut b, "trunk/a.txt", b"one\n");
    svn_revision(
        &mut b,
        2,
        &[
            ("svn:author", "carol"),
            ("svn:date", "2024-02-03T00:00:00.000000Z"),
            ("svn:log", "branch"),
        ],
    );
    svn_add_dir(&mut b, "branches", None);
    svn_add_dir(&mut b, "branches/x", Some((1, "trunk")));
    b
}

fn write_temp_dump(bytes: &[u8]) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("brygge-cli-svn-{}-{n}.dump", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn decode_svn_from_a_dumpfile_reconstructs_refs_and_verifies() {
    let dump = write_temp_dump(&svn_dump_with_branch());
    let out = std::env::temp_dir().join(format!("brygge-cli-svn-{}.ir", std::process::id()));

    // decode with --reconstruct-refs (a file path selects the DumpFile source).
    let code = run_decode(
        SourceKind::Svn,
        &dump,
        Some(&out),
        false,
        true,
        Format::Machine,
    );
    // trunk + branch reconstruction is Derived; mergeinfo/custom absent, so at most representation drops.
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "unexpected exit {code}"
    );
    assert!(out.exists());

    assert_eq!(run_inspect(&out, Format::Human), exit::CLEAN);
    assert_eq!(run_verify_internal(&out, Format::Machine), exit::CLEAN);
    // against-source re-decodes the same dumpfile, reproducing the reconstruct_refs/layout from provenance.
    assert_eq!(
        run_verify_against_source(&dump, &out, Format::Human),
        exit::CLEAN,
        "the artifact corresponds to its dumpfile (VF-2)"
    );

    let _ = std::fs::remove_file(&dump);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn decode_svn_flat_layout_with_reconstruct_is_a_convention_violation() {
    // A flat layout (no trunk/branches/tags) with reconstruction requested → CL-08 convention violation.
    let mut b = Vec::new();
    b.extend(b"SVN-fs-dump-format-version: 2\n\n");
    b.extend(b"UUID: 88888888-9999-0000-1111-222222222222\n\n");
    svn_revision(&mut b, 0, &[("svn:date", "2024-02-01T00:00:00.000000Z")]);
    svn_revision(
        &mut b,
        1,
        &[
            ("svn:log", "flat"),
            ("svn:date", "2024-02-02T00:00:00.000000Z"),
        ],
    );
    svn_add_file(&mut b, "main.c", b"x\n");
    let dump = write_temp_dump(&b);

    let code = run_decode(SourceKind::Svn, &dump, None, false, true, Format::Machine);
    assert_eq!(code, exit::CONVENTION_VIOLATION);
    let _ = std::fs::remove_file(&dump);
}

#[test]
fn decode_svn_from_a_live_repository_via_svnadmin() {
    if !svnadmin_available() {
        eprintln!("skipping: svnadmin not on PATH");
        return;
    }
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let repo = std::env::temp_dir().join(format!("brygge-cli-svnrepo-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    assert!(
        PCommand::new("svnadmin")
            .arg("create")
            .arg(&repo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    );
    // Load a dump so the repo has real history, then decode the repo directory (svnadmin dump subprocess).
    use std::io::Write as _;
    let mut child = PCommand::new("svnadmin")
        .arg("load")
        .arg(&repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn svnadmin load");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&svn_dump_with_branch())
        .unwrap();
    assert!(child.wait().map(|s| s.success()).unwrap_or(false));

    let out = repo.join("out.ir");
    let code = run_decode(
        SourceKind::Svn,
        &repo,
        Some(&out),
        false,
        true,
        Format::Machine,
    );
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "unexpected exit {code}"
    );
    assert!(out.exists());
    assert_eq!(run_verify_internal(&out, Format::Machine), exit::CLEAN);

    let _ = std::fs::remove_dir_all(&repo);
}

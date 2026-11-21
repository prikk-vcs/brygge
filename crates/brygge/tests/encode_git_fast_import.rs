//! Level 2 (RFC 019 batch G1 §0): end-to-end proof of the built-in `brygge encode git` against a real
//! `git`, over fixtures from all four decoders, and the determinism/environment-isolation properties
//! (D-5, D-1). The real corpus runs (D-8) are batch G4's job, not this file's.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_brygge")
}

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "brygge-encode-git-fi-test-{label}-{}-{n}",
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

fn hg_available() -> bool {
    Command::new("hg")
        .arg("--version")
        .env("HGRCPATH", "/dev/null")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---- a Git source fixture (merges, executable, symlink, rename, annotated tag) -----------------------

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
        let dir = scratch_dir("git-repo");
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
        let p = self.dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    fn commit_only(&self, msg: &str) {
        self.git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", msg]);
    }

    fn add_symlink(&self, rel: &str, target: &str) {
        let sha = self.git_with_stdin(&["hash-object", "-w", "--stdin"], target);
        self.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{sha},{rel}"),
        ]);
    }

    fn make_executable(&self, rel: &str) {
        self.git(&["add", rel]);
        self.git(&["update-index", "--chmod=+x", rel]);
    }

    fn git_with_stdin(&self, args: &[&str], input: &str) -> String {
        let mut child = Command::new("git")
            .current_dir(&self.dir)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn git");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}

fn build_round_trip_fixture() -> TempRepo {
    let r = TempRepo::new();
    r.write("readme.txt", "hello\n");
    r.write("old-name.txt", "will be renamed\n");
    r.write("run.sh", "#!/bin/sh\necho hi\n");
    r.git(&["add", "-A"]);
    r.make_executable("run.sh");
    r.add_symlink("link-to-readme", "readme.txt");
    r.commit_only("initial");

    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("feature.txt", "a feature\n");
    r.git(&["add", "feature.txt"]);
    r.commit_only("add feature");

    r.git(&["checkout", "-q", "main"]);
    r.git(&["mv", "old-name.txt", "new-name.txt"]);
    r.commit_only("rename old-name.txt to new-name.txt");

    r.git(&[
        "-c",
        "commit.gpgsign=false",
        "merge",
        "-q",
        "--no-ff",
        "-m",
        "merge feature",
        "feature",
    ]);
    r.git(&["tag", "-a", "v1", "-m", "release one"]);
    r
}

fn write_artifact(ir: &brygge_ir::Ir) -> PathBuf {
    let dir = scratch_dir("artifact");
    let path = dir.join("artifact.ir");
    std::fs::write(&path, brygge_ir::to_bytes(ir)).unwrap();
    path
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

/// One commit's comparable facts, from `git log`.
#[derive(Debug, PartialEq, Eq)]
struct CommitFacts {
    tree: String,
    author_name: String,
    author_email: String,
    author_date: String,
    message: String,
}

fn strip_trailers(full: &str) -> &str {
    full.split("\n\nBrygge-Encoded:")
        .next()
        .unwrap()
        .trim_end_matches('\n')
}

fn commit_facts(dir: &Path, rev: &str) -> Vec<CommitFacts> {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "log",
            "--topo-order",
            "--reverse",
            "--format=%H%x1f%T%x1f%an%x1f%ae%x1f%ad%x1f%B%x1e",
            rev,
        ])
        .output()
        .expect("git log");
    assert!(out.status.success(), "{:?}", out);
    String::from_utf8_lossy(&out.stdout)
        .split('\x1e')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|record| {
            let mut fields = record.splitn(6, '\x1f');
            let _hash = fields.next().unwrap();
            let tree = fields.next().unwrap().to_string();
            let author_name = fields.next().unwrap().to_string();
            let author_email = fields.next().unwrap().to_string();
            let author_date = fields.next().unwrap().to_string();
            let message = strip_trailers(fields.next().unwrap().trim_end()).to_string();
            CommitFacts {
                tree,
                author_name,
                author_email,
                author_date,
                message,
            }
        })
        .collect()
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {:?}", out);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn the_git_round_trip_reproduces_tree_message_author_and_refs() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let source = build_round_trip_fixture();
    let ir =
        brygge_decode_git::decode(source.path(), &brygge_decode_git::Options::default()).unwrap();
    let artifact = write_artifact(&ir);
    let out = scratch_dir("result").join("repo");

    let encoded = run_encode_git(&artifact, &out);
    assert!(
        encoded.status.success(),
        "encode git failed: {}",
        String::from_utf8_lossy(&encoded.stderr)
    );

    for rev in ["main", "feature", "v1"] {
        let before = commit_facts(source.path(), rev);
        let after = commit_facts(&out, rev);
        assert_eq!(before, after, "mismatch on {rev}");
    }

    // The tag's own message, minus the trailer block brygge added (RFC 019 D-2: the same block goes on
    // an annotated tag).
    let tag_message_before = git_out(
        source.path(),
        &["for-each-ref", "--format=%(contents)", "refs/tags/v1"],
    );
    let tag_message_after = git_out(
        &out,
        &["for-each-ref", "--format=%(contents)", "refs/tags/v1"],
    );
    assert_eq!(tag_message_before, strip_trailers(&tag_message_after));

    let ls_before = git_out(source.path(), &["ls-tree", "-r", "main"]);
    let ls_after = git_out(&out, &["ls-tree", "-r", "main"]);
    assert_eq!(ls_before, ls_after);
    assert!(ls_after.contains("100755") && ls_after.contains("run.sh"));
    assert!(ls_after.contains("120000") && ls_after.contains("link-to-readme"));
    assert!(ls_after.contains("new-name.txt") && !ls_after.contains("old-name.txt"));

    // No stray scratch ref was left behind, and HEAD points at the real default branch.
    let refs_after = git_out(&out, &["for-each-ref", "--format=%(refname)"]);
    assert!(
        !refs_after.contains("brygge-encode-git-scratch"),
        "{refs_after}"
    );
    let head = git_out(&out, &["symbolic-ref", "HEAD"]);
    assert_eq!(head, "refs/heads/main");

    // The report sidecar exists and names the real digest.
    let report_path = {
        let mut p = out.as_os_str().to_os_string();
        p.push(".brygge-report");
        PathBuf::from(p)
    };
    let report = std::fs::read_to_string(&report_path).unwrap();
    assert!(report.contains("encode_git.commits=4"), "{report}");
    assert!(report.contains("encode_git.tags=1"), "{report}");
}

#[test]
fn git_interpret_trailers_reads_back_the_block_brygge_wrote() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let source = build_round_trip_fixture();
    let ir =
        brygge_decode_git::decode(source.path(), &brygge_decode_git::Options::default()).unwrap();
    let artifact = write_artifact(&ir);
    let out = scratch_dir("result").join("repo");
    let encoded = run_encode_git(&artifact, &out);
    assert!(encoded.status.success());

    let message = git_out(&out, &["log", "-1", "--format=%B", "main"]);
    let child = Command::new("git")
        .current_dir(&out)
        .args(["interpret-trailers", "--parse"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn git interpret-trailers");
    child
        .stdin
        .as_ref()
        .unwrap()
        .write_all(message.as_bytes())
        .unwrap();
    let parsed = child.wait_with_output().unwrap();
    assert!(parsed.status.success());
    let parsed_text = String::from_utf8_lossy(&parsed.stdout);
    assert!(parsed_text.contains("Brygge-Encoded:"), "{parsed_text}");
    assert!(parsed_text.contains("Brygge-Source-Id:"), "{parsed_text}");
    assert!(parsed_text.contains("Brygge-Artifact:"), "{parsed_text}");
}

// ---- the other sources ---------------------------------------------------------------------------

struct HgRepo {
    dir: PathBuf,
}

impl Drop for HgRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl HgRepo {
    fn new() -> Self {
        let dir = scratch_dir("hg-repo");
        let r = Self { dir };
        r.run(&["init", r.dir.to_str().unwrap()]);
        r
    }

    fn run(&self, args: &[&str]) {
        let ok = Command::new("hg")
            .env("HGRCPATH", "/dev/null")
            .arg("--cwd")
            .arg(&self.dir)
            .arg("-R")
            .arg(&self.dir)
            .arg("--config")
            .arg("ui.username=A U Thor <a@example.com>")
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "hg {args:?} failed");
    }

    fn commit(&self, rel: &str, contents: &str, epoch: &str, msg: &str) {
        std::fs::write(self.dir.join(rel), contents).unwrap();
        self.run(&["add"]);
        self.run(&["commit", "-d", &format!("{epoch} 0"), "-m", msg]);
    }
}

#[test]
fn an_hg_fixture_encodes_into_a_repository_git_accepted() {
    if !hg_available() || !git_available() {
        eprintln!("skipping: hg or git not on PATH");
        return;
    }
    let repo = HgRepo::new();
    repo.commit("a.txt", "one\n", "1700000000", "c1");
    repo.commit("a.txt", "two\n", "1700000100", "c2");

    let ir = brygge_decode_hg::decode(&repo.dir, &brygge_decode_hg::Options::default()).unwrap();
    let artifact = write_artifact(&ir);
    let out = scratch_dir("result").join("repo");
    let encoded = run_encode_git(&artifact, &out);
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    assert_eq!(git_out(&out, &["rev-list", "--count", "HEAD"]), "2");
}

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

fn svn_dump_fixture() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(b"SVN-fs-dump-format-version: 2\n\n");
    b.extend(b"UUID: 11111111-2222-3333-4444-555555555555\n\n");
    svn_revision(&mut b, 0, &[("svn:date", "2024-02-01T00:00:00.000000Z")]);
    svn_revision(
        &mut b,
        1,
        &[
            ("svn:author", "carol"),
            ("svn:date", "2024-02-02T00:00:00.000000Z"),
            ("svn:log", "add a.txt"),
        ],
    );
    svn_add_file(&mut b, "a.txt", b"one\n");
    b
}

/// This fixture has no `trunk`/`branches`/`tags` layout at all (`a.txt` sits at the repository root) —
/// exactly the one documented SVN limit (the guide's "what this does not do yet"): even with
/// `--reconstruct-refs`, there is no convention to reconstruct a ref from, so the IR has zero refs and
/// `encode git` refuses it (review 136 F-2) rather than writing unreachable commits. It is not
/// encodable to Git in 0.8.0; this test demonstrates that limit for real, rather than skip it.
#[test]
fn an_svn_fixture_with_no_trunk_layout_is_refused_not_silently_lost() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let dump_dir = scratch_dir("svn-dump");
    let dump_path = dump_dir.join("repo.dump");
    std::fs::write(&dump_path, svn_dump_fixture()).unwrap();

    let ir = brygge_decode_svn::decode(
        &brygge_decode_svn::Source::DumpFile(dump_path),
        &brygge_decode_svn::Options {
            reconstruct_refs: true,
            ..brygge_decode_svn::Options::default()
        },
    )
    .unwrap();
    assert!(
        ir.refs.is_empty(),
        "this fixture has no trunk/branches/tags layout to reconstruct a ref from"
    );
    let artifact = write_artifact(&ir);
    let out = scratch_dir("result").join("repo");
    let result = run_encode_git(&artifact, &out);
    assert_eq!(result.status.code(), Some(2));
    assert!(
        !out.exists(),
        "nothing must be written at {}",
        out.display()
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("not reachable from any ref"), "{stderr}");
}

fn cvs_single_rev(author: &str, date: &str, log: &str, content: &str) -> Vec<u8> {
    format!(
        "head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t{date};\tauthor {author};\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@{log}@\ntext\n@{content}@\n"
    )
    .into_bytes()
}

/// As [`cvs_single_rev`], but with one RCS symbol (`TAG1:1.1`) naming the one revision — with
/// `--reconstruct-refs`, this is enough for the CVS decoder to reconstruct a real tag ref, unlike a
/// fixture with no symbols at all (which gives zero refs even with the flag — see
/// `an_artifact_with_zero_refs_is_refused_not_warned_about`).
fn cvs_single_rev_with_symbol(author: &str, date: &str, log: &str, content: &str) -> Vec<u8> {
    format!(
        "head\t1.1;\naccess;\nsymbols\n\tTAG1:1.1;\nlocks; strict;\n\n\n\
1.1\ndate\t{date};\tauthor {author};\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@{log}@\ntext\n@{content}@\n"
    )
    .into_bytes()
}

#[test]
fn a_cvs_fixture_encodes_with_derived_trailers() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo_dir = scratch_dir("cvs-repo");
    let vfile = repo_dir.join("a.c,v");
    // One RCS symbol (TAG1:1.1) gives --reconstruct-refs a real tag to build, so the resulting repository
    // has a reachable commit (review 136 F-2: an artifact with zero refs is refused, not written —
    // see `an_artifact_with_zero_refs_is_refused_not_warned_about` for that case).
    std::fs::write(
        &vfile,
        cvs_single_rev_with_symbol("alice", "2024.01.01.12.00.00", "add a.c", "aaa\n"),
    )
    .unwrap();

    let ir = brygge_decode_cvs::decode(
        &brygge_decode_cvs::Source::LocalRepo(repo_dir),
        &brygge_decode_cvs::Options {
            reconstruct_refs: true,
            ..brygge_decode_cvs::Options::default()
        },
    )
    .unwrap();
    assert!(
        !ir.refs.is_empty(),
        "the TAG1 symbol must reconstruct a ref"
    );
    let artifact = write_artifact(&ir);
    let out = scratch_dir("result").join("repo");
    let encoded = run_encode_git(&artifact, &out);
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );

    let message = git_out(&out, &["log", "-1", "--format=%B", "refs/tags/TAG1"]);
    assert!(
        message.contains("Brygge-Derived: atom reconstructed-changeset"),
        "{message}"
    );
    assert!(message.contains("Brygge-Supplied:"), "{message}");
    assert!(message.contains("committer"), "{message}");
}

// ---- reachability (review 136 F-2) -------------------------------------------------------------------

#[test]
fn an_artifact_with_zero_refs_is_refused_not_warned_about() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo_dir = scratch_dir("cvs-repo-norefs");
    let vfile = repo_dir.join("a.c,v");
    // No symbols at all: even --reconstruct-refs reconstructs no branch or tag, so the IR has atoms but
    // zero refs — every commit would be unreachable once written. This must now refuse, not warn.
    std::fs::write(
        &vfile,
        cvs_single_rev("alice", "2024.01.01.12.00.00", "add a.c", "aaa\n"),
    )
    .unwrap();

    let ir = brygge_decode_cvs::decode(
        &brygge_decode_cvs::Source::LocalRepo(repo_dir),
        &brygge_decode_cvs::Options {
            reconstruct_refs: true,
            ..brygge_decode_cvs::Options::default()
        },
    )
    .unwrap();
    assert!(
        ir.refs.is_empty(),
        "this fixture has no symbols to reconstruct a ref from"
    );
    let artifact = write_artifact(&ir);
    let out = scratch_dir("result").join("repo");
    let result = run_encode_git(&artifact, &out);
    assert_eq!(result.status.code(), Some(2));
    assert!(
        !out.exists(),
        "nothing must be written at {}",
        out.display()
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("not reachable from any ref"), "{stderr}");
    assert!(stderr.contains("--reconstruct-refs"), "{stderr}");
}

// ---- refusals ---------------------------------------------------------------------------------------

#[test]
fn a_corrupted_artifact_is_refused_before_any_output() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let source = build_round_trip_fixture();
    let ir =
        brygge_decode_git::decode(source.path(), &brygge_decode_git::Options::default()).unwrap();
    let artifact = write_artifact(&ir);
    let mut bytes = std::fs::read(&artifact).unwrap();
    let i = bytes.len() - 1;
    bytes[i] ^= 0xff;
    std::fs::write(&artifact, &bytes).unwrap();

    let out = scratch_dir("corrupt-out").join("repo");
    let result = run_encode_git(&artifact, &out);
    assert!(!result.status.success());
    assert!(
        !out.exists(),
        "nothing must be written at {}",
        out.display()
    );
}

#[test]
fn an_existing_out_directory_is_refused() {
    let out = scratch_dir("exists");
    let artifact = scratch_dir("dummy").join("a.ir");
    std::fs::write(&artifact, b"not a real artifact").unwrap();
    let result = run_encode_git(&artifact, &out);
    assert_eq!(result.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("already exists"),
        "{:?}",
        result
    );
}

/// Review 136 F-4: `<dir>.brygge-report` is refused up front, exactly as `<dir>` is, rather than
/// silently overwritten.
#[test]
fn an_existing_report_path_is_refused() {
    let dir = scratch_dir("report-exists");
    let out = dir.join("repo");
    let report_path = dir.join("repo.brygge-report");
    std::fs::write(&report_path, b"pre-existing report").unwrap();
    let artifact = scratch_dir("dummy2").join("a.ir");
    std::fs::write(&artifact, b"not a real artifact").unwrap();

    let result = run_encode_git(&artifact, &out);
    assert_eq!(result.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("already exists"), "{stderr}");
    assert!(
        !out.exists(),
        "nothing must be written at {}",
        out.display()
    );
    assert_eq!(
        std::fs::read(&report_path).unwrap(),
        b"pre-existing report",
        "the pre-existing report must be left untouched"
    );
}

// ---- determinism (D-5) -------------------------------------------------------------------------------

#[test]
fn encoding_the_same_fixture_twice_gives_byte_identical_refs() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let source = build_round_trip_fixture();
    let ir =
        brygge_decode_git::decode(source.path(), &brygge_decode_git::Options::default()).unwrap();
    let artifact = write_artifact(&ir);

    let out1 = scratch_dir("det1").join("repo");
    let out2 = scratch_dir("det2").join("repo");
    assert!(run_encode_git(&artifact, &out1).status.success());
    assert!(run_encode_git(&artifact, &out2).status.success());

    let refs1 = git_out(
        &out1,
        &["for-each-ref", "--format=%(objectname) %(refname)"],
    );
    let refs2 = git_out(
        &out2,
        &["for-each-ref", "--format=%(objectname) %(refname)"],
    );
    assert_eq!(refs1, refs2);
}

#[test]
fn encoding_under_a_different_home_tz_and_locale_gives_the_same_ids() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let source = build_round_trip_fixture();
    let ir =
        brygge_decode_git::decode(source.path(), &brygge_decode_git::Options::default()).unwrap();
    let artifact = write_artifact(&ir);

    let baseline = scratch_dir("env-baseline").join("repo");
    assert!(run_encode_git(&artifact, &baseline).status.success());
    let baseline_refs = git_out(
        &baseline,
        &["for-each-ref", "--format=%(objectname) %(refname)"],
    );

    let hostile_home = scratch_dir("hostile-home");
    std::fs::write(
        hostile_home.join(".gitconfig"),
        "[user]\n\tname = Hostile\n\temail = hostile@example.com\n[core]\n\tautocrlf = true\n",
    )
    .unwrap();
    let hostile_template = scratch_dir("hostile-template");
    std::fs::write(hostile_template.join("description"), "hostile template\n").unwrap();

    let out = scratch_dir("env-varied").join("repo");
    let result = Command::new(bin())
        .args(["encode", "git"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .env("HOME", &hostile_home)
        .env("GIT_TEMPLATE_DIR", &hostile_template)
        .env("TZ", "Pacific/Kiritimati")
        .env("LC_ALL", "C")
        .output()
        .expect("run brygge encode git");
    assert!(result.status.success(), "{:?}", result);
    let varied_refs = git_out(&out, &["for-each-ref", "--format=%(objectname) %(refname)"]);
    assert_eq!(
        baseline_refs, varied_refs,
        "a hostile HOME/GIT_TEMPLATE_DIR and a different TZ in the parent environment must not change \
         the result"
    );
    // The hostile identity must not have leaked into the commit either.
    let author = git_out(&out, &["log", "-1", "--format=%an <%ae>", "main"]);
    assert_ne!(author, "Hostile <hostile@example.com>");
}

// ---- review 137 F-7: a many-pair CVS id round-trips through git interpret-trailers --------------------

#[test]
fn a_forty_pair_cvs_changeset_id_round_trips_through_git_interpret_trailers() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo_dir = scratch_dir("cvs-repo-40");
    // 40 files, all committed with the same author/date/log, so CVS's changeset reconstruction clusters
    // them into one changeset — a 40-pair path@rev source id, each pair on its own line in the real
    // RCS-derived bytes.
    for i in 0..40 {
        let vfile = repo_dir.join(format!("f{i}.c,v"));
        std::fs::write(
            &vfile,
            cvs_single_rev("alice", "2024.01.01.12.00.00", "a 40-file commit", "x\n"),
        )
        .unwrap();
    }
    // One symbol, on f0, so --reconstruct-refs gives the result a real, reachable ref.
    let symbol_file = repo_dir.join("f0.c,v");
    std::fs::write(
        &symbol_file,
        cvs_single_rev_with_symbol("alice", "2024.01.01.12.00.00", "a 40-file commit", "x\n"),
    )
    .unwrap();

    let ir = brygge_decode_cvs::decode(
        &brygge_decode_cvs::Source::LocalRepo(repo_dir),
        &brygge_decode_cvs::Options {
            reconstruct_refs: true,
            ..brygge_decode_cvs::Options::default()
        },
    )
    .unwrap();
    assert_eq!(
        ir.atoms.len(),
        1,
        "all 40 files must cluster into one changeset"
    );
    assert_eq!(
        ir.atoms[0]
            .source
            .atom_id
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1,
        40,
        "the changeset's own source id must hold all 40 path@rev pairs"
    );

    let artifact = write_artifact(&ir);
    let out = scratch_dir("result").join("repo");
    let encoded = run_encode_git(&artifact, &out);
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );

    let message = git_out(&out, &["log", "-1", "--format=%B", "refs/tags/TAG1"]);
    let child = Command::new("git")
        .current_dir(&out)
        .args(["interpret-trailers", "--parse"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn git interpret-trailers");
    child
        .stdin
        .as_ref()
        .unwrap()
        .write_all(message.as_bytes())
        .unwrap();
    let parsed = child.wait_with_output().unwrap();
    assert!(parsed.status.success());
    let parsed_text = String::from_utf8_lossy(&parsed.stdout);
    let source_id_line = parsed_text
        .lines()
        .find(|l| l.starts_with("Brygge-Source-Id:"))
        .unwrap_or_else(|| panic!("no Brygge-Source-Id trailer: {parsed_text}"));
    // git interpret-trailers --parse folds a multi-line trailer value onto one line, space-separated —
    // confirming it really did read every continuation line as part of the one trailer, not 39 separate,
    // broken ones.
    let pair_count = source_id_line.matches(".c@1.1").count();
    assert_eq!(pair_count, 40, "{source_id_line}");
}

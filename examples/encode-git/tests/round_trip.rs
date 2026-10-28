//! The real round trip (RFC 017 D-1/D-2 proof items 1–4): decode a real repository, write its artifact,
//! run the compiled `brygge-encode-git-example` binary against it, and `git fast-import` the result —
//! comparing with the original where the IR recorded no loss.
//!
//! Each source's test skips (with a note) when its real tool is not on `PATH`, matching this project's
//! existing real-tool-test convention.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "brygge-encode-git-example-test-{label}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- Git fixture ------------------------------------------------------------------------------------

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

    /// Commits whatever is already staged, with no `git add` of its own — needed after
    /// `make_executable`/`add_symlink`, which stage an index entry directly; a later `add -A` would
    /// re-stage the file from its real on-disk mode and silently undo the `--chmod`.
    fn commit_only(&self, msg: &str) {
        self.git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", msg]);
    }

    /// Adds `rel` as a symlink pointing at `target`, through the index directly (`cacheinfo`), which
    /// needs no OS symlink privilege (portable to Windows CI, matching this crate's own cross-platform
    /// aim even though the test itself skips without `git`).
    fn add_symlink(&self, rel: &str, target: &str) {
        let sha = self.git_with_stdin(&["hash-object", "-w", "--stdin"], target);
        self.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{sha},{rel}"),
        ]);
    }

    /// Marks `rel` executable in the index (`--chmod`), likewise with no OS privilege needed.
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

/// Merges, an executable file, a symlink, a rename, and an annotated tag (RFC 017 D-1/D-2 proof item 1).
fn build_round_trip_fixture() -> TempRepo {
    let r = TempRepo::new();
    r.write("readme.txt", "hello\n");
    r.write("old-name.txt", "will be renamed\n");
    r.write("run.sh", "#!/bin/sh\necho hi\n");
    r.git(&["add", "-A"]);
    r.make_executable("run.sh");
    r.add_symlink("link-to-readme", "readme.txt");
    r.commit_only("initial");

    // From here on, never `git add -A`: the working tree has no real chmod +x on `run.sh` and no real
    // symlink at `link-to-readme` (both are index-only tricks, portable to Windows CI, above) — a
    // blanket `-A` would re-stage them from the working tree and silently revert both.
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

fn run_encoder(artifact: &Path, out: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brygge-encode-git-example"))
        .arg("--artifact")
        .arg(artifact)
        .arg("--out")
        .arg(out)
        .output()
        .expect("run brygge-encode-git-example")
}

fn fast_import(stream_path: &Path) -> TempRepo {
    let target = TempRepo::new();
    let bytes = std::fs::read(stream_path).unwrap();
    let mut child = Command::new("git")
        .current_dir(&target.dir)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn git fast-import");
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git fast-import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    target
}

/// One commit's comparable facts, from `git log`, with the `\x1f`/`\x1e` separator trick (field/record
/// separators unlikely to appear in test content).
#[derive(Debug, PartialEq, Eq)]
struct CommitFacts {
    tree: String,
    author_name: String,
    author_email: String,
    author_date: String,
    message: String,
}

/// Strips brygge-encode-git-example's own trailer paragraph (everything from the blank line before
/// `Encoded-by:` onward) so the remaining message can be compared with the source's original.
fn strip_trailers(full: &str) -> &str {
    full.split("\n\nEncoded-by: brygge-encode-git-example")
        .next()
        .unwrap()
        .trim_end_matches('\n')
}

fn commit_facts(repo: &TempRepo, rev: &str) -> Vec<CommitFacts> {
    let out = repo.git(&[
        "log",
        "--topo-order",
        "--reverse",
        "--format=%H%x1f%T%x1f%an%x1f%ae%x1f%ad%x1f%B%x1e",
        rev,
    ]);
    out.split('\x1e')
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
    let stream_dir = scratch_dir("stream");
    let stream = stream_dir.join("stream.txt");

    let encoded = run_encoder(&artifact, &stream);
    assert!(
        encoded.status.success(),
        "encoder failed: {}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    // The stderr summary (loss boundary/flags) is always printed; this fixture has no derivations and
    // no drops worth asserting on individually, but the call must not panic or print nothing at all.
    assert!(!encoded.stderr.is_empty());

    let target = fast_import(&stream);

    for rev in ["main", "feature", "v1"] {
        let before = commit_facts(&source, rev);
        let after = commit_facts(&target, rev);
        assert_eq!(before, after, "mismatch on {rev}");
    }

    // The annotated tag's own identity/message (not just the commit it points at).
    let tag_before = source.git(["tag", "-l", "-n99", "v1"].as_slice());
    let tag_after = target.git(["tag", "-l", "-n99", "v1"].as_slice());
    assert_eq!(tag_before, tag_after);
    let tagger_before = source.git(&[
        "for-each-ref",
        "--format=%(taggername) %(taggeremail)",
        "refs/tags/v1",
    ]);
    let tagger_after = target.git(&[
        "for-each-ref",
        "--format=%(taggername) %(taggeremail)",
        "refs/tags/v1",
    ]);
    assert_eq!(tagger_before, tagger_after);

    // The rename, the executable bit, and the symlink survive onto `main`'s final tree.
    let ls_before = source.git(&["ls-tree", "-r", "main"]);
    let ls_after = target.git(&["ls-tree", "-r", "main"]);
    assert_eq!(ls_before, ls_after);
    assert!(ls_after.contains("100755") && ls_after.contains("run.sh"));
    assert!(ls_after.contains("120000") && ls_after.contains("link-to-readme"));
    assert!(ls_after.contains("new-name.txt") && !ls_after.contains("old-name.txt"));

    // No stray scratch ref was left behind.
    let refs_after = target.git(&["for-each-ref", "--format=%(refname)"]);
    assert!(
        !refs_after.contains("brygge-encode-git-example-scratch"),
        "{refs_after}"
    );
}

// ---- The other sources (proof item 2) ---------------------------------------------------------------

fn hg_available() -> bool {
    Command::new("hg")
        .arg("--version")
        .env("HGRCPATH", "/dev/null")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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
fn an_hg_fixture_encodes_and_fast_imports_without_error() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = HgRepo::new();
    repo.commit("a.txt", "one\n", "1700000000", "c1");
    repo.commit("a.txt", "two\n", "1700000100", "c2");

    let ir = brygge_decode_hg::decode(&repo.dir, &brygge_decode_hg::Options::default()).unwrap();
    let artifact = write_artifact(&ir);
    let stream_dir = scratch_dir("hg-stream");
    let stream = stream_dir.join("stream.txt");
    let encoded = run_encoder(&artifact, &stream);
    assert!(
        encoded.status.success(),
        "encoder failed: {}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    fast_import(&stream);
}

/// A property block (`K <len>\n<key>\nV <len>\n<value>\n...PROPS-END\n`), as SVN dumps use.
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

#[test]
fn an_svn_fixture_encodes_and_fast_imports_without_error() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let dump_dir = scratch_dir("svn-dump");
    let dump_path = dump_dir.join("repo.dump");
    std::fs::write(&dump_path, svn_dump_fixture()).unwrap();

    let ir = brygge_decode_svn::decode(
        &brygge_decode_svn::Source::DumpFile(dump_path),
        &brygge_decode_svn::Options::default(),
    )
    .unwrap();
    let artifact = write_artifact(&ir);
    let stream_dir = scratch_dir("svn-stream");
    let stream = stream_dir.join("stream.txt");
    let encoded = run_encoder(&artifact, &stream);
    assert!(
        encoded.status.success(),
        "encoder failed: {}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    fast_import(&stream);
}

/// A single-revision (`1.1`) RCS `,v` with full text, matching `crates/brygge/src/commands/tests.rs`'s
/// own CVS fixture shape.
fn cvs_single_rev(author: &str, date: &str, log: &str, content: &str) -> Vec<u8> {
    format!(
        "head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t{date};\tauthor {author};\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@{log}@\ntext\n@{content}@\n"
    )
    .into_bytes()
}

#[test]
fn a_cvs_fixture_encodes_and_fast_imports_with_derived_trailers() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo_dir = scratch_dir("cvs-repo");
    let vfile = repo_dir.join("a.c,v");
    std::fs::write(
        &vfile,
        cvs_single_rev("alice", "2024.01.01.12.00.00", "add a.c", "aaa\n"),
    )
    .unwrap();

    let ir = brygge_decode_cvs::decode(
        &brygge_decode_cvs::Source::LocalRepo(repo_dir),
        &brygge_decode_cvs::Options::default(),
    )
    .unwrap();
    let artifact = write_artifact(&ir);
    let stream_dir = scratch_dir("cvs-stream");
    let stream = stream_dir.join("stream.txt");
    let encoded = run_encoder(&artifact, &stream);
    assert!(
        encoded.status.success(),
        "encoder failed: {}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    let bytes = std::fs::read(&stream).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    // CVS changesets are reconstructed (`Derived`), so the commit must carry a `Brygge-Derived:` trailer
    // naming it, and (CVS has no separate committer/commit-time) `Encoded-supplied` for both.
    assert!(
        text.contains("Brygge-Derived: atom reconstructed-changeset"),
        "{text}"
    );
    assert!(text.contains("Encoded-supplied:"), "{text}");
    assert!(text.contains("committer"), "{text}");

    fast_import(&stream);
}

// ---- review 091 F-1/F-3: `Encoded-altered`, and a declared message encoding (hand-built IRs, no
// decoder needed — these exercise `encode()` directly against values no real source fixture
// conveniently produces) -------------------------------------------------------------------------------

/// A one-atom, one-ref `Ir`, built directly with `IrBuilder` (not through a decoder), so the author
/// identity and message can be set to exactly what a test needs.
fn minimal_ir(author: brygge_ir::Identity, message: brygge_ir::Text) -> brygge_ir::Ir {
    use brygge_ir::{
        AtomDraft, EpistemicStatus, ImportProvenance, IrBuilder, MetadataClaims, PathOp, RefKind,
        RefRecord, SourceIdentity, SourceKind, Time,
    };

    let source = |atom_id: &[u8]| SourceIdentity {
        kind: SourceKind::Git,
        repo_id: Vec::new(),
        atom_id: atom_id.to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
    };
    let provenance = ImportProvenance {
        source: source(b"repo"),
        brygge_version: "0.0.0".to_string(),
        decoder: "test".to_string(),
        decoder_version: "0.0.0".to_string(),
        params: std::collections::BTreeMap::new(),
    };
    let mut builder = IrBuilder::new(provenance);
    let blob = builder.add_blob(b"hello\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "readme.txt".to_string(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(author),
                author_time: Some(Time {
                    seconds: 1_700_000_000,
                    offset_minutes: Some(0),
                }),
                committer: None,
                commit_time: None,
                message: Some(message),
            },
            source: source(b"r1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    builder.finish().unwrap()
}

#[test]
fn an_altered_identity_gets_its_own_trailer_and_fast_import_accepts_it() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let author = brygge_ir::Identity {
        name: brygge_ir::Text {
            bytes: vec![0xff, 0xfe], // not UTF-8
            encoding: None,
        },
        email: Some(brygge_ir::Text::utf8("a<b>@example.com")), // angle brackets
    };
    let ir = minimal_ir(author, brygge_ir::Text::utf8("a message"));
    let artifact = write_artifact(&ir);
    let stream = scratch_dir("altered-stream").join("stream.txt");
    let encoded = run_encoder(&artifact, &stream);
    assert!(
        encoded.status.success(),
        "encoder failed: {}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    let text = std::fs::read_to_string(&stream).unwrap();
    assert!(
        text.contains("Encoded-altered: author-name (not UTF-8, replaced), author-email (angle bracket removed)"),
        "{text}"
    );
    assert!(text.contains("author unknown <ab@example.com>"), "{text}");

    let target = fast_import(&stream);
    let author_line = target.git(&["log", "-1", "--format=%an <%ae>", "main"]);
    assert_eq!(author_line, "unknown <ab@example.com>");
}

#[test]
fn a_declared_message_encoding_is_written_and_preserved_byte_for_byte() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    // "café" in ISO-8859-1/latin-1: 0xe9 is not valid UTF-8 on its own.
    let message_bytes = b"caf\xe9 test\n".to_vec();
    let message = brygge_ir::Text {
        bytes: message_bytes.clone(),
        encoding: Some("ISO-8859-1".to_string()),
    };
    let author = brygge_ir::Identity {
        name: brygge_ir::Text::utf8("A U Thor"),
        email: Some(brygge_ir::Text::utf8("a@example.com")),
    };
    let ir = minimal_ir(author, message);
    let artifact = write_artifact(&ir);
    let stream = scratch_dir("encoding-stream").join("stream.txt");
    let encoded = run_encoder(&artifact, &stream);
    assert!(
        encoded.status.success(),
        "encoder failed: {}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    let stream_bytes = std::fs::read(&stream).unwrap();
    let text = String::from_utf8_lossy(&stream_bytes);
    assert!(text.contains("encoding ISO-8859-1\n"), "{text}");

    let target = fast_import(&stream);
    let raw = target.git(&["cat-file", "commit", "main"]);
    // `git cat-file commit` already prints the object's raw bytes (lossily re-decoded as UTF-8 by
    // `TempRepo::git`'s own `from_utf8_lossy`, but the header line proves the bytes reached the object
    // store unconverted): `encoding ISO-8859-1` is a genuine header line Git itself wrote back out.
    assert!(raw.contains("encoding ISO-8859-1"), "{raw}");
    let raw_bytes = Command::new("git")
        .current_dir(target.path())
        .args(["cat-file", "commit", "main"])
        .output()
        .unwrap()
        .stdout;
    assert!(
        raw_bytes
            .windows(message_bytes.len())
            .any(|w| w == message_bytes.as_slice()),
        "the message's original (non-UTF-8) bytes must reach the object unchanged: {raw_bytes:?}"
    );
}

// ---- Refusals (proof item 3) -----------------------------------------------------------------------

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
    // Flip one byte well past the header, inside the stored body (not the magic/format/version bytes),
    // so it is caught by the digest, not an earlier structural gate.
    let i = bytes.len() - 1;
    bytes[i] ^= 0xff;
    std::fs::write(&artifact, &bytes).unwrap();

    let out = run_encoder(&artifact, &scratch_dir("corrupt-out").join("stream.txt"));
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "no output must reach stdout: {:?}",
        out.stdout
    );
    assert!(!out.stderr.is_empty());
}

#[test]
fn a_repeated_artifact_flag_is_a_usage_error() {
    let dir = scratch_dir("usage");
    let out = Command::new(env!("CARGO_BIN_EXE_brygge-encode-git-example"))
        .arg("--artifact")
        .arg(dir.join("a"))
        .arg("--artifact")
        .arg(dir.join("b"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    let dir = scratch_dir("usage2");
    let out = Command::new(env!("CARGO_BIN_EXE_brygge-encode-git-example"))
        .arg("--artifact")
        .arg(dir.join("a"))
        .arg("--nonsense")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn help_carries_the_unsupported_example_label_and_exits_cleanly() {
    let out = Command::new(env!("CARGO_BIN_EXE_brygge-encode-git-example"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("an unsupported example; not part of brygge"),
        "{text}"
    );
}

// ---- Isolation (proof item 4) ----------------------------------------------------------------------

/// `cargo tree -p brygge-encode-git-example`'s normal-dependency closure is exactly `brygge-ir`'s own
/// allowed closure (`crates/brygge-ir/allowed-dependencies.txt`, the single source of truth
/// `tools/check-ir-isolation.sh` already checks `brygge-ir` itself against) plus this crate's own
/// package name — no decoder, no Git library, nothing else (RFC 017 D-1/D-2, PU-3).
#[test]
fn the_crate_depends_on_brygge_ir_alone() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "brygge-encode-git-example",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(&workspace_root)
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut actual: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.split_whitespace().next().unwrap_or(l).to_string())
        .collect();
    actual.sort();
    actual.dedup();

    let allowlist_text =
        std::fs::read_to_string(workspace_root.join("crates/brygge-ir/allowed-dependencies.txt"))
            .unwrap();
    let mut expected: Vec<String> = allowlist_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .chain(std::iter::once("brygge-encode-git-example".to_string()))
        .collect();
    expected.sort();
    expected.dedup();

    assert_eq!(actual, expected);
}

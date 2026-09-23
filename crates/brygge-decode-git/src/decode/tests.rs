//! Tests for the Git decoder (RFC 004). Fixtures are built with the `git` CLI in throwaway temp repos
//! with pinned identity and dates; if `git` is not on `PATH` the test skips (with a note) rather than
//! failing a git-less environment.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::Options;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A temp repository directory, cleaned on drop.
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
            "brygge-git-test-{}-{nanos}-{n}",
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

    fn commit_all(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", msg]);
    }
}

/// A repo with a linear history, a branch, a merge, a modify, a delete, and a tag.
fn build_rich_repo() -> TempRepo {
    let r = TempRepo::new();
    r.write("readme.txt", "hello\n");
    r.write("keep.txt", "keep me\n");
    r.commit_all("initial");

    r.write("readme.txt", "hello world\n"); // modify
    r.commit_all("expand readme");

    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("feature.txt", "a feature\n");
    r.commit_all("add feature");

    r.git(&["checkout", "-q", "main"]);
    std::fs::remove_file(r.path().join("keep.txt")).unwrap(); // delete
    r.commit_all("drop keep");

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

#[test]
fn decodes_a_rich_history_all_stated() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = build_rich_repo();
    let ir = decode(r.path(), &Options::default()).unwrap();

    // 5 commits: initial, expand, add-feature, drop-keep, merge.
    assert_eq!(ir.atoms.len(), 5, "expected five commits");
    // A branch (main; feature is merged so its head ref may or may not remain) and a tag.
    assert!(ir.refs.iter().any(|rf| rf.name == "v1"));
    assert!(ir.refs.iter().any(|rf| rf.name == "main"));

    // Everything is source-stated: no derived marks with renames off.
    let report = brygge_ir::honesty::summary(&ir);
    assert!(
        report.derived.is_empty(),
        "a plain import must be all Stated"
    );
    assert!(report.blobs >= 3);

    // The merge atom has two parents; a root atom has none.
    assert!(
        ir.atoms.iter().any(|a| a.parents.len() == 2),
        "merge has two parents"
    );
    assert!(
        ir.atoms.iter().any(|a| a.parents.is_empty()),
        "root has no parents"
    );

    // Metadata claims are carried.
    let any = &ir.atoms[0];
    assert_eq!(
        any.metadata.author.as_ref().unwrap().email,
        "author@example.com"
    );
    assert!(any.metadata.commit_time.is_some());

    // A delete and a modify appear literally among the ops.
    let has_delete = ir
        .atoms
        .iter()
        .flat_map(|a| &a.ops)
        .any(|op| matches!(op, brygge_ir::PathOp::Delete { path, .. } if path == "keep.txt"));
    let has_modify = ir
        .atoms
        .iter()
        .flat_map(|a| &a.ops)
        .any(|op| matches!(op, brygge_ir::PathOp::Modify { path, .. } if path == "readme.txt"));
    assert!(has_delete, "the delete of keep.txt is a literal Delete op");
    assert!(has_modify, "the readme modification is a literal Modify op");
}

#[test]
fn decode_is_byte_deterministic_and_pack_independent() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = build_rich_repo();
    let a = brygge_ir::to_bytes(&decode(r.path(), &Options::default()).unwrap());
    let b = brygge_ir::to_bytes(&decode(r.path(), &Options::default()).unwrap());
    assert_eq!(
        a, b,
        "two decodes of the same repo are byte-identical (VF-1)"
    );

    // Repacking changes the physical layout but not the logical objects.
    r.git(&["repack", "-a", "-d", "-q"]);
    r.git(&["gc", "-q", "--aggressive"]);
    let c = brygge_ir::to_bytes(&decode(r.path(), &Options::default()).unwrap());
    assert_eq!(
        a, c,
        "physical packing must not change the output (RFC 004 D-6)"
    );
}

#[test]
fn renames_off_by_default_on_marks_derived() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("old/name.txt", "identical content\n");
    r.commit_all("add file");
    // A pure move: delete old path, add new path with the *same* bytes, in one commit.
    std::fs::remove_file(r.path().join("old/name.txt")).unwrap();
    r.write("new/name.txt", "identical content\n");
    r.commit_all("move file");

    // Off by default: the literal delete+add, no rename hint.
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(ir.atoms.iter().all(|a| a.rename_hints.is_empty()));
    assert!(brygge_ir::honesty::summary(&ir).derived.is_empty());

    // On: a Derived InferredRename hint appears beside the still-present literal ops.
    let opts = Options {
        detect_renames: true,
        rename_threshold: 100,
    };
    let ir = decode(r.path(), &opts).unwrap();
    let move_atom = ir
        .atoms
        .iter()
        .find(|a| !a.rename_hints.is_empty())
        .expect("the move commit carries a rename hint");
    let hint = &move_atom.rename_hints[0];
    assert_eq!(hint.from, "old/name.txt");
    assert_eq!(hint.to, "new/name.txt");
    assert!(hint.status.is_derived(), "an inferred rename is Derived");
    // The literal delete+add are still there, never collapsed.
    assert!(
        move_atom.ops.iter().any(
            |op| matches!(op, brygge_ir::PathOp::Delete { path, .. } if path == "old/name.txt")
        )
    );
    assert!(
        move_atom
            .ops
            .iter()
            .any(|op| matches!(op, brygge_ir::PathOp::Add { path, .. } if path == "new/name.txt"))
    );
    assert_eq!(
        brygge_ir::honesty::summary(&ir)
            .derived
            .get("inferred-rename"),
        Some(&1)
    );
}

#[test]
fn loss_boundary_records_representation_drops() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = build_rich_repo();
    let ir = decode(r.path(), &Options::default()).unwrap();
    let whats: Vec<&str> = ir.loss.dropped.iter().map(|d| d.what.as_str()).collect();
    assert!(whats.iter().any(|w| w.contains("packfile")));
    assert!(whats.iter().any(|w| w.contains("reflogs")));
    // The representation drops are all Representation-class; the one non-representation drop is the
    // annotated tag v1's tagger/message (OQ-B), which the RefRecord cannot hold.
    assert!(
        ir.loss
            .dropped
            .iter()
            .filter(|d| matches!(d.class, brygge_ir::LossClass::Representation))
            .count()
            >= 3
    );
    let other: Vec<&brygge_ir::DropRecord> = ir
        .loss
        .dropped
        .iter()
        .filter(|d| !matches!(d.class, brygge_ir::LossClass::Representation))
        .collect();
    assert_eq!(
        other.len(),
        1,
        "only the annotated-tag metadata is a non-representation drop"
    );
    assert!(other[0].what.contains("annotated tag"));
    assert!(matches!(other[0].class, brygge_ir::LossClass::Other));
}

#[test]
fn refuses_a_submodule_gitlink() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("readme.txt", "hi\n");
    r.commit_all("initial");
    let head = r.git(&["rev-parse", "HEAD"]);
    // Add a gitlink (mode 160000) pointing at any valid commit id.
    r.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},sub"),
    ]);
    r.git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "add gitlink",
    ]);

    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => assert_eq!(feature, "submodule"),
        other => panic!("expected a submodule floor refusal, got {other:?}"),
    }
}

#[test]
fn refuses_shallow_and_grafts_and_replace() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    // Shallow.
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    std::fs::write(
        r.path().join(".git/shallow"),
        r.git(&["rev-parse", "HEAD"]) + "\n",
    )
    .unwrap();
    assert!(matches!(
        decode(r.path(), &Options::default()),
        Err(crate::Error::FloorRefusal { ref feature, .. }) if feature == "shallow clone"
    ));

    // Grafts.
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    std::fs::create_dir_all(r.path().join(".git/info")).unwrap();
    std::fs::write(
        r.path().join(".git/info/grafts"),
        r.git(&["rev-parse", "HEAD"]) + "\n",
    )
    .unwrap();
    assert!(matches!(
        decode(r.path(), &Options::default()),
        Err(crate::Error::FloorRefusal { ref feature, .. }) if feature == "grafts"
    ));

    // Replace ref.
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let c1 = r.git(&["rev-parse", "HEAD"]);
    r.write("a.txt", "b\n");
    r.commit_all("c2");
    let c2 = r.git(&["rev-parse", "HEAD"]);
    r.git(&["replace", &c2, &c1]);
    assert!(matches!(
        decode(r.path(), &Options::default()),
        Err(crate::Error::FloorRefusal { ref feature, .. }) if feature == "replace ref"
    ));
}

// --- CR-16: symbolic refs must not panic, and are dropped-with-record ---------------------------

#[test]
fn symbolic_ref_in_a_dropped_namespace_decodes_without_panic() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    // The exact shape an ordinary `git clone` creates: `refs/remotes/origin/HEAD` -> `.../main`.
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let head = r.git(&["rev-parse", "HEAD"]);
    r.git(&["update-ref", "refs/remotes/origin/main", &head]);
    r.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let whats: Vec<&str> = ir.loss.dropped.iter().map(|d| d.what.as_str()).collect();
    assert!(
        whats.contains(&"symbolic refs (1)"),
        "the symbolic ref is recorded as its own drop: {whats:?}"
    );
    // Its target is a distinct, non-symbolic ref in the same (already dropped) namespace, carried on
    // its own by the existing remote-tracking record — not folded into the symbolic-ref count.
    assert!(
        whats.contains(&"remote-tracking refs"),
        "the target ref is still handled on its own: {whats:?}"
    );
}

#[test]
fn symbolic_ref_in_a_carried_namespace_is_dropped_not_its_target() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    r.git(&["symbolic-ref", "refs/heads/alias", "refs/heads/main"]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(
        ir.refs.iter().any(|rf| rf.name == "main"),
        "the real branch main is still carried"
    );
    assert!(
        ir.refs.iter().all(|rf| rf.name != "alias"),
        "a symbolic ref is never carried, even in a normally-carried namespace (RFC 004 OQ-B)"
    );
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "symbolic refs (1)")
    );
}

#[test]
fn dangling_symbolic_ref_decodes_and_is_dropped() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    // Points at a branch that does not exist. `try_id()` reflects the ref's own (symbolic) shape, not
    // whether its target resolves, so this is dropped exactly like a resolving symbolic ref.
    r.git(&[
        "symbolic-ref",
        "refs/heads/nowhere",
        "refs/heads/does-not-exist",
    ]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(ir.refs.iter().all(|rf| rf.name != "nowhere"));
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "symbolic refs (1)"),
        "a dangling symbolic ref is recorded exactly like a resolving one"
    );
}

// --- CR-16 R-1: an unparseable commit time is absent, never fabricated or salvaged --------------

/// Build a commit in `r` whose author and committer both carry the raw `time_field` bytes, via
/// `git hash-object --literally` (which bypasses git's own well-formedness checks), and point
/// `refs/heads/main` at it. Returns the commit sha.
fn commit_with_raw_time(r: &TempRepo, time_field: &str) -> String {
    use std::io::Write as _;

    r.write("a.txt", "a\n");
    r.git(&["add", "-A"]);
    let tree = r.git(&["write-tree"]);
    let body = format!(
        "tree {tree}\n\
         author A U Thor <author@example.com> {time_field}\n\
         committer A U Thor <author@example.com> {time_field}\n\
         \n\
         malformed time\n"
    );
    let mut child = Command::new("git")
        .current_dir(r.path())
        .args([
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--literally",
            "--stdin",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn git hash-object");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git hash-object: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    r.git(&["update-ref", "refs/heads/main", &sha]);
    sha
}

#[test]
fn overflowing_numeric_time_becomes_an_absent_claim_not_a_fabricated_one() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    // All-digit, so gix's own raw-signature scan captures it whole; only the `i64` conversion fails.
    commit_with_raw_time(&r, "99999999999999999999 +0000");

    let ir = decode(r.path(), &Options::default()).unwrap();
    let atom = &ir.atoms[0];
    assert!(
        atom.metadata.author_time.is_none(),
        "an overflowing time is absent, never fabricated as 0 (NG-5)"
    );
    assert!(atom.metadata.commit_time.is_none(), "same for commit_time");
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "unparseable author/committer times (2)"),
        "both the author and committer time failed to parse"
    );
}

// Not `"1695456000abc"`: gix's raw-signature scanner (`is_time_byte`) stops at the first byte
// outside `[-+0-9 \t]`, so a letter glued onto the digits truncates the captured `time` field to
// clean digits *before* this code ever sees it — the "abc" instead corrupts the *next* header's
// parse, an unrelated, earlier failure (`Error::Read("object parsing failed")`), verified directly
// against this build. An embedded `-` stays inside `is_time_byte`'s alphabet, so the raw field is
// captured whole as `"16954-56000"` — syntactically time-shaped, numerically not `-?[0-9]+`, and it
// reaches `strict_seconds` intact, which is the seam R-1 exists to guard.
#[test]
fn a_time_token_that_is_not_purely_digits_becomes_an_absent_claim() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    commit_with_raw_time(&r, "16954-56000 +0000");

    let ir = decode(r.path(), &Options::default()).unwrap();
    let atom = &ir.atoms[0];
    assert!(
        atom.metadata.author_time.is_none(),
        "a non-digit-shaped time is absent, never salvaged to its leading digits"
    );
    assert!(atom.metadata.commit_time.is_none(), "same for commit_time");
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "unparseable author/committer times (2)")
    );
}

#[test]
fn strict_seconds_accepts_well_formed_tokens_and_rejects_malformed_ones() {
    assert_eq!(strict_seconds("1695456000 +0000"), Some(1_695_456_000));
    assert_eq!(strict_seconds("-1695456000 +0000"), Some(-1_695_456_000));
    assert_eq!(strict_seconds("0 +0000"), Some(0));
    assert_eq!(
        strict_seconds("+1695456000 +0000"),
        None,
        "a leading + is not -?[0-9]+"
    );
    assert_eq!(strict_seconds("1695456000abc +0000"), None);
    assert_eq!(strict_seconds("16954-56000 +0000"), None);
    assert_eq!(
        strict_seconds("99999999999999999999 +0000"),
        None,
        "overflows i64"
    );
    assert_eq!(strict_seconds(""), None);
    assert_eq!(strict_seconds("   "), None);
}

#[test]
fn an_empty_repository_decodes_to_an_empty_ir() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(ir.atoms.is_empty());
    assert!(ir.refs.is_empty());
}

// --- OQ-B: annotated-tag identity preservation -------------------------------------------------

#[test]
fn annotated_tag_preserves_identity_and_records_loss() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let commit = r.git(&["rev-parse", "HEAD"]);
    r.git(&["tag", "-a", "rel-1", "-m", "the first release"]);
    let tag_sha = r.git(&["rev-parse", "rel-1"]); // the annotated tag object's own id
    assert_ne!(tag_sha, commit, "an annotated tag has its own object id");

    let ir = decode(r.path(), &Options::default()).unwrap();
    let tag = ir
        .refs
        .iter()
        .find(|rf| rf.name == "rel-1")
        .expect("tag carried");
    assert!(matches!(tag.kind, brygge_ir::RefKind::Tag));
    let src = tag
        .source
        .as_ref()
        .expect("an annotated tag preserves its opaque source identity (PR-4)");
    let hex: String = src.atom_id.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex, tag_sha,
        "the preserved id is the tag object's sha, not the commit's"
    );
    // Its authored tagger/message are recorded as loss, never silently dropped (PR-9).
    assert!(ir.loss.dropped.iter().any(
        |d| d.what.contains("annotated tag") && matches!(d.class, brygge_ir::LossClass::Other)
    ));
}

#[test]
fn lightweight_tag_has_no_separate_identity_and_no_tag_loss() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    r.git(&["tag", "light"]); // lightweight — points straight at the commit
    let ir = decode(r.path(), &Options::default()).unwrap();
    let tag = ir
        .refs
        .iter()
        .find(|rf| rf.name == "light")
        .expect("carried");
    assert!(
        tag.source.is_none(),
        "a lightweight tag has no separate object identity"
    );
    assert!(
        !ir.loss
            .dropped
            .iter()
            .any(|d| d.what.contains("annotated tag"))
    );
}

// --- OQ-A: exact-content rename detection is 1:1 only ------------------------------------------

#[test]
fn ambiguous_identical_content_move_is_not_marked() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "same\n");
    r.write("b.txt", "same\n"); // identical content -> same blob id
    r.commit_all("two identical files");
    std::fs::remove_file(r.path().join("a.txt")).unwrap();
    std::fs::remove_file(r.path().join("b.txt")).unwrap();
    r.write("x.txt", "same\n");
    r.write("y.txt", "same\n"); // same blob id, added at two paths
    r.commit_all("shuffle identical content");

    let opts = Options {
        detect_renames: true,
        rename_threshold: 100,
    };
    let ir = decode(r.path(), &opts).unwrap();
    // A 2->2 identical-content shuffle is ambiguous: brygge does not guess which became which.
    assert!(
        ir.atoms.iter().all(|a| a.rename_hints.is_empty()),
        "ambiguous identical-content moves are left unmarked (OQ-A)"
    );
    assert!(brygge_ir::honesty::summary(&ir).derived.is_empty());
    // The literal delete+add remain, so nothing is lost by declining to guess.
    let move_atom = ir
        .atoms
        .iter()
        .find(|a| {
            a.ops
                .iter()
                .any(|op| matches!(op, brygge_ir::PathOp::Add { path, .. } if path == "x.txt"))
        })
        .expect("the shuffle commit exists");
    assert!(
        move_atom
            .ops
            .iter()
            .any(|op| matches!(op, brygge_ir::PathOp::Delete { path, .. } if path == "a.txt"))
    );
}

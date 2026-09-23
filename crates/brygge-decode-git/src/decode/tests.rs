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
    // A stash on top of everything else (requirement 7: determinism/pack-independence with a stash
    // present). `refs/stash` is a dropped namespace (CR-05); this must not change what decodes.
    r.write("readme.txt", "hello world, locally dirty\n");
    r.git(&["stash", "push", "-q", "-m", "wip"]);
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
        infer_renames: true,
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
        whats.contains(&"remote-tracking refs (1)"),
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
        infer_renames: true,
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

// --- Git corrections batch 1 (CR-05, CR-11/D-3(ii), CR-03/D-3(i), CR-06) -----------------------------

#[test]
fn stash_notes_and_remote_tracking_are_excluded_and_counted() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    // Baseline: no workflow refs at all, for the repo_id comparison below.
    let baseline = TempRepo::new();
    baseline.write("a.txt", "a\n");
    baseline.commit_all("c1");
    let baseline_ir = decode(baseline.path(), &Options::default()).unwrap();

    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let head = r.git(&["rev-parse", "HEAD"]);

    r.write("a.txt", "b\n");
    r.git(&["stash", "push", "-q", "-m", "wip"]);

    r.git(&["notes", "add", "-m", "a note", &head]);

    r.write("only-on-remote.txt", "x\n");
    r.commit_all("remote-only commit");
    let remote_head = r.git(&["rev-parse", "HEAD"]);
    r.git(&["update-ref", "refs/remotes/origin/x", &remote_head]);
    r.git(&["reset", "-q", "--hard", &head]); // main no longer carries the remote-only commit

    let ir = decode(r.path(), &Options::default()).unwrap();
    assert_eq!(
        ir.atoms.len(),
        1,
        "only c1 is carried history: {:?}",
        ir.atoms
    );
    assert!(
        ir.atoms.iter().all(|a| a.ops.iter().all(
            |op| !matches!(op, brygge_ir::PathOp::Add { path, .. } if path == "only-on-remote.txt")
        )),
        "the commit reachable only from refs/remotes/origin/x never became an atom"
    );

    let whats: Vec<&str> = ir.loss.dropped.iter().map(|d| d.what.as_str()).collect();
    assert!(whats.contains(&"stash refs (1)"), "{whats:?}");
    assert!(whats.contains(&"notes refs (1)"), "{whats:?}");
    assert!(whats.contains(&"remote-tracking refs (1)"), "{whats:?}");
    assert!(
        whats
            .iter()
            .any(|w| w.starts_with("commits reachable only from dropped refs")),
        "{whats:?}"
    );

    assert_eq!(
        ir.provenance.source.repo_id, baseline_ir.provenance.source.repo_id,
        "repo_id is computed over imported commits only, unaffected by dropped-namespace refs"
    );
}

#[test]
fn alternates_are_refused_but_an_empty_file_is_not() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let origin = TempRepo::new();
    origin.write("a.txt", "a\n");
    origin.commit_all("c1");

    let clone_dir = std::env::temp_dir().join(format!(
        "brygge-git-clone-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    // `git clone --shared` creates objects/info/alternates pointing at the origin's object store.
    let out = Command::new("git")
        .args([
            "clone",
            "-q",
            "--shared",
            origin.path().to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git clone --shared failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    match decode(&clone_dir, &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => assert_eq!(feature, "object alternates"),
        other => panic!("expected an object-alternates refusal, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&clone_dir);

    // A hand-written EMPTY alternates file is not refused.
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    std::fs::create_dir_all(r.path().join(".git/objects/info")).unwrap();
    std::fs::write(r.path().join(".git/objects/info/alternates"), b"").unwrap();
    assert!(decode(r.path(), &Options::default()).is_ok());
}

#[test]
fn redirected_git_directory_is_refused_but_main_repo_succeeds() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");

    let worktree_dir = std::env::temp_dir().join(format!(
        "brygge-git-worktree-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    r.git(&[
        "worktree",
        "add",
        "-q",
        worktree_dir.to_str().unwrap(),
        "-b",
        "wt-branch",
    ]);

    // The linked worktree's `.git` is a file (a `gitdir:` redirect).
    match decode(&worktree_dir, &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, "redirected git directory");
        }
        other => panic!("expected a redirected-git-directory refusal, got {other:?}"),
    }

    // The main repository still decodes.
    assert!(decode(r.path(), &Options::default()).is_ok());

    r.git(&["worktree", "remove", "-f", worktree_dir.to_str().unwrap()]);
    let _ = std::fs::remove_dir_all(&worktree_dir);
}

fn unique_temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "brygge-git-{label}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(unix)]
#[test]
fn a_symlinked_git_directory_is_refused() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let target = TempRepo::new();
    target.write("a.txt", "a\n");
    target.commit_all("c1");

    let outer = unique_temp_dir("symlink-outer");
    std::fs::create_dir_all(&outer).unwrap();
    std::os::unix::fs::symlink(target.path().join(".git"), outer.join(".git")).unwrap();

    match decode(&outer, &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, "redirected git directory");
        }
        other => panic!("expected a redirected-git-directory refusal, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&outer);
}

#[cfg(unix)]
#[test]
fn a_symlinked_objects_directory_is_refused() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let target = TempRepo::new();
    target.write("a.txt", "a\n");
    target.commit_all("c1");

    let r = TempRepo::new();
    r.write("b.txt", "b\n");
    r.commit_all("c1");
    let objects = r.path().join(".git").join("objects");
    std::fs::remove_dir_all(&objects).unwrap();
    std::os::unix::fs::symlink(target.path().join(".git").join("objects"), &objects).unwrap();

    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, "redirected git directory");
        }
        other => panic!("expected a redirected-git-directory refusal, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_pack_file_is_refused() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    r.git(&["repack", "-a", "-d", "-q"]);

    let pack_dir = r.path().join(".git").join("objects").join("pack");
    let entries: Vec<PathBuf> = std::fs::read_dir(&pack_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    let victim = entries.first().expect("repack produced pack files").clone();
    let name = victim.file_name().unwrap().to_owned();
    std::fs::remove_file(&victim).unwrap();
    // The symlink need not resolve; detection is of the symlink itself (`symlink_metadata`).
    std::os::unix::fs::symlink(
        pack_dir.join("does-not-exist").with_file_name(name),
        &victim,
    )
    .unwrap();

    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, "redirected git directory");
        }
        other => panic!("expected a redirected-git-directory refusal, got {other:?}"),
    }
}

#[test]
fn an_unsymlinked_repository_decodes_normally() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    assert!(decode(r.path(), &Options::default()).is_ok());
}

#[test]
fn corrupt_dropped_only_history_does_not_fail_the_decode() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let head = r.git(&["rev-parse", "HEAD"]);

    r.write("a.txt", "b\n");
    r.git(&["stash", "push", "-q", "-m", "wip"]);
    let stash_id = r.git(&["rev-parse", "refs/stash"]);
    // The stash commit's second parent is the index-tree commit — reachable only from refs/stash,
    // never from any carried ref.
    let index_parent = r.git(&["rev-parse", &format!("{stash_id}^2")]);
    assert_ne!(
        index_parent, head,
        "the index-tree parent is a commit of its own, not HEAD"
    );

    let loose = r
        .path()
        .join(".git")
        .join("objects")
        .join(&index_parent[..2])
        .join(&index_parent[2..]);
    assert!(
        loose.exists(),
        "expected a loose object at {}",
        loose.display()
    );
    std::fs::remove_file(&loose).unwrap();

    let ir = decode(r.path(), &Options::default()).unwrap();
    assert_eq!(ir.atoms.len(), 1, "carried history (c1) is intact");
    let whats: Vec<&str> = ir.loss.dropped.iter().map(|d| d.what.as_str()).collect();
    assert!(
        whats
            .iter()
            .any(|w| w.starts_with("commits reachable only from dropped refs (count unavailable")),
        "{whats:?}"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_path_is_refused_with_escaped_bytes_and_commit_hex() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    use std::os::unix::ffi::OsStrExt;

    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let blob = r.git(&["hash-object", "-w", "--", "a.txt"]);

    let mut cacheinfo_bytes = format!("100644,{blob},bad-").into_bytes();
    cacheinfo_bytes.push(0xFF);
    cacheinfo_bytes.extend_from_slice(b".txt");
    let cacheinfo = std::ffi::OsStr::from_bytes(&cacheinfo_bytes).to_os_string();

    let out = Command::new("git")
        .current_dir(r.path())
        .arg("update-index")
        .arg("--add")
        .arg("--cacheinfo")
        .arg(&cacheinfo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "update-index failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = Command::new("git")
        .current_dir(r.path())
        .args([
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "add non-utf8 path",
        ])
        .env("GIT_AUTHOR_NAME", "A U Thor")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", "2005-04-07T22:13:13 +0000")
        .env("GIT_COMMITTER_NAME", "C O Mitter")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_DATE", "2005-04-07T22:13:13 +0000")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let commit_hex = r.git(&["rev-parse", "HEAD"]);

    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, reason }) => {
            assert_eq!(feature, "non-UTF-8 path");
            assert!(
                reason.contains("\\xFF"),
                "reason should escape the invalid byte: {reason}"
            );
            assert!(
                reason.contains(&commit_hex),
                "reason should name the commit: {reason}"
            );
        }
        other => panic!("expected a non-UTF-8-path refusal, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_ref_name_is_refused() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    use std::os::unix::ffi::OsStrExt;

    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let head = r.git(&["rev-parse", "HEAD"]);

    let mut name_bytes = b"refs/heads/bad-".to_vec();
    name_bytes.push(0xFF);
    let ref_name = std::ffi::OsStr::from_bytes(&name_bytes).to_os_string();

    let out = Command::new("git")
        .current_dir(r.path())
        .arg("update-ref")
        .arg(&ref_name)
        .arg(&head)
        .output()
        .unwrap();
    if !out.status.success() {
        eprintln!(
            "skipping: this git/platform rejects a non-UTF-8 ref name: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }

    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, reason }) => {
            assert_eq!(feature, "non-UTF-8 ref name");
            assert!(
                reason.contains("\\xFF"),
                "reason should escape the invalid byte: {reason}"
            );
        }
        other => panic!("expected a non-UTF-8-ref-name refusal, got {other:?}"),
    }
}

/// Fallback unit coverage of the conversion itself (requirement 5: "cover the conversion with a unit
/// test" when the fixture above cannot be built on a given platform).
#[test]
fn escape_invalid_utf8_never_substitutes_u_fffd() {
    assert_eq!(escape_invalid_utf8(b"bad-\xFF.txt"), "bad-\\xFF.txt");
    assert_eq!(escape_invalid_utf8(b"plain"), "plain");
    assert_eq!(escape_invalid_utf8(b"\xC0\xC1"), "\\xC0\\xC1");
    assert!(!escape_invalid_utf8(b"\xFF").contains('\u{FFFD}'));
}

#[test]
fn tag_on_a_blob_is_not_a_ref_record_but_is_counted() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let blob = r.git(&["hash-object", "-w", "--", "a.txt"]);
    r.git(&["tag", "blobtag", &blob]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(
        ir.refs.iter().all(|rf| rf.name != "blobtag"),
        "a tag on a blob is never a RefRecord (it has no atom to target)"
    );
    let whats: Vec<&str> = ir.loss.dropped.iter().map(|d| d.what.as_str()).collect();
    assert!(
        whats.contains(&"refs to non-commit objects (1)"),
        "the drop is counted, not silently skipped: {whats:?}"
    );
}

// --- RFC 010 CR-10: resource ceilings ----------------------------------------------------------------

/// Run `git <args>` with `stdin_data` piped in and pinned identity/dates, returning trimmed stdout.
fn git_stdin(dir: &Path, args: &[&str], stdin_data: &[u8]) -> String {
    use std::io::Write as _;
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "A U Thor")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", "2005-04-07T22:13:13 +0000")
        .env("GIT_COMMITTER_NAME", "C O Mitter")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_DATE", "2005-04-07T22:13:13 +0000")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin_data).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A tree nested exactly `depth` levels deep (the innermost level holds one file, `leaf.txt`), built
/// with `git mktree`, without touching the filesystem (a git tree entry name has no OS path-length
/// limit, only the ceiling brygge itself enforces).
fn build_nested_tree(r: &TempRepo, depth: usize) -> String {
    assert!(depth >= 1);
    let blob = git_stdin(r.path(), &["hash-object", "-w", "--stdin"], b"leaf\n");
    let mut tree = git_stdin(
        r.path(),
        &["mktree"],
        format!("100644 blob {blob}\tleaf.txt\n").as_bytes(),
    );
    for _ in 1..depth {
        tree = git_stdin(
            r.path(),
            &["mktree"],
            format!("040000 tree {tree}\td\n").as_bytes(),
        );
    }
    tree
}

fn commit_tree(r: &TempRepo, tree: &str, msg: &str) -> String {
    let commit = git_stdin(r.path(), &["commit-tree", tree, "-m", msg], b"");
    r.git(&["update-ref", "refs/heads/main", &commit]);
    commit
}

#[test]
fn a_blob_over_the_limit_is_refused() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("big.bin", &"x".repeat(5_000_000));
    r.commit_all("big blob");

    // The header check (`find_header`) runs before any read of the blob's content; a limit far below
    // the real size is refused instantly rather than after inflating 5 MB.
    let limits = Limits {
        max_blob_bytes: 1024,
        ..Limits::default()
    };
    match decode_with(r.path(), &Options::default(), &limits) {
        Err(crate::Error::ResourceLimit { .. }) => {}
        other => panic!("expected a resource-limit refusal, got {other:?}"),
    }
}

#[test]
fn a_deep_tree_is_refused_by_depth_but_a_shallower_one_decodes() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let limits = Limits {
        max_tree_depth: 50,
        ..Limits::default()
    };

    let too_deep = TempRepo::new();
    let tree = build_nested_tree(&too_deep, 60);
    commit_tree(&too_deep, &tree, "60 levels");
    match decode_with(too_deep.path(), &Options::default(), &limits) {
        Err(crate::Error::ResourceLimit { .. }) => {}
        other => panic!("expected a resource-limit refusal, got {other:?}"),
    }

    let shallow_enough = TempRepo::new();
    let tree = build_nested_tree(&shallow_enough, 40);
    commit_tree(&shallow_enough, &tree, "40 levels");
    let ir = decode_with(shallow_enough.path(), &Options::default(), &limits)
        .expect("40 levels is within the injected 50-level ceiling");
    assert_eq!(ir.atoms.len(), 1);
}

#[test]
fn a_path_over_the_limit_is_refused() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    let blob = git_stdin(r.path(), &["hash-object", "-w", "--stdin"], b"x\n");
    let long_name = "a".repeat(5000);
    let tree = git_stdin(
        r.path(),
        &["mktree"],
        format!("100644 blob {blob}\t{long_name}\n").as_bytes(),
    );
    commit_tree(&r, &tree, "long path");

    let limits = Limits {
        max_path_bytes: 100,
        ..Limits::default()
    };
    match decode_with(r.path(), &Options::default(), &limits) {
        Err(crate::Error::ResourceLimit { .. }) => {}
        other => panic!("expected a resource-limit refusal, got {other:?}"),
    }
}

#[test]
fn the_commit_cap_refuses_when_exceeded() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    for i in 0..5 {
        r.write("a.txt", &format!("{i}\n"));
        r.commit_all(&format!("c{i}"));
    }
    let limits = Limits {
        max_commits: 2,
        ..Limits::default()
    };
    match decode_with(r.path(), &Options::default(), &limits) {
        Err(crate::Error::ResourceLimit { .. }) => {}
        other => panic!("expected a resource-limit refusal, got {other:?}"),
    }
}

// --- CR-12.2: the floor is one declared list, recorded in provenance --------------------------------

#[test]
fn provenance_floor_param_equals_the_declared_list() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let ir = decode(r.path(), &Options::default()).unwrap();
    let expected = crate::floor::joined();
    assert_eq!(ir.provenance.params.get("floor"), Some(&expected));
}

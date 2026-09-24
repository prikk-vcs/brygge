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

/// Lowercase hex of `bytes` (built with `write!`, not `format!` per byte).
fn hex_of(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

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
        any.metadata
            .author
            .as_ref()
            .unwrap()
            .email
            .as_ref()
            .and_then(brygge_ir::Text::as_utf8),
        Some("author@example.com")
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

    // Off by default: the literal delete+add, no copy record.
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(ir.atoms.iter().all(|a| a.copies.is_empty()));
    assert!(brygge_ir::honesty::summary(&ir).derived.is_empty());

    // On: a Derived InferredRename copy record appears beside the still-present literal ops.
    let opts = Options {
        infer_renames: true,
        rename_threshold: 100,
    };
    let ir = decode(r.path(), &opts).unwrap();
    let move_atom = ir
        .atoms
        .iter()
        .find(|a| !a.copies.is_empty())
        .expect("the move commit carries a copy record");
    let copy = &move_atom.copies[0];
    assert_eq!(copy.from, "old/name.txt");
    assert_eq!(copy.to, "new/name.txt");
    assert_eq!(
        copy.from_atom, move_atom.parents[0],
        "from_atom is the first parent"
    );
    assert!(
        move_atom.is_move(copy),
        "a delete+add move is reported as a move"
    );
    assert!(copy.status.is_derived(), "an inferred rename is Derived");
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
    assert!(
        ir.loss
            .dropped
            .iter()
            .all(|d| matches!(d.class, brygge_ir::LossClass::Representation)),
        "an annotated tag's tagger/time/message is now carried in `annotation`, not dropped \
         (batch-2 handoff §1.6): {:?}",
        ir.loss.dropped
    );
    assert!(
        !whats.iter().any(|w| w.contains("annotated tag")),
        "the old annotated-tag drop record is gone: {whats:?}"
    );
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
        Err(crate::Error::FloorRefusal { ref feature, .. }) if feature == "shallow-clone"
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
        Err(crate::Error::FloorRefusal { ref feature, .. }) if feature == "replace-ref"
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
            .any(|d| d.what == "unparseable author/committer/tagger times (2)"),
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
            .any(|d| d.what == "unparseable author/committer/tagger times (2)")
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
fn strict_offset_minutes_accepts_well_formed_tokens_and_rejects_malformed_ones() {
    assert_eq!(strict_offset_minutes("1112911993 +0900"), Some(540));
    assert_eq!(strict_offset_minutes("1112911993 -0130"), Some(-90));
    assert_eq!(strict_offset_minutes("1112911993 +0000"), Some(0));
    assert_eq!(
        strict_offset_minutes("1112911993 +09"),
        None,
        "not exactly four digits"
    );
    assert_eq!(
        strict_offset_minutes("1112911993 +0960"),
        None,
        "MM must be < 60"
    );
    assert_eq!(
        strict_offset_minutes("1112911993 0900"),
        None,
        "no sign is not +HHMM/-HHMM"
    );
    assert_eq!(
        strict_offset_minutes("1112911993 +09a0"),
        None,
        "non-digit in the offset"
    );
    assert_eq!(
        strict_offset_minutes("1112911993"),
        None,
        "missing offset token entirely"
    );
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
    let hex = hex_of(&src.atom_id);
    assert_eq!(
        hex, tag_sha,
        "the preserved id is the tag object's sha, not the commit's"
    );
    // Its tagger/time/message are now carried, not dropped (batch-2 handoff §1.6).
    assert!(
        !ir.loss
            .dropped
            .iter()
            .any(|d| d.what.contains("annotated tag")),
        "the old annotated-tag drop record is gone"
    );
    let annotation = tag
        .annotation
        .as_ref()
        .expect("an annotated tag's tagger/time/message is carried as an Annotation");
    assert_eq!(
        annotation.tagger.as_ref().and_then(|t| t.name.as_utf8()),
        Some("C O Mitter")
    );
    assert_eq!(
        annotation.message.as_ref().and_then(|m| m.as_utf8()),
        Some("the first release\n")
    );
    assert!(annotation.time.is_some());
    // An annotated-tag-only repository (no other loss) exits clean, per the handoff's own required test.
    assert!(
        ir.loss
            .dropped
            .iter()
            .all(|d| matches!(d.class, brygge_ir::LossClass::Representation)),
        "nothing but representation-class drops remain: {:?}",
        ir.loss.dropped
    );
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
        ir.atoms.iter().all(|a| a.copies.is_empty()),
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
        Err(crate::Error::FloorRefusal { feature, .. }) => assert_eq!(feature, "object-alternates"),
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
            assert_eq!(feature, "redirected-git-directory");
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

/// Create a link of the given kind, or say why it could not be. A test that cannot make the link (Windows
/// without the symlink privilege) is skipped with a message on a developer machine, and **fails on CI**, so
/// a skip can never hide a missing proof there.
fn link_created(what: &str, result: std::io::Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("SKIPPED: cannot create {what} here: {e}");
            false
        }
        Err(e) => panic!("cannot create {what} on CI: {e}"),
    }
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

/// A directory junction (`mklink /J`): a reparse point that is not a symlink and needs no privilege.
#[cfg(windows)]
fn junction(target: &Path, link: &Path) -> std::io::Result<()> {
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "mklink /J exited with {status}"
        )))
    }
}

fn assert_redirected(result: Result<brygge_ir::Ir, crate::Error>) {
    match result {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, "redirected-git-directory");
        }
        other => panic!("expected a redirected-git-directory refusal, got {other:?}"),
    }
}

/// Which directory of a repository is replaced by a link to the same directory of another repository.
#[derive(Clone, Copy, Debug)]
enum Redirected {
    GitDir,
    Objects,
    ObjectsInfo,
    ObjectsPack,
}

impl Redirected {
    /// The path of this directory inside `repo`, relative to the repository's working directory.
    fn inside(self, repo: &Path) -> PathBuf {
        let git = repo.join(".git");
        match self {
            Self::GitDir => git,
            Self::Objects => git.join("objects"),
            Self::ObjectsInfo => git.join("objects").join("info"),
            Self::ObjectsPack => git.join("objects").join("pack"),
        }
    }
}

/// Replace `which` of a fresh repository by a link (made by `link`) to the same directory of another
/// repository, then decode it: it must be refused as `redirected-git-directory`. Returns without a
/// verdict only when the link could not be made (see [`link_created`]).
fn refuse_a_redirected_directory(
    which: Redirected,
    what: &str,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let target = TempRepo::new();
    target.write("a.txt", "a\n");
    target.commit_all("c1");
    // `objects/pack` exists only after a repack; make sure both repositories have every directory.
    target.git(&["repack", "-a", "-d", "-q"]);
    std::fs::create_dir_all(Redirected::ObjectsInfo.inside(target.path())).unwrap();

    let r = TempRepo::new();
    r.write("b.txt", "b\n");
    r.commit_all("c1");
    r.git(&["repack", "-a", "-d", "-q"]);
    std::fs::create_dir_all(Redirected::ObjectsInfo.inside(r.path())).unwrap();

    // For `GitDir` the link stands in for `.git` of an otherwise empty directory (the repository under
    // test is that directory); for the others it replaces the directory inside `r`.
    let (subject, replaced) = if matches!(which, Redirected::GitDir) {
        let outer = unique_temp_dir("redirected-outer");
        std::fs::create_dir_all(&outer).unwrap();
        let replaced = which.inside(&outer);
        (outer, replaced)
    } else {
        let replaced = which.inside(r.path());
        std::fs::remove_dir_all(&replaced).unwrap();
        (r.path().to_path_buf(), replaced)
    };
    let made = link_created(what, link(&which.inside(target.path()), &replaced));
    let result = if made {
        Some(decode(&subject, &Options::default()))
    } else {
        None
    };
    if matches!(which, Redirected::GitDir) {
        let _ = std::fs::remove_dir_all(&subject);
    }
    if let Some(result) = result {
        assert_redirected(result);
    }
}

#[test]
fn a_symlinked_git_directory_is_refused() {
    refuse_a_redirected_directory(Redirected::GitDir, "a directory symlink", symlink_dir);
}

#[test]
fn a_symlinked_objects_directory_is_refused() {
    refuse_a_redirected_directory(Redirected::Objects, "a directory symlink", symlink_dir);
}

#[test]
fn a_symlinked_objects_info_directory_is_refused() {
    refuse_a_redirected_directory(Redirected::ObjectsInfo, "a directory symlink", symlink_dir);
}

#[test]
fn a_symlinked_objects_pack_directory_is_refused() {
    refuse_a_redirected_directory(Redirected::ObjectsPack, "a directory symlink", symlink_dir);
}

/// RFC 012 D-9 / threat model C-2c: on Windows a directory junction is treated like a symlink in every
/// one of Git's redirected-directory checks. `std` reports a junction as a symlink (its reparse tag is a
/// "name surrogate"), so no Windows-specific product code is needed; these tests are the proof.
#[cfg(windows)]
mod junctions {
    use super::*;

    #[test]
    fn a_junctioned_git_directory_is_refused() {
        refuse_a_redirected_directory(Redirected::GitDir, "a directory junction", junction);
    }

    #[test]
    fn a_junctioned_objects_directory_is_refused() {
        refuse_a_redirected_directory(Redirected::Objects, "a directory junction", junction);
    }

    #[test]
    fn a_junctioned_objects_info_directory_is_refused() {
        refuse_a_redirected_directory(Redirected::ObjectsInfo, "a directory junction", junction);
    }

    #[test]
    fn a_junctioned_objects_pack_directory_is_refused() {
        refuse_a_redirected_directory(Redirected::ObjectsPack, "a directory junction", junction);
    }

    #[test]
    fn std_reports_a_junction_as_a_symlink() {
        // Recorded for the review: what `std` actually says about a junction on this runner.
        let dir = unique_temp_dir("junction-std");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.join("link");
        assert!(link_created("a directory junction", junction(&real, &link)));
        let ft = std::fs::symlink_metadata(&link).unwrap().file_type();
        eprintln!(
            "junction: is_symlink={} is_dir={} is_file={}",
            ft.is_symlink(),
            ft.is_dir(),
            ft.is_file()
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert!(ft.is_symlink());
    }
}

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
    let made = link_created(
        "a file symlink",
        symlink_file(
            &pack_dir.join("does-not-exist").with_file_name(name),
            &victim,
        ),
    );
    if made {
        assert_redirected(decode(r.path(), &Options::default()));
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
    // The record is fixed (batch-2 handoff §1.2) — never the raw gix error text, which is neither
    // deterministic nor identity-bearing.
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "commits reachable only from dropped refs (count unavailable)"),
        "{:?}",
        ir.loss.dropped
    );
    let ir2 = decode(r.path(), &Options::default()).unwrap();
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&ir2),
        "decoding twice gives identical bytes, count-unavailable case included"
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
            assert_eq!(feature, "non-utf8-path");
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
            assert_eq!(feature, "non-utf8-ref-name");
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

// --- batch-2 handoff §1.1: every read object is verified against its id -----------------------------

#[test]
fn a_retargeted_loose_object_is_refused_naming_the_id() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "victim content\n");
    r.commit_all("c1");
    let victim_blob = r.git(&["hash-object", "--", "a.txt"]);

    // A second, distinct blob of the same kind, loose in the same store.
    let attacker_blob = git_stdin(
        r.path(),
        &["hash-object", "-w", "--stdin"],
        b"attacker content\n",
    );
    assert_ne!(victim_blob, attacker_blob);

    let loose_path = |sha: &str| {
        r.path()
            .join(".git")
            .join("objects")
            .join(&sha[..2])
            .join(&sha[2..])
    };
    let victim_path = loose_path(&victim_blob);
    let attacker_path = loose_path(&attacker_blob);
    assert!(victim_path.exists(), "victim blob must be loose");
    assert!(attacker_path.exists(), "attacker blob must be loose");
    // Retarget: the victim's id now names the attacker's content on disk. Git writes loose objects
    // read-only, so the victim file's permissions are relaxed before overwriting it.
    let attacker_bytes = std::fs::read(&attacker_path).unwrap();
    let mut perms = std::fs::metadata(&victim_path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&victim_path, perms).unwrap();
    std::fs::write(&victim_path, attacker_bytes).unwrap();

    match decode(r.path(), &Options::default()) {
        Err(crate::Error::Read(msg)) => {
            assert!(msg.contains(&victim_blob), "names the mismatched id: {msg}");
            assert!(msg.contains("does not match its content"));
        }
        other => panic!("expected Error::Read naming the mismatched object, got {other:?}"),
    }
}

#[test]
fn an_ordinary_repository_decodes_with_every_object_verified() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = build_rich_repo();
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(!ir.atoms.is_empty());
}

// --- batch-2 handoff §1.1 investigation: a SHA-256 object-format repository -------------------------

#[test]
fn a_sha256_object_format_repository_is_refused_with_the_named_floor_feature() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("brygge-git-sha256-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new("git")
        .current_dir(&dir)
        .args(["init", "-q", "--object-format=sha256"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    if !out.status.success() {
        eprintln!("skipping: installed git does not support --object-format=sha256");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let result = decode(&dir, &Options::default());
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, "sha256-object-format");
        }
        other => panic!("expected FloorRefusal naming the SHA-256 feature, got {other:?}"),
    }
}

// --- batch-2 handoff §1.3: the message's declared encoding, on the message only ----------------------

#[test]
fn a_declared_message_encoding_is_carried_on_the_message_only() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.git(&["add", "-A"]);
    let tree = r.git(&["write-tree"]);
    let mut body = format!(
        "tree {tree}\n\
         author A U Thor <author@example.com> 1112911993 +0000\n\
         committer A U Thor <author@example.com> 1112911993 +0000\n\
         encoding ISO-8859-1\n\
         \n"
    )
    .into_bytes();
    // Latin-1 'é' (0xE9) — not valid UTF-8 on its own, carried byte-exact regardless.
    body.extend_from_slice(&[0xE9, b'\n']);
    let sha = git_stdin(
        r.path(),
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--literally",
            "--stdin",
        ],
        &body,
    );
    r.git(&["update-ref", "refs/heads/main", &sha]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let atom = &ir.atoms[0];
    let msg = atom.metadata.message.as_ref().unwrap();
    assert_eq!(msg.bytes, vec![0xE9, b'\n']);
    assert_eq!(msg.encoding.as_deref(), Some("ISO-8859-1"));
    let author_name = &atom.metadata.author.as_ref().unwrap().name;
    assert_eq!(
        author_name.encoding, None,
        "names keep encoding: None — \"not stated\", never the message's encoding"
    );
}

// --- batch-2 handoff §1.4: a malformed offset is counted in the loss boundary ------------------------

#[test]
fn a_malformed_timezone_offset_is_absent_and_counted() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    // Well-formed seconds, malformed offset (three digits).
    commit_with_raw_time(&r, "1112911993 +090");

    let ir = decode(r.path(), &Options::default()).unwrap();
    let atom = &ir.atoms[0];
    let author_time = atom.metadata.author_time.expect("seconds still parsed");
    assert_eq!(author_time.seconds, 1_112_911_993);
    assert_eq!(
        author_time.offset_minutes, None,
        "a malformed offset is absent, never salvaged or defaulted"
    );
    assert!(
        ir.loss.dropped.iter().any(|d| d
            .what
            .starts_with("unparseable author/committer/tagger timezone offsets")),
        "{:?}",
        ir.loss.dropped
    );
}

// --- batch-2 handoff §1.5: signatures and extras, in header order -------------------------------------

#[test]
fn a_mergetag_header_is_carried_as_an_extra_with_unfolded_bytes() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.git(&["add", "-A"]);
    let tree = r.git(&["write-tree"]);
    // A `mergetag` header's continuation lines are each prefixed with one space (git's own header
    // folding); gix unfolds them back to real newlines, stripping the leading space.
    let body = format!(
        "tree {tree}\n\
         author A U Thor <author@example.com> 1112911993 +0000\n\
         committer A U Thor <author@example.com> 1112911993 +0000\n\
         mergetag object 0000000000000000000000000000000000000000\n\
        \x20type commit\n\
        \x20tag faketag\n\
        \x20tagger t <t@example.com> 0 +0000\n\
        \x20\n\
        \x20fake mergetag message\n\
         \n\
         c1\n"
    );
    let sha = git_stdin(
        r.path(),
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--literally",
            "--stdin",
        ],
        body.as_bytes(),
    );
    r.git(&["update-ref", "refs/heads/main", &sha]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let atom = &ir.atoms[0];
    let extra = atom
        .source
        .extras
        .iter()
        .find(|e| e.label == "mergetag")
        .expect("mergetag carried as an Extra");
    let expected = "object 0000000000000000000000000000000000000000\ntype commit\ntag faketag\n\
                     tagger t <t@example.com> 0 +0000\n\nfake mergetag message\n";
    assert_eq!(String::from_utf8_lossy(&extra.bytes), expected);
}

#[test]
fn gpgsig_and_gpgsig_sha256_are_both_carried_as_signatures_in_header_order() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.git(&["add", "-A"]);
    let tree = r.git(&["write-tree"]);
    let body = format!(
        "tree {tree}\n\
         author A U Thor <author@example.com> 1112911993 +0000\n\
         committer A U Thor <author@example.com> 1112911993 +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n\
        \x20\n\
        \x20fake-sha1-signature\n\
        \x20-----END PGP SIGNATURE-----\n\
         gpgsig-sha256 -----BEGIN PGP SIGNATURE-----\n\
        \x20\n\
        \x20fake-sha256-signature\n\
        \x20-----END PGP SIGNATURE-----\n\
         \n\
         c1\n"
    );
    let sha = git_stdin(
        r.path(),
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--literally",
            "--stdin",
        ],
        body.as_bytes(),
    );
    r.git(&["update-ref", "refs/heads/main", &sha]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let atom = &ir.atoms[0];
    let labels: Vec<&str> = atom
        .source
        .signatures
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(
        labels,
        vec!["gpgsig", "gpgsig-sha256"],
        "both signature labels are present, in header order"
    );
    assert!(atom.source.extras.is_empty(), "neither is also an Extra");
}

// --- review 011 R-1: history comes from verified commits, not the commit-graph cache -----------------

/// Locate a chunk's `(offset, size)` in a commit-graph file: 8-byte header (`CGPH`, version, hash
/// version, chunk count, base-graph count), then `chunks + 1` table entries of a 4-byte id and an
/// 8-byte big-endian offset; a chunk ends where the next entry's offset begins.
fn commit_graph_chunk(bytes: &[u8], id: &[u8; 4]) -> (usize, usize) {
    assert_eq!(&bytes[..4], b"CGPH");
    let chunks = usize::from(bytes[6]);
    for i in 0..chunks {
        let e = 8 + i * 12;
        if &bytes[e..e + 4] == id {
            let off = |at: usize| u64::from_be_bytes(bytes[at + 4..at + 12].try_into().unwrap());
            let start = usize::try_from(off(e)).unwrap();
            let end = usize::try_from(off(e + 12)).unwrap();
            return (start, end - start);
        }
    }
    panic!("chunk {id:?} not found");
}

#[test]
fn a_crafted_commit_graph_cannot_drop_a_parent_and_the_walk_never_reads_it() {
    // Review 011 R-1/F-1: a hand-corrupted `objects/info/commit-graph` that says the tip has no parent.
    // This test fails if `.use_commit_graph(false)` is removed from either walk in `decode.rs`.
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    for (i, msg) in ["c1", "c2", "c3", "c4"].iter().enumerate() {
        r.write("a.txt", &format!("{i}\n"));
        r.commit_all(msg);
    }
    let baseline = decode(r.path(), &Options::default()).unwrap();
    assert_eq!(baseline.atoms.len(), 4);

    r.git(&["commit-graph", "write", "--reachable"]);
    let graph_path = r.path().join(".git/objects/info/commit-graph");
    let mut bytes = std::fs::read(&graph_path).unwrap();

    // Find the tip's index in the sorted OID lookup, then blank its parent-1 field in `CDAT`
    // (36 bytes per commit for SHA-1: 20 tree id, 4 parent-1, 4 parent-2, 8 generation/date).
    let tip_hex = r.git(&["rev-parse", "HEAD"]);
    let tip: Vec<u8> = (0..20)
        .map(|i| u8::from_str_radix(&tip_hex[i * 2..i * 2 + 2], 16).unwrap())
        .collect();
    let (oidl, oidl_len) = commit_graph_chunk(&bytes, b"OIDL");
    let idx = (0..oidl_len / 20)
        .find(|i| bytes[oidl + i * 20..oidl + i * 20 + 20] == tip[..])
        .expect("the tip is in the graph");
    let (cdat, _) = commit_graph_chunk(&bytes, b"CDAT");
    let parent1 = cdat + idx * 36 + 20;
    bytes[parent1..parent1 + 4].copy_from_slice(&0x7000_0000u32.to_be_bytes()); // GRAPH_PARENT_NONE
    let mut perms = std::fs::metadata(&graph_path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&graph_path, perms).unwrap();
    std::fs::write(&graph_path, &bytes).unwrap();

    // First: the corruption bites — a default gix walk over this graph sees fewer commits than exist.
    let repo = gix::open(r.path()).unwrap();
    let tip_id = repo.head_id().unwrap().detach();
    let with_graph = repo.rev_walk([tip_id]).all().unwrap().count();
    let without_graph = repo
        .rev_walk([tip_id])
        .use_commit_graph(false)
        .all()
        .unwrap()
        .count();
    assert_eq!(without_graph, 4);
    assert!(
        with_graph < without_graph,
        "the crafted graph must actually truncate a default walk ({with_graph} vs {without_graph}), \
         or this test proves nothing"
    );

    // Then: the decode never reads the graph at all, so it must equal the no-graph baseline exactly.
    // A `Read("history walk disagrees with commit content")` is what the defense-in-depth check in
    // `commit_parents` would give if a walk *did* trust the graph — accepting it here would let the
    // test pass with `use_commit_graph(false)` removed, so it is deliberately not accepted.
    let ir = decode(r.path(), &Options::default())
        .expect("the graph is ignored, so the decode succeeds exactly as without it");
    assert_eq!(
        ir, baseline,
        "a present commit-graph must never change history"
    );
}

// --- review 011 R-2: tag chains are verified all the way --------------------------------------------

#[test]
fn a_tag_of_a_tag_verifies_the_final_target_and_counts_the_nested_tag() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    r.git(&["tag", "-a", "inner", "-m", "inner tag"]);
    let inner_sha = r.git(&["rev-parse", "inner"]);
    // An outer annotated tag pointing at the *inner tag object*, not at the commit.
    let outer_body = format!(
        "object {inner_sha}\n\
         type tag\n\
         tag outer\n\
         tagger A U Thor <author@example.com> 1112911993 +0000\n\
         \n\
         outer tag\n"
    );
    let outer_sha = git_stdin(
        r.path(),
        &["hash-object", "-t", "tag", "-w", "--literally", "--stdin"],
        outer_body.as_bytes(),
    );
    r.git(&["update-ref", "refs/tags/outer", &outer_sha]);
    // Remove the plain, single-hop `inner` ref so only the two-hop `outer` ref exercises this path
    // (both are still carried refs, which is fine — `inner` just adds an extra, uninteresting ref).

    let ir = decode(r.path(), &Options::default()).unwrap();
    let outer_ref = ir
        .refs
        .iter()
        .find(|rf| rf.name == "outer")
        .expect("outer tag ref is carried");
    // The target resolves all the way to the one commit atom.
    assert_eq!(outer_ref.target, ir.atoms[0].id);
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "nested tag objects not carried (1)"),
        "{:?}",
        ir.loss.dropped
    );
}

#[test]
fn a_branch_pointing_at_a_tag_object_counts_that_tag_as_not_carried() {
    // Review 011 F-2: a branch carries no annotation, so the tag it points at loses its tagger/message.
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    r.git(&["tag", "-a", "t", "-m", "a tag"]);
    let tag_sha = r.git(&["rev-parse", "t"]);
    r.git(&["tag", "-d", "t"]);
    // `git update-ref` refuses a non-commit under refs/heads/, so write the loose ref as a crafted (or
    // hand-edited) repository would have it.
    std::fs::write(r.path().join(".git/refs/heads/x"), format!("{tag_sha}\n")).unwrap();

    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(
        ir.refs.iter().any(|rf| rf.name == "x"),
        "the branch is carried, pointing at the commit"
    );
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "nested tag objects not carried (1)"),
        "{:?}",
        ir.loss.dropped
    );
}

// A Git ref name is bytes in the repository, but reaching it through the `git` command line with a non-UTF-8
// name is a Unix-only act (a Windows command line is UTF-16), so this test is `cfg(unix)`; the same
// property on a Windows *path* is proved in the CVS scanner's tests.
#[cfg(unix)]
#[test]
fn a_non_utf8_replace_ref_name_is_refused_with_escaped_bytes() {
    // Review 011 F-3: the replace-ref refusal must not convert the name lossily.
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    use std::os::unix::ffi::OsStrExt;

    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let head = r.git(&["rev-parse", "HEAD"]);
    let mut name_bytes = b"refs/replace/bad-".to_vec();
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
        eprintln!("skipping: this git/platform rejects a non-UTF-8 ref name");
        return;
    }
    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, reason }) => {
            assert_eq!(feature, "replace-ref");
            assert!(reason.contains("\\xFF"), "escaped byte expected: {reason}");
            assert!(
                !reason.contains('\u{FFFD}'),
                "no lossy substitution: {reason}"
            );
        }
        other => panic!("expected a replace-ref refusal, got {other:?}"),
    }
}

// --- review 011 R-3: nothing unparseable is silent ---------------------------------------------------

#[test]
fn an_unparseable_tagger_time_is_counted_like_author_and_committer_times() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.commit_all("c1");
    let commit_sha = r.git(&["rev-parse", "HEAD"]);
    // Well-formed seconds, malformed offset (three digits) — the same shape as the existing
    // commit-level malformed-offset test, which is known to parse structurally; only the offset token
    // itself is what strict parsing rejects.
    let tag_body = format!(
        "object {commit_sha}\n\
         type commit\n\
         tag broken\n\
         tagger A U Thor <author@example.com> 1112911993 +090\n\
         \n\
         broken tagger offset\n"
    );
    let tag_sha = git_stdin(
        r.path(),
        &["hash-object", "-t", "tag", "-w", "--literally", "--stdin"],
        tag_body.as_bytes(),
    );
    r.git(&["update-ref", "refs/tags/broken", &tag_sha]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let tagged = ir.refs.iter().find(|rf| rf.name == "broken").unwrap();
    let annotation = tagged.annotation.as_ref().expect("annotation is carried");
    let time = annotation.time.expect("seconds still parsed");
    assert_eq!(time.seconds, 1_112_911_993);
    assert_eq!(
        time.offset_minutes, None,
        "a malformed tagger offset is absent, never salvaged or defaulted"
    );
    assert!(
        ir.loss.dropped.iter().any(|d| d
            .what
            .starts_with("unparseable author/committer/tagger timezone offsets")),
        "{:?}",
        ir.loss.dropped
    );
}

#[test]
fn a_non_utf8_encoding_header_value_is_not_carried_and_is_counted() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.git(&["add", "-A"]);
    let tree = r.git(&["write-tree"]);
    let mut body = format!(
        "tree {tree}\n\
         author A U Thor <author@example.com> 1112911993 +0000\n\
         committer A U Thor <author@example.com> 1112911993 +0000\n\
         encoding "
    )
    .into_bytes();
    body.extend_from_slice(&[0xff, 0xfe]); // not valid UTF-8
    body.extend_from_slice(b"\n\nc1\n");
    let sha = git_stdin(
        r.path(),
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--literally",
            "--stdin",
        ],
        &body,
    );
    r.git(&["update-ref", "refs/heads/main", &sha]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let atom = &ir.atoms[0];
    assert_eq!(
        atom.metadata.message.as_ref().unwrap().encoding,
        None,
        "an undecodable encoding value is not carried"
    );
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "undecodable encoding headers (1)"),
        "{:?}",
        ir.loss.dropped
    );
}

#[test]
fn a_non_utf8_commit_header_name_is_refused() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("a.txt", "a\n");
    r.git(&["add", "-A"]);
    let tree = r.git(&["write-tree"]);
    let mut body = format!(
        "tree {tree}\n\
         author A U Thor <author@example.com> 1112911993 +0000\n\
         committer A U Thor <author@example.com> 1112911993 +0000\n"
    )
    .into_bytes();
    body.extend_from_slice(&[0xff, 0xfe]); // the header *name* itself is not valid UTF-8
    body.extend_from_slice(b" some-value\n\nc1\n");
    let sha = git_stdin(
        r.path(),
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--literally",
            "--stdin",
        ],
        &body,
    );
    r.git(&["update-ref", "refs/heads/main", &sha]);

    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, "non-utf8-commit-header-name");
        }
        other => panic!("expected a FloorRefusal, got {other:?}"),
    }
}

// ---- release-prep §4: no internal reference identifiers in user-facing artifact text -----------------

/// True if `text` contains something like `RFC 004`, `CR-16`, `PR-7`, `NG-5`, `INV-3` — an internal
/// requirement/RFC/review reference (`\b(RFC|OQ|CR|PR|INV|NG|SRC|FS|VF|HO|CL|CT|FA)[- ]?[0-9]`), hand-rolled.
/// A space between the prefix and the number is also matched, which the handoff's own pattern would miss.
fn has_internal_reference(text: &str) -> Option<String> {
    const PREFIXES: [&str; 13] = [
        "RFC", "OQ", "CR", "PR", "INV", "NG", "SRC", "FS", "VF", "HO", "CL", "CT", "FA",
    ];
    let bytes = text.as_bytes();
    for prefix in PREFIXES {
        let mut from = 0;
        while let Some(off) = text[from..].find(prefix) {
            let at = from + off;
            from = at + prefix.len();
            let before_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
            if !before_ok {
                continue;
            }
            let mut i = at + prefix.len();
            if i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'-') {
                i += 1;
            }
            if i < bytes.len() && bytes[i].is_ascii_digit() {
                return Some(text[at..(i + 1).min(text.len())].to_string());
            }
        }
    }
    None
}

#[test]
fn the_reference_matcher_finds_what_it_should() {
    assert!(has_internal_reference("see RFC 004 D-5").is_some());
    assert!(has_internal_reference("(CR-16)").is_some());
    assert!(has_internal_reference("(NG-5, RFC 011 D-5)").is_some());
    assert!(has_internal_reference("(RFC 011 D-4)").is_some());
    assert!(has_internal_reference("OQ-2").is_some());
    assert!(has_internal_reference("(FA-1)").is_some());
    assert!(has_internal_reference("per OQ-B").is_none()); // a letter, not a number, follows
    assert!(has_internal_reference("local state, not history").is_none());
    assert!(has_internal_reference("a CRC32 check").is_none());
}

fn assert_no_internal_references(ir: &brygge_ir::Ir, what: &str) {
    for d in &ir.loss.dropped {
        for text in [&d.what, &d.reason] {
            assert!(
                has_internal_reference(text).is_none(),
                "{what}: a drop's text carries an internal reference: {text:?}"
            );
        }
    }
    for f in &ir.flags {
        for text in [&f.what, &f.reason] {
            assert!(
                has_internal_reference(text).is_none(),
                "{what}: a flag's text carries an internal reference: {text:?}"
            );
        }
    }
}

#[test]
fn no_drop_or_flag_text_carries_an_internal_reference() {
    // Every record `loss_boundary` can produce, forced on at once (a fixture cannot reach all of them).
    let mut namespaces = std::collections::BTreeMap::new();
    namespaces.insert("refs/remotes", 2u64);
    let counts = super::ParseCounts {
        unparseable_times: 1,
        unparseable_offsets: 1,
        undecodable_encodings: 1,
    };
    for only in [
        super::DroppedOnlyCommits::Counted(3),
        super::DroppedOnlyCommits::Unavailable,
    ] {
        let lb = super::loss_boundary(&namespaces, only, 1, 1, counts, 1);
        assert!(
            lb.dropped.len() >= 9,
            "expected every record: {:?}",
            lb.dropped
        );
        for d in &lb.dropped {
            for text in [&d.what, &d.reason] {
                assert!(
                    has_internal_reference(text).is_none(),
                    "a drop's text carries an internal reference: {text:?}"
                );
            }
        }
    }

    // And what real decodes of this crate's fixtures actually record.
    if !git_available() {
        return;
    }
    let r = build_rich_repo();
    let ir = decode(r.path(), &crate::Options::default()).unwrap();
    assert_no_internal_references(&ir, "the rich repository");
    let empty = TempRepo::new();
    let ir = decode(empty.path(), &crate::Options::default()).unwrap();
    assert_no_internal_references(&ir, "an empty repository");
}

// ---- RFC 010 increment 5: the snapshot retention bound -----------------------------------------------------

fn tree(n: u8) -> ObjectId {
    let mut bytes = [0u8; 20];
    bytes[0] = n;
    ObjectId::from_bytes_or_panic(&bytes)
}

fn snap_with(path: &str) -> Snapshot {
    let mut s = Snapshot::new();
    s.insert(path.to_string(), (tree(200), 0o100_644));
    s
}

#[test]
fn a_snapshot_is_dropped_after_its_last_use_and_not_before() {
    let mut cache = SnapshotCache::new(HashMap::from([(tree(1), 2)]));
    let mut builds = 0;
    for _ in 0..2 {
        cache
            .get_or_build(tree(1), || {
                builds += 1;
                Ok(snap_with("a"))
            })
            .unwrap();
    }
    assert_eq!(builds, 1, "the second use is served from the cache");
    assert_eq!(cache.len(), 1);
    cache.release(tree(1));
    assert_eq!(cache.len(), 1, "one use is still to come");
    cache.release(tree(1));
    assert_eq!(cache.len(), 0, "the last use is done");
}

#[test]
fn a_tree_that_a_later_commit_reverts_to_stays_cached_until_that_commit() {
    // Trees A (used by commits 1 and 3 and as the base of 2 and 4), B (commit 2 and the base of 3).
    let (a, b) = (tree(1), tree(2));
    let mut cache = SnapshotCache::new(HashMap::from([(a, 4), (b, 2)]));
    let mut builds = 0;
    let mut get = |cache: &mut SnapshotCache, t: ObjectId| {
        cache
            .get_or_build(t, || {
                builds += 1;
                Ok(snap_with("a"))
            })
            .unwrap();
    };
    // commit 1: own A. commit 2: own B, base A. commit 3: own A, base B. commit 4: own A, base A.
    get(&mut cache, a);
    cache.release(a);
    get(&mut cache, b);
    get(&mut cache, a);
    cache.release(b);
    cache.release(a);
    assert_eq!(
        cache.len(),
        2,
        "A is used again by commits 3 and 4, B by commit 3"
    );
    get(&mut cache, a);
    get(&mut cache, b);
    cache.release(a);
    cache.release(b);
    assert_eq!(
        cache.len(),
        1,
        "B is finished, A is used once more (commit 4)"
    );
    get(&mut cache, a);
    get(&mut cache, a);
    cache.release(a);
    cache.release(a);
    assert_eq!(cache.len(), 0);
    assert_eq!(
        builds, 2,
        "each tree was built exactly once: nothing was evicted early"
    );
}

#[test]
fn a_linear_history_holds_at_most_two_snapshots_however_long() {
    // Commit i has tree i and first parent i-1: each tree is used by commit i and by commit i+1.
    let n = 1_000u16;
    let id = |i: u16| {
        let mut bytes = [0u8; 20];
        bytes[..2].copy_from_slice(&i.to_be_bytes());
        ObjectId::from_bytes_or_panic(&bytes)
    };
    let mut uses = HashMap::new();
    for i in 0..n {
        *uses.entry(id(i)).or_insert(0) += 1;
        if i > 0 {
            *uses.entry(id(i - 1)).or_insert(0) += 1;
        }
    }
    let mut cache = SnapshotCache::new(uses);
    let mut most = 0;
    for i in 0..n {
        cache.get_or_build(id(i), || Ok(snap_with("a"))).unwrap();
        if i > 0 {
            cache
                .get_or_build(id(i - 1), || Ok(snap_with("a")))
                .unwrap();
        }
        most = most.max(cache.len());
        cache.release(id(i));
        if i > 0 {
            cache.release(id(i - 1));
        }
    }
    assert!(most <= 2, "held {most} snapshots at once over {n} commits");
    assert_eq!(cache.len(), 0);
}

#[test]
fn a_tree_without_a_counted_use_is_never_evicted() {
    // A commit that could not be read is not counted; its trees must stay (the loop reports the failure).
    let mut cache = SnapshotCache::new(HashMap::new());
    cache.get_or_build(tree(1), || Ok(snap_with("a"))).unwrap();
    cache.release(tree(1));
    assert_eq!(cache.len(), 1);
}

/// c1 and c3 have the very same root tree (c3 reverts c2), and the snapshot of that tree is needed again by
/// c3's own diff and by c4's base after c2 was processed. The diffs must be exactly the revert.
#[test]
fn a_revert_back_to_an_older_tree_is_diffed_correctly() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let r = TempRepo::new();
    r.write("f.txt", "one\n");
    r.commit_all("c1");
    r.write("f.txt", "two\n");
    r.commit_all("c2");
    r.write("f.txt", "one\n"); // back to c1's tree
    r.commit_all("c3 revert");
    r.write("g.txt", "later\n");
    r.commit_all("c4");
    // An empty commit: its tree equals its parent's (two uses of one tree by one commit).
    r.git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "c5 empty",
    ]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    assert_eq!(ir.atoms.len(), 5);
    let content = |op: &PathOp| match op {
        PathOp::Add { blob, .. } | PathOp::Modify { blob, .. } => {
            ir.content.get(blob).map(<[u8]>::to_vec)
        }
        _ => None,
    };
    let ops: Vec<(String, Vec<u8>)> = ir
        .atoms
        .iter()
        .flat_map(|a| a.ops.iter())
        .map(|op| match op {
            PathOp::Add { path, .. } | PathOp::Modify { path, .. } => {
                (path.clone(), content(op).expect("content"))
            }
            other => panic!("unexpected op {other:?}"),
        })
        .collect();
    assert_eq!(
        ops,
        vec![
            ("f.txt".to_string(), b"one\n".to_vec()),
            ("f.txt".to_string(), b"two\n".to_vec()),
            ("f.txt".to_string(), b"one\n".to_vec()), // c3: the revert, a Modify back to c1's content
            ("g.txt".to_string(), b"later\n".to_vec()),
        ],
        "c1 add, c2 modify, c3 revert, c4 add; the empty c5 has no ops"
    );
    assert!(
        ir.atoms[4].ops.is_empty(),
        "an empty commit is an atom with no ops"
    );
    // Decoding twice gives identical bytes (the bookkeeping cannot change the output).
    let again = decode(r.path(), &Options::default()).unwrap();
    assert_eq!(brygge_ir::to_bytes(&ir), brygge_ir::to_bytes(&again));
}

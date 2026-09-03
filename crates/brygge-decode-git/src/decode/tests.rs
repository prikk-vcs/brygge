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
    assert!(
        ir.loss
            .dropped
            .iter()
            .all(|d| matches!(d.class, brygge_ir::LossClass::Representation))
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

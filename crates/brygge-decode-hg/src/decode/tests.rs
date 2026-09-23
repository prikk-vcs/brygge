//! Integration tests for the Mercurial decoder (RFC 005): decode a real hg repo into an IR and assert
//! the M2 properties — **source-recorded renames are `Stated`**, determinism, the IR contract holds a
//! second source (round-trip), refs, content fidelity, and the subrepo floor. Skips without `hg`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::decode;
use crate::Options;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn hg_available() -> bool {
    Command::new("hg")
        .arg("--version")
        .env("HGRCPATH", "/dev/null")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

struct Repo {
    dir: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Repo {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("brygge-hgdec-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Self { dir };
        r.run(&["init", r.dir.to_str().unwrap()]);
        r
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn try_run(&self, args: &[&str]) -> bool {
        Command::new("hg")
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
            .unwrap_or(false)
    }

    fn run(&self, args: &[&str]) {
        assert!(self.try_run(args), "hg {args:?} failed");
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    fn commit(&self, epoch: &str, msg: &str) {
        self.run(&["add"]);
        self.run(&["commit", "-d", &format!("{epoch} 0"), "-m", msg]);
    }
}

/// c0 initial; c1 modify; c2 `hg mv` (stated rename); c3 on a named branch; a bookmark on the tip.
fn build_repo() -> Repo {
    let r = Repo::new();
    r.write("readme.txt", "hello\n");
    r.write("keep.txt", "keep\n");
    r.commit("1136239445", "initial");

    r.write("readme.txt", "hello world\n");
    r.commit("1136239446", "expand readme");

    r.run(&["mv", "keep.txt", "kept.txt"]);
    r.commit("1136239447", "rename keep to kept");

    r.run(&["branch", "feature"]);
    r.write("feature.txt", "f\n");
    r.commit("1136239448", "start feature");

    r.run(&["bookmark", "bm"]);
    r
}

#[test]
fn decodes_history_with_a_stated_rename() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let ir = decode(r.path(), &Options::default()).unwrap();

    assert_eq!(ir.atoms.len(), 4, "four changesets");

    // The point of M2: a source-recorded rename is Stated, not derived -> zero derived marks.
    let report = brygge_ir::honesty::summary(&ir);
    assert!(
        report.derived.is_empty(),
        "hg renames are Stated, not derived"
    );

    // Find the rename: a Stated hint keep.txt -> kept.txt, beside the literal Delete+Add.
    let rename_atom = ir
        .atoms
        .iter()
        .find(|a| !a.rename_hints.is_empty())
        .expect("the mv commit carries a rename hint");
    let hint = &rename_atom.rename_hints[0];
    assert_eq!(hint.from, "keep.txt");
    assert_eq!(hint.to, "kept.txt");
    assert!(
        !hint.status.is_derived(),
        "a source-recorded rename is Stated (SRC-H2)"
    );
    assert!(
        rename_atom
            .ops
            .iter()
            .any(|op| matches!(op, brygge_ir::PathOp::Delete { path, .. } if path == "keep.txt"))
    );
    assert!(
        rename_atom
            .ops
            .iter()
            .any(|op| matches!(op, brygge_ir::PathOp::Add { path, .. } if path == "kept.txt"))
    );

    // Metadata is carried as claims.
    let root = &ir.atoms[0];
    assert_eq!(
        root.metadata.author.as_ref().unwrap().email,
        "a@example.com"
    );
    assert!(
        root.metadata
            .message
            .as_deref()
            .unwrap_or("")
            .contains("initial")
    );
    assert!(root.parents.is_empty(), "the first changeset is a root");

    // Refs: the bookmark, and the named branches (branch-head semantics -> default and feature tips).
    assert!(
        ir.refs
            .iter()
            .any(|rf| rf.name == "bm" && matches!(rf.kind, brygge_ir::RefKind::Bookmark))
    );
    assert!(
        ir.refs
            .iter()
            .any(|rf| rf.name == "feature" && matches!(rf.kind, brygge_ir::RefKind::NamedBranch))
    );
    assert!(
        ir.refs
            .iter()
            .any(|rf| rf.name == "default" && matches!(rf.kind, brygge_ir::RefKind::NamedBranch))
    );
}

#[test]
fn content_is_faithful_and_the_loss_boundary_is_stated() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(
        ir.content
            .contains(&brygge_ir::BlobId::of(b"hello world\n")),
        "the modified readme content is present in the store"
    );
    // Representation drops are recorded; nothing silently omitted.
    let whats: Vec<&str> = ir.loss.dropped.iter().map(|d| d.what.as_str()).collect();
    assert!(whats.iter().any(|w| w.contains("revlog")));
    assert!(whats.iter().any(|w| w.contains("phases")));
}

#[test]
fn decode_is_deterministic_and_the_ir_contract_holds_a_second_source() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let a = brygge_ir::to_bytes(&decode(r.path(), &Options::default()).unwrap());
    let b = brygge_ir::to_bytes(&decode(r.path(), &Options::default()).unwrap());
    assert_eq!(
        a, b,
        "two decodes of the same repo are byte-identical (VF-1)"
    );

    // D-8: the current IR contract holds Mercurial with no change -> a full round-trip (the freeze basis).
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert_eq!(ir.contract_version, brygge_ir::version::CURRENT);
    let round = brygge_ir::from_bytes(&brygge_ir::to_bytes(&ir)).unwrap();
    assert_eq!(
        round, ir,
        "the artifact round-trips under the current IR contract"
    );
}

#[test]
fn a_subrepo_is_refused() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    // A nested subrepo; committing .hgsub is config-gated across hg versions, so this is best-effort.
    let sub = r.dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let sub_ok = Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .args(["init", sub.to_str().unwrap()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !sub_ok {
        eprintln!("skipping: could not init nested subrepo");
        return;
    }
    std::fs::write(sub.join("s.txt"), b"s\n").unwrap();
    let _ = Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .args([
            "-R",
            sub.to_str().unwrap(),
            "--config",
            "ui.username=A <a@e.com>",
            "commit",
            "-A",
            "-d",
            "1 0",
            "-m",
            "sub",
        ])
        .output();
    r.write(".hgsub", "sub = sub\n");
    if !r.try_run(&["add", ".hgsub"])
        || !r.try_run(&[
            "--config",
            "subrepos.allowed=true",
            "--config",
            "subrepos.hg:allowed=true",
            "commit",
            "-d",
            "2 0",
            "-m",
            "add subrepo",
        ])
    {
        eprintln!("skipping: this hg refused to commit a subrepo");
        return;
    }
    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => assert_eq!(feature, "subrepo"),
        other => panic!("expected a subrepo floor refusal, got {other:?}"),
    }
}

// ---- RFC 010 CR-15: the format gate and repo_id ----------------------------------------------------

#[cfg(unix)]
#[test]
fn an_unreadable_requires_file_is_an_open_error_not_silently_ignored() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    use std::os::unix::fs::PermissionsExt;

    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1136239445", "c0");

    let requires_path = r.path().join(".hg").join("requires");
    let original = std::fs::metadata(&requires_path).unwrap().permissions();
    std::fs::set_permissions(&requires_path, std::fs::Permissions::from_mode(0o000)).unwrap();

    if std::fs::read_to_string(&requires_path).is_ok() {
        // The current user ignores Unix permission bits (e.g. running as root): the premise doesn't
        // hold here, so the test cannot exercise the "unreadable" case.
        std::fs::set_permissions(&requires_path, original).unwrap();
        eprintln!("skipping: the current user can read a mode-000 file (likely running as root)");
        return;
    }

    let result = decode(r.path(), &Options::default());
    std::fs::set_permissions(&requires_path, original).unwrap();
    match result {
        Err(crate::Error::Open(_)) => {}
        other => panic!("expected Error::Open (not a silently-empty format gate), got {other:?}"),
    }
}

#[test]
fn repo_id_is_independent_of_pull_order_across_multiple_roots() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let a = Repo::new();
    a.write("a.txt", "a\n");
    a.commit("1136239445", "root a");

    let b = Repo::new();
    b.write("b.txt", "b\n");
    b.commit("1136239450", "root b");

    // Two repositories, each pulling both unrelated histories in a different order — a real store
    // with more than one root, numbered oppositely by local pull order.
    let x = Repo::new();
    if !x.try_run(&["pull", "--force", a.path().to_str().unwrap()])
        || !x.try_run(&["pull", "--force", b.path().to_str().unwrap()])
    {
        eprintln!("skipping: this hg would not pull unrelated histories with --force");
        return;
    }
    let y = Repo::new();
    assert!(y.try_run(&["pull", "--force", b.path().to_str().unwrap()]));
    assert!(y.try_run(&["pull", "--force", a.path().to_str().unwrap()]));

    let ir_x = decode(x.path(), &Options::default()).expect("decode x");
    let ir_y = decode(y.path(), &Options::default()).expect("decode y");
    assert_eq!(ir_x.atoms.len(), 2, "both roots are carried in x");
    assert_eq!(ir_y.atoms.len(), 2, "both roots are carried in y");
    assert_eq!(
        ir_x.provenance.source.repo_id, ir_y.provenance.source.repo_id,
        "repo_id (the smallest root node) must not depend on local pull/revision order"
    );
}

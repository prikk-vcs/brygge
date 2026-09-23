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

    fn changelog(&self) -> crate::revlog::Revlog {
        crate::revlog::Revlog::open(&self.dir.join(".hg").join("store").join("00changelog.i"))
            .unwrap()
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

    // Find the rename: a Stated copy record keep.txt -> kept.txt, beside the literal Delete+Add.
    let rename_atom = ir
        .atoms
        .iter()
        .find(|a| !a.copies.is_empty())
        .expect("the mv commit carries a copy record");
    let copy = &rename_atom.copies[0];
    assert_eq!(copy.from, "keep.txt");
    assert_eq!(copy.to, "kept.txt");
    assert!(
        !copy.status.is_derived(),
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
        root.metadata
            .author
            .as_ref()
            .unwrap()
            .email
            .as_ref()
            .and_then(brygge_ir::Text::as_utf8),
        Some("a@example.com")
    );
    assert!(
        root.metadata
            .message
            .as_ref()
            .and_then(brygge_ir::Text::as_utf8)
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

    // D-8: the current IR contract holds Mercurial with no change -> a full round-trip.
    let ir = decode(r.path(), &Options::default()).unwrap();
    let round = brygge_ir::from_bytes(&brygge_ir::to_bytes(&ir)).unwrap();
    assert_eq!(
        round.ir, ir,
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

// --- review 012 F-1: the repository identity is the smallest root among the *published* changesets ---

fn root_node(r: &Repo, rev: &str) -> Vec<u8> {
    let out = Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .arg("-R")
        .arg(r.path())
        .args(["log", "-r", rev, "-T", "{node}"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let hex = String::from_utf8(out.stdout).unwrap();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// Two unrelated roots pulled into one repository, the one with the *smaller* node made secret. Returns
/// the repository together with the (smaller, larger) root nodes; `None` if this hg cannot pull unrelated
/// histories.
fn repo_with_a_secret_smaller_root() -> Option<(Repo, Vec<u8>, Vec<u8>)> {
    let a = Repo::new();
    a.write("a.txt", "a\n");
    a.commit("1136239445", "root a");
    let b = Repo::new();
    b.write("b.txt", "b\n");
    b.commit("1136239450", "root b");
    let (na, nb) = (root_node(&a, "0"), root_node(&b, "0"));
    let (small, large) = if na < nb { (na, nb) } else { (nb, na) };
    assert!(small < large, "the ordering precondition must hold");

    let x = Repo::new();
    if !x.try_run(&["pull", "--force", a.path().to_str().unwrap()])
        || !x.try_run(&["pull", "--force", b.path().to_str().unwrap()])
    {
        return None;
    }
    let small_hex = crate::util::hex(&small);
    x.run(&["phase", "--secret", "--force", "-r", &small_hex]);
    Some((x, small, large))
}

#[test]
fn a_secret_smallest_root_does_not_decide_the_repo_id() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let Some((x, small, large)) = repo_with_a_secret_smaller_root() else {
        eprintln!("skipping: this hg would not pull unrelated histories with --force");
        return;
    };
    let ir = decode(x.path(), &Options::default()).expect("decode");
    assert_eq!(ir.atoms.len(), 1, "only the published root is carried");
    assert_ne!(
        ir.provenance.source.repo_id, small,
        "the secret root decided the identity"
    );
    assert_eq!(ir.provenance.source.repo_id, large);
}

#[test]
fn the_repo_id_equals_that_of_an_hg_clone_of_the_repository() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let Some((x, _small, large)) = repo_with_a_secret_smaller_root() else {
        eprintln!("skipping: this hg would not pull unrelated histories with --force");
        return;
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("brygge-hgclone-{}-{nanos}-{n}", std::process::id()));
    let cloned = Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .args(["clone", "-q"])
        .arg(x.path())
        .arg(&dir)
        .status()
        .unwrap()
        .success();
    let z = Repo { dir };
    assert!(cloned, "hg clone failed");
    let ir_x = decode(x.path(), &Options::default()).unwrap();
    let ir_z = decode(z.path(), &Options::default()).unwrap();
    assert_eq!(ir_z.atoms.len(), 1, "the clone excludes the secret root");
    assert_eq!(
        ir_x.provenance.source.repo_id,
        ir_z.provenance.source.repo_id
    );
    assert_eq!(ir_z.provenance.source.repo_id, large);
}

// --- CR-12.2: the floor is one declared list, recorded in provenance --------------------------------

#[test]
fn provenance_floor_param_equals_the_declared_list() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1136239445", "c1");
    let ir = decode(r.path(), &Options::default()).unwrap();
    let expected = crate::floor::joined();
    assert_eq!(ir.provenance.params.get("floor"), Some(&expected));
}

// ---- RFC 005 corrections handoff §3: claims and extras ---------------------------------------------

#[test]
fn committer_and_commit_time_are_absent_and_the_timezone_converts_correctly() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    // hg's `-d` takes "<epoch-seconds> <tz-seconds-west-of-utc>"; +0900 is 9 hours *east*, so west-of-
    // UTC is -32400.
    r.run(&["add"]);
    r.run(&["commit", "-d", "1136239445 -32400", "-m", "c0"]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let atom = &ir.atoms[0];
    assert!(
        atom.metadata.committer.is_none(),
        "one-claim rule: no committer"
    );
    assert!(
        atom.metadata.commit_time.is_none(),
        "one-claim rule: no commit_time"
    );
    assert_eq!(
        atom.metadata.author_time.and_then(|t| t.offset_minutes),
        Some(540),
        "tz -32400 (west) -> +0900 (east) -> 540 minutes"
    );
}

#[test]
fn a_closed_branch_carries_the_close_extra() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.run(&["branch", "feature"]);
    r.write("b.txt", "b\n");
    r.commit("2", "c1");
    r.run(&["commit", "--close-branch", "-d", "3 0", "-m", "closing"]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let closer = ir
        .atoms
        .iter()
        .find(|a| a.source.extras.iter().any(|e| e.label == "close"))
        .expect("the closing commit carries a close extra");
    let close = closer
        .source
        .extras
        .iter()
        .find(|e| e.label == "close")
        .unwrap();
    assert_eq!(close.bytes, b"1");
    // `branch` itself is never duplicated as an Extra (it has its own home).
    assert!(!closer.source.extras.iter().any(|e| e.label == "branch"));
}

// ---- review 009 R-4/R-3/R-2 ------------------------------------------------------------------------

#[test]
fn an_unresolved_merge_is_refused() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.write("a.txt", "on-a\n");
    r.commit("2", "on-a");
    r.run(&["update", "0"]);
    r.write("a.txt", "on-b\n");
    r.commit("3", "on-b");
    // A merge with a real conflict, left unresolved (no --tool, no commit).
    let _ = std::process::Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .arg("--cwd")
        .arg(r.path())
        .args(["merge"])
        .output();
    assert!(
        r.path().join(".hg").join("merge").join("state2").exists(),
        "the fixture must actually leave an unresolved merge in progress"
    );
    match decode(r.path(), &Options::default()) {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, crate::floor::UNFINISHED_MERGE);
        }
        other => panic!("expected FloorRefusal(unfinished merge), got {other:?}"),
    }
}

#[test]
fn an_unrepresentable_timezone_offset_is_counted_not_silently_dropped() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.run(&["add"]);
    // tz = -30 seconds west of UTC: does not divide evenly into whole minutes.
    r.run(&["commit", "-d", "0 -30", "-m", "c0"]);
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(
        ir.atoms[0]
            .metadata
            .author_time
            .unwrap()
            .offset_minutes
            .is_none()
    );
    let recorded = ir
        .loss
        .dropped
        .iter()
        .any(|d| d.what.contains("unrepresentable timezone offsets"));
    assert!(recorded, "loss boundary: {:?}", ir.loss.dropped);
}

// ---- RFC 011 D-6 / RFC 005 corrections handoff §2: copy source resolution --------------------------

#[test]
fn a_copy_from_the_p2_side_of_a_merge_resolves_to_p2() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("base.txt", "base\n");
    r.commit("1", "c0");
    // Branch A: unrelated change.
    r.write("a.txt", "a\n");
    r.commit("2", "on-a");
    // Branch B: from c0, adds `src.txt`.
    r.run(&["update", "0"]);
    r.write("src.txt", "src\n");
    r.commit("3", "on-b");
    // hg's merge parent order is: p1 = whatever is currently checked out, p2 = the branch merged in.
    // Check out A (which does *not* have src.txt) first, so the merge's p1 lacks the file and only p2
    // (B, merged in) has it — the case this test exists to exercise.
    r.run(&["update", "1"]);
    r.run(&["merge", "--tool", ":other"]);
    r.run(&["copy", "src.txt", "copied.txt"]);
    r.run(&["commit", "-d", "4 0", "-m", "merge and copy from p2"]);

    let ir = decode(r.path(), &Options::default()).unwrap();
    let merge_atom = ir
        .atoms
        .iter()
        .find(|a| a.parents.len() == 2)
        .expect("a merge atom with two parents");
    assert_eq!(merge_atom.copies.len(), 1);
    let copy = &merge_atom.copies[0];
    assert_eq!(copy.from, "src.txt");
    assert_eq!(copy.to, "copied.txt");
    assert_eq!(
        copy.from_atom, merge_atom.parents[1],
        "src.txt only exists on the p2 (B) side, so from_atom must be p2, not p1"
    );
}

#[test]
fn decode_is_deterministic_with_secret_and_obsolete_changesets_present() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    std::fs::write(
        r.path().join(".hg").join("hgrc"),
        "[experimental]\nevolution = all\nevolution.createmarkers = true\n",
    )
    .unwrap();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.write("b.txt", "b\n");
    r.commit("2", "c1");
    r.run(&["phase", "--secret", "--force", "-r", "1"]);
    r.write("c.txt", "c\n");
    r.commit("3", "c2");
    r.run(&["commit", "--amend", "-d", "4 0", "-m", "c2 amended"]);

    let a = brygge_ir::to_bytes(&decode(r.path(), &Options::default()).unwrap());
    let b = brygge_ir::to_bytes(&decode(r.path(), &Options::default()).unwrap());
    assert_eq!(
        a, b,
        "deterministic even with secret and obsolete changesets present"
    );
}

// ---- RFC 005 corrections handoff §5.5: parity with `hg log -r 'not secret()'` (the acceptance oracle) --

fn hg_log_not_secret_nodes(r: &Repo) -> std::collections::BTreeSet<String> {
    let out = Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .arg("--cwd")
        .arg(r.path())
        .arg("-R")
        .arg(r.path())
        .args(["log", "-r", "not secret()", "-T", "{node}\n"])
        .output()
        .expect("hg log");
    assert!(out.status.success(), "hg log failed: {out:?}");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn brygge_imported_nodes(ir: &brygge_ir::Ir) -> std::collections::BTreeSet<String> {
    ir.atoms
        .iter()
        .map(|a| crate::util::hex(&a.source.atom_id))
        .collect()
}

/// The second oracle (review 009 R-7): the true transferred set, via an actual `hg clone --pull` of the
/// fixture followed by `hg log` on the clone — not `hg log`'s own view filter, but what Mercurial's own
/// wire protocol actually hands over.
fn hg_clone_served_nodes(r: &Repo) -> std::collections::BTreeSet<String> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dst =
        std::env::temp_dir().join(format!("brygge-hg-parity-clone-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dst);
    let ok = Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .args(["clone", "--pull"])
        .arg(r.path())
        .arg(&dst)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(ok, "hg clone --pull failed");
    let out = Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .arg("-R")
        .arg(&dst)
        .args(["log", "-T", "{node}\n"])
        .output()
        .expect("hg log on the clone");
    assert!(out.status.success(), "hg log on the clone failed: {out:?}");
    let nodes = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let _ = std::fs::remove_dir_all(&dst);
    nodes
}

/// Assert brygge's imported node set matches **both** oracles: `hg log -r 'not secret()'` (the
/// view-filter oracle) and a real `hg clone --pull` (the transfer oracle) — review 009 R-7.
fn assert_parity(r: &Repo) {
    let ir = decode(r.path(), &Options::default()).unwrap();
    let ours = brygge_imported_nodes(&ir);
    let by_log = hg_log_not_secret_nodes(r);
    let by_clone = hg_clone_served_nodes(r);
    assert_eq!(
        ours, by_log,
        "brygge's imported node set must equal `hg log -r 'not secret()'`"
    );
    assert_eq!(
        ours, by_clone,
        "brygge's imported node set must equal what `hg clone --pull` actually transfers"
    );
}

fn with_evolution(r: &Repo) {
    std::fs::write(
        r.path().join(".hg").join("hgrc"),
        "[experimental]\nevolution = all\nevolution.createmarkers = true\n",
    )
    .unwrap();
}

#[test]
fn parity_secret_fixture() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.write("b.txt", "b\n");
    r.commit("2", "c1");
    r.run(&["phase", "--secret", "--force", "-r", "1"]);
    assert_parity(&r);
}

#[test]
fn parity_hidden_fixture_no_secret_ancestor() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    with_evolution(&r);
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.write("a.txt", "a a\n");
    r.run(&["commit", "--amend", "-d", "2 0", "-m", "c0 amended"]);
    assert_parity(&r);
    // review 009 handoff §5.2: no extra named-branch head from the hidden precursor.
    let ir = decode(r.path(), &Options::default()).unwrap();
    let branch_heads = ir
        .refs
        .iter()
        .filter(|rf| matches!(rf.kind, brygge_ir::model::RefKind::NamedBranch))
        .count();
    assert_eq!(branch_heads, 1, "only the amended tip is a branch head");
}

#[test]
fn parity_orphan_fixture_no_secret_ancestor() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    with_evolution(&r);
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.write("b.txt", "b\n");
    r.commit("2", "c1"); // depends on c0
    r.run(&["update", "0"]);
    r.write("a.txt", "a a\n");
    r.run(&["commit", "--amend", "-d", "3 0", "-m", "c0 amended"]);
    assert_parity(&r);
}

#[test]
fn parity_pinned_by_bookmark_fixture_no_secret_ancestor() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    with_evolution(&r);
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.run(&["bookmark", "keep-me", "-r", "0"]);
    r.write("a.txt", "a a\n");
    let precursor_node = {
        let changelog = r.changelog();
        changelog.entry(0).unwrap().node
    };
    let precursor_hex = crate::util::hex(&precursor_node);
    r.run(&["commit", "--amend", "-d", "2 0", "-m", "c0 amended"]);
    r.run(&[
        "bookmark",
        "--hidden",
        "-r",
        &precursor_hex,
        "-f",
        "keep-me",
    ]);
    assert_parity(&r);
}

#[test]
fn parity_hgtags_does_not_pin_fixture() {
    // review 009 R-4: `.hgtags` does NOT pin — a tagged-then-obsoleted changeset stays hidden, matching
    // both oracles (which is the opposite of what the original handoff specified).
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    with_evolution(&r);
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.run(&["tag", "v1", "-d", "2 0"]); // tags c0, adds a .hgtags commit
    r.write("a.txt", "a a\n");
    r.run(&["update", "0"]);
    r.run(&["commit", "--amend", "-d", "3 0", "-m", "c0 amended"]);
    assert_parity(&r);
}

// ---- review 009 §3.1: the omit-and-count path for an unresolvable copy (unit-level; Mercurial never
// writes a store that reaches it, so no live fixture exists) -------------------------------------------

#[test]
fn an_unresolvable_copy_is_omitted_and_counted_never_placed_on_a_guess() {
    use brygge_ir::AtomId;

    let mut copies = Vec::new();
    let mut unresolved = 0usize;
    super::place_copy(None, "a", "b", &mut copies, &mut unresolved);
    assert!(
        copies.is_empty(),
        "no CopyRecord for an unresolvable source"
    );
    assert_eq!(unresolved, 1);

    // A resolved copy is carried, Stated, on exactly the atom it was resolved to.
    let atom = AtomId([7u8; 32]);
    super::place_copy(Some(atom), "a", "c", &mut copies, &mut unresolved);
    assert_eq!(unresolved, 1);
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].from_atom, atom);
    assert_eq!((copies[0].from.as_str(), copies[0].to.as_str()), ("a", "c"));
}

#[test]
fn the_unresolvable_copy_record_has_the_exact_text_and_class() {
    let store = std::env::temp_dir().join("brygge-hg-no-such-store");
    let lb = super::loss_boundary(&store, 0, 0, 1, 0, 0, 0);
    let d = lb
        .dropped
        .iter()
        .find(|d| d.what == "copy sources not resolvable (1)")
        .expect("the record is present, with the count in `what`");
    assert_eq!(d.class, brygge_ir::LossClass::Other);
    assert_eq!(
        d.reason,
        "the stated copy source could not be placed on an imported changeset; the copy is omitted \
         rather than placed on a guess"
    );
    // ... and is absent when there is nothing to count.
    let none = super::loss_boundary(&store, 0, 0, 0, 0, 0, 0);
    assert!(
        !none
            .dropped
            .iter()
            .any(|d| d.what.starts_with("copy sources"))
    );
}

// ---- review 009 F-4: the copy-source decision, one case per step, driven through the `CopyEnv` seam over a
// small in-memory graph (no store, no `hg`). Steps 3-4 serve stores written by hg < 3.3, which current
// Mercurial cannot produce, so this is their only test. -------------------------------------------------

mod copy_decision {
    use std::collections::HashSet;

    use brygge_ir::AtomId;

    use super::super::{CopyEnv, Linkrev, decide_copy_source};

    /// A linear-or-forked changeset graph: `parents[rev] = first parent`, plus the per-revision facts the
    /// decision consults. Atoms are `AtomId([rev; 32])`, so an assertion names the revision it expects.
    struct Graph {
        first_parent: Vec<Option<usize>>,
        /// Revisions whose manifest maps `from` to `copyrev`.
        maps: HashSet<usize>,
        served: Vec<bool>,
        linkrev: Linkrev,
        /// Ancestor-or-self relation for step 3, as `(target, of)` pairs.
        ancestors: HashSet<(usize, usize)>,
        /// Every `manifest_maps` call, to prove a walk stopped or never started.
        probes: std::cell::RefCell<Vec<usize>>,
    }

    fn atom(rev: usize) -> AtomId {
        AtomId([u8::try_from(rev).unwrap(); 32])
    }

    impl CopyEnv for Graph {
        fn copyrev_linkrev(&self) -> Linkrev {
            self.linkrev
        }
        fn manifest_maps(&self, rev: usize) -> Result<bool, crate::Error> {
            self.probes.borrow_mut().push(rev);
            Ok(self.maps.contains(&rev))
        }
        fn atom_of(&self, rev: usize) -> Option<AtomId> {
            Some(atom(rev))
        }
        fn first_parent(&self, rev: usize) -> Option<usize> {
            self.first_parent.get(rev).copied().flatten()
        }
        fn served(&self, rev: usize) -> bool {
            self.served.get(rev).copied().unwrap_or(false)
        }
        fn is_ancestor(&self, target: usize, starts: &[usize]) -> bool {
            starts
                .iter()
                .any(|s| self.ancestors.contains(&(target, *s)))
        }
    }

    /// 0 <- 1 <- 2 <- 3 (linear), all served, nothing maps yet; a linkrev is set per case.
    fn linear(linkrev: Linkrev) -> Graph {
        Graph {
            first_parent: vec![None, Some(0), Some(1), Some(2)],
            maps: HashSet::new(),
            served: vec![true; 4],
            linkrev,
            ancestors: HashSet::new(),
            probes: std::cell::RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn step_1_p1_wins_even_when_p2_and_the_linkrev_would_also_do() {
        let mut g = linear(Linkrev::Known(0));
        g.maps.extend([2, 1]); // p2's manifest maps it too
        g.ancestors.insert((0, 2));
        // current atom = 3 with p1 = 2, p2 = 1.
        assert_eq!(
            decide_copy_source(&g, Some(2), Some(1), true).unwrap(),
            Some(atom(2))
        );
        assert!(
            g.probes.borrow().is_empty(),
            "p1 must win before anything else is probed"
        );
    }

    #[test]
    fn step_2_p2_when_p1_does_not_map_it() {
        let mut g = linear(Linkrev::Known(0));
        g.maps.insert(1);
        assert_eq!(
            decide_copy_source(&g, Some(2), Some(1), false).unwrap(),
            Some(atom(1))
        );
    }

    #[test]
    fn step_3_the_linkrev_only_when_served_and_an_ancestor() {
        // Neither parent maps it; linkrev 0 is served and an ancestor of p1 (= 2).
        let mut g = linear(Linkrev::Known(0));
        g.ancestors.insert((0, 2));
        assert_eq!(
            decide_copy_source(&g, Some(2), None, false).unwrap(),
            Some(atom(0))
        );

        // Not an ancestor: step 3 declines, and step 4's walk (which never finds it) ends in `None`.
        let g = linear(Linkrev::Known(0));
        assert_eq!(decide_copy_source(&g, Some(2), None, false).unwrap(), None);

        // Not served: step 3 declines even though it is an ancestor.
        let mut g = linear(Linkrev::Known(0));
        g.ancestors.insert((0, 2));
        g.served[0] = false;
        assert_eq!(decide_copy_source(&g, Some(2), None, false).unwrap(), None);
    }

    #[test]
    fn step_4_the_first_parent_walk_finds_the_nearest_changeset_that_maps_it() {
        // The linkrev (0) points elsewhere (not an ancestor here), but rev 1 on the first-parent
        // chain maps `from` to `copyrev`: found by the walk, nearest first, starting past p1 (= 3).
        let mut g = linear(Linkrev::Known(0));
        g.maps.insert(1);
        assert_eq!(
            decide_copy_source(&g, Some(3), None, false).unwrap(),
            Some(atom(1))
        );
        // Walked 2 (no), then 1 (yes); never 0.
        assert_eq!(*g.probes.borrow(), vec![2, 1]);
    }

    #[test]
    fn step_4_stops_below_the_linkrev() {
        // linkrev = 2: no changeset older than 2 can contain the filenode, so 1 and 0 are never probed
        // even though rev 0 would (wrongly) map it.
        let mut g = linear(Linkrev::Known(2));
        g.maps.insert(0);
        assert_eq!(decide_copy_source(&g, Some(3), None, false).unwrap(), None);
        assert_eq!(
            *g.probes.borrow(),
            vec![2],
            "the walk must stop once rev < linkrev"
        );
    }

    #[test]
    fn nothing_found_is_none() {
        let g = linear(Linkrev::Unknown);
        assert_eq!(decide_copy_source(&g, Some(3), None, false).unwrap(), None);
        // With no usable linkrev the walk is unbounded below: it probed the whole chain.
        assert_eq!(*g.probes.borrow(), vec![2, 1, 0]);
    }

    #[test]
    fn a_copyrev_absent_from_the_filelog_returns_none_at_once() {
        let mut g = linear(Linkrev::Absent);
        g.maps.extend([0, 1, 2]); // even if every manifest "maps" it, nothing is consulted
        assert_eq!(
            decide_copy_source(&g, Some(3), Some(2), true).unwrap(),
            None
        );
        assert!(g.probes.borrow().is_empty(), "no walk, no probes");
    }
}

// ---- release-prep §1, end to end: a store whose text was altered without updating its node -------------

/// Rewrite a single-revision revlog (inline, or split into `.i` + `.d`) in place: decode its one chunk, let
/// `mutate` alter the text, and store the result as a raw (`u`) chunk with the index's lengths updated —
/// leaving the node exactly as it was. Returns the revision's (unchanged) node.
fn alter_single_revision(index_path: &Path, mutate: impl FnOnce(&mut Vec<u8>)) -> [u8; 20] {
    use std::io::Read as _;
    let index = std::fs::read(index_path).unwrap();
    let inline = u32::from_be_bytes(index[..4].try_into().unwrap()) & (1 << 16) != 0;
    let data_path = index_path.with_extension("d");
    let comp_len = u32::from_be_bytes(index[8..12].try_into().unwrap()) as usize;
    let chunk: Vec<u8> = if inline {
        assert_eq!(
            index.len(),
            64 + comp_len,
            "expected a single-revision revlog at {index_path:?}"
        );
        index[64..].to_vec()
    } else {
        assert_eq!(
            index.len(),
            64,
            "expected a single-revision revlog at {index_path:?}"
        );
        let d = std::fs::read(&data_path).unwrap();
        assert_eq!(d.len(), comp_len);
        d
    };
    let mut text = match chunk.first() {
        None => Vec::new(),
        Some(0x00) => chunk.clone(),
        Some(b'u') => chunk[1..].to_vec(),
        Some(b'x') => {
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(&chunk[..])
                .read_to_end(&mut out)
                .unwrap();
            out
        }
        Some(other) => panic!("unexpected chunk marker {other:#x}"),
    };
    mutate(&mut text);
    let mut new_chunk = vec![b'u'];
    new_chunk.extend_from_slice(&text);
    let mut out = index[..64].to_vec();
    out[8..12].copy_from_slice(&u32::try_from(new_chunk.len()).unwrap().to_be_bytes());
    out[12..16].copy_from_slice(&u32::try_from(text.len()).unwrap().to_be_bytes());
    if inline {
        out.extend_from_slice(&new_chunk);
    } else {
        std::fs::write(&data_path, &new_chunk).unwrap();
    }
    std::fs::write(index_path, out).unwrap();
    index[32..52].try_into().unwrap()
}

fn one_commit_repo() -> Repo {
    let r = Repo::new();
    r.write("a.txt", "hello\n");
    r.commit("1136239445", "initial");
    r
}

fn assert_refused_naming(result: Result<brygge_ir::Ir, crate::Error>, node: [u8; 20]) {
    match result {
        Err(crate::Error::Read(msg)) => assert_eq!(
            msg,
            format!(
                "revision {} does not match its content (corrupt or crafted store)",
                crate::util::hex(&node)
            )
        ),
        other => panic!("expected the mismatch refusal naming the node, got {other:?}"),
    }
}

#[test]
fn an_untouched_store_verifies_and_decodes() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = one_commit_repo();
    assert!(decode(r.path(), &Options::default()).is_ok());
}

#[test]
fn a_changeset_whose_text_was_altered_without_its_node_is_refused() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = one_commit_repo();
    // The node `hg` itself reports for the changeset, asked before the store is altered (afterwards `hg`
    // refuses the store too — it checks every revision's hash on read).
    let out = Command::new("hg")
        .env("HGRCPATH", "/dev/null")
        .arg("-R")
        .arg(r.path())
        .args(["log", "-r", "0", "-T", "{node}"])
        .output()
        .unwrap();
    let hg_node = String::from_utf8(out.stdout).unwrap();
    let node = alter_single_revision(&r.path().join(".hg/store/00changelog.i"), |t| {
        t.extend_from_slice(b" (edited)");
    });
    assert_eq!(hg_node, crate::util::hex(&node));
    assert_refused_naming(decode(r.path(), &Options::default()), node);
}

#[test]
fn a_manifest_whose_text_was_altered_without_its_node_is_refused() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = one_commit_repo();
    let node = alter_single_revision(&r.path().join(".hg/store/00manifest.i"), |t| {
        // Rename the tracked path in place, same length: `a.txt` -> `b.txt`.
        assert_eq!(&t[..5], b"a.txt");
        t[0] = b'b';
    });
    assert_refused_naming(decode(r.path(), &Options::default()), node);
}

#[test]
fn a_filelog_revision_whose_text_was_altered_without_its_node_is_refused() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = one_commit_repo();
    let node = alter_single_revision(&r.path().join(".hg/store/data/a.txt.i"), |t| {
        assert_eq!(t.as_slice(), b"hello\n");
        t[0] = b'J';
    });
    assert_refused_naming(decode(r.path(), &Options::default()), node);
}

#[test]
fn a_filelog_revision_with_copy_metadata_decodes_and_carries_the_copy() {
    // Verification hashes the raw text including the `\1\ncopy: …\n\1\n` header, so a real `hg cp`/`hg mv`
    // filelog revision — which carries that header — must still verify and decode.
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("old.txt", "moved content\n");
    r.commit("1136239445", "add");
    r.run(&["mv", "old.txt", "new.txt"]);
    r.commit("1136239446", "move");
    let ir = decode(r.path(), &Options::default()).unwrap();
    let copies: Vec<_> = ir.atoms.iter().flat_map(|a| &a.copies).collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(
        (copies[0].from.as_str(), copies[0].to.as_str()),
        ("old.txt", "new.txt")
    );
}

// ---- release-prep §4: artifact text carries no internal references --------------------------------------

/// True if `s` contains `\b(RFC|OQ|CR|PR|INV|NG|SRC|FS|VF|HO|CL|CT|FA)[- ]?[0-9]` — an internal reference
/// number users and prikk cannot look up (review 012 F-3: the separator may be a space, as in `RFC 011`).
/// `OQ` also matches a lettered form (`OQ-B`), which the numeric pattern alone would miss.
/// Hand-rolled (no regex dependency).
fn has_internal_reference(s: &str) -> bool {
    const PREFIXES: [&str; 13] = [
        "RFC", "OQ", "CR", "PR", "INV", "NG", "SRC", "FS", "VF", "HO", "CL", "CT", "FA",
    ];
    let bytes = s.as_bytes();
    for start in 0..bytes.len() {
        let word_start =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        if !word_start {
            continue;
        }
        for p in PREFIXES {
            if bytes[start..].starts_with(p.as_bytes()) {
                let mut i = start + p.len();
                let separated = matches!(bytes.get(i), Some(&b'-') | Some(&b' '));
                if separated {
                    i += 1;
                }
                // An open-question reference is lettered (`OQ-B`), not numbered: accept a capital there too.
                let lettered =
                    p == "OQ" && separated && bytes.get(i).is_some_and(u8::is_ascii_uppercase);
                if bytes.get(i).is_some_and(u8::is_ascii_digit) || lettered {
                    return true;
                }
            }
        }
    }
    false
}

#[test]
fn the_internal_reference_matcher_matches_what_the_handoff_describes() {
    for bad in [
        "(PR-7)",
        "see RFC-5",
        "RFC005",
        "CR-03",
        "a (INV-3) b",
        "x FS-02",
        "VF-1",
        "RFC 011",
        "(RFC 005 D-4)",
        "OQ-B",
        "(FA-1)",
    ] {
        assert!(has_internal_reference(bad), "should match: {bad:?}");
    }
    for good in [
        "representation not assertion",
        "PRIVATE 7",
        "the CRT tube",
        "copy sources not resolvable (2)",
        "a CLI user",
        "HOME1",
        "phase 2 changesets",
        "the OQUI 7",
    ] {
        assert!(!has_internal_reference(good), "should not match: {good:?}");
    }
}

fn assert_no_internal_references(ir: &brygge_ir::Ir, what: &str) {
    for d in &ir.loss.dropped {
        assert!(
            !has_internal_reference(&d.what) && !has_internal_reference(&d.reason),
            "{what}: a drop carries an internal reference: {d:?}"
        );
    }
    for f in &ir.flags {
        assert!(
            !has_internal_reference(&f.what) && !has_internal_reference(&f.reason),
            "{what}: a flag carries an internal reference: {f:?}"
        );
    }
}

#[test]
fn every_loss_record_the_decoder_can_produce_is_free_of_internal_references() {
    // Every optional record at once (a store with phaseroots and an obsstore, and every counter non-zero),
    // so each `DropRecord` text this crate writes is scanned without needing a repository per record.
    let dir = std::env::temp_dir().join(format!("brygge-hg-scan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("phaseroots"), b"").unwrap();
    std::fs::write(dir.join("obsstore"), b"").unwrap();
    let lb = super::loss_boundary(&dir, 3, 2, 1, 1, 1, 1);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        lb.dropped.len() >= 9,
        "expected every record, got {}",
        lb.dropped.len()
    );
    for d in &lb.dropped {
        assert!(
            !has_internal_reference(&d.what) && !has_internal_reference(&d.reason),
            "a drop carries an internal reference: {d:?}"
        );
    }
}

#[test]
fn the_loss_and_flag_text_of_real_fixtures_is_free_of_internal_references() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert_no_internal_references(&ir, "build_repo");

    let secret = Repo::new();
    secret.write("a.txt", "a\n");
    secret.commit("1136239445", "public");
    secret.write("a.txt", "b\n");
    secret.run(&["commit", "-d", "1136239446 0", "-m", "secret", "--secret"]);
    let ir = decode(secret.path(), &Options::default()).unwrap();
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what.starts_with("changesets not published")),
        "the secret fixture should record its exclusion: {:?}",
        ir.loss.dropped
    );
    assert_no_internal_references(&ir, "secret fixture");
}

#[test]
fn refusal_messages_are_free_of_internal_references() {
    // The requirement gate's own refusal texts, for every branch it can take.
    for req in [
        "largefiles",
        "lfs",
        "revlogv2",
        "treemanifest",
        "narrowhg",
        "something-new",
    ] {
        match crate::requires::check(req) {
            Err(crate::Error::FloorRefusal { reason, .. })
            | Err(crate::Error::UnsupportedFormat { reason, .. }) => {
                assert!(!has_internal_reference(&reason), "{req}: {reason:?}");
            }
            other => panic!("{req}: expected a refusal, got {other:?}"),
        }
    }
}

// ---- release prep part 2: hg infers nothing, so provenance claims no inference -----------------------------

#[test]
fn an_hg_artifact_records_no_rename_inference_params() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.run(&["mv", "a.txt", "b.txt"]);
    r.commit("2", "c1 (a stated rename)");
    let ir = decode(r.path(), &Options::default()).unwrap();
    let keys: Vec<&String> = ir.provenance.params.keys().collect();
    assert!(
        !keys
            .iter()
            .any(|k| k.starts_with("rename_") || k.as_str() == "infer_renames"),
        "no inference param may be claimed, got {keys:?}"
    );
    // The stated rename is still carried as Stated, regardless.
    assert!(
        ir.atoms
            .iter()
            .flat_map(|a| &a.copies)
            .all(|c| c.status == brygge_ir::EpistemicStatus::Stated)
    );
    assert_eq!(
        Options::default().as_params().len(),
        0,
        "the options themselves render nothing"
    );
}

// ---- release prep part 2 §4: one bookmarks parse, strict and bounded ------------------------------------

#[test]
fn a_malformed_bookmark_line_is_a_read_error_through_the_single_path() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.write(".hg/bookmarks", "this-line-has-no-node-and-no-space\n");
    match decode(r.path(), &Options::default()) {
        Err(crate::Error::Read(m)) => assert!(m.contains("malformed bookmarks line"), "{m}"),
        other => panic!("expected Error::Read, got {other:?}"),
    }
}

#[test]
fn a_bookmark_naming_an_unknown_changeset_is_counted_not_skipped() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.commit("1", "c0");
    r.write(".hg/bookmarks", &format!("{} ghost\n", "0".repeat(40)));
    let ir = decode(r.path(), &Options::default()).unwrap();
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "bookmarks naming unimported changesets (1)")
    );
    assert!(ir.refs.iter().all(|rf| rf.name != "ghost"));
}

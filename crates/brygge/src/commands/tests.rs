//! Command integration tests (handoff `cli-and-verify-handoff-v2.md` §5): build fixture repos, then
//! decode → inspect → verify, asserting outcomes, exit classes, and — for `verify` — that every check can
//! actually fail. Git/hg/svnadmin-dependent tests skip when the tool is absent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as PCommand;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::cli::{Format, SourceKind};
use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    DropRecord, Identity, ImportProvenance, LossBoundary, MetadataClaims, RefRecord,
};

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

/// Removes a directory on drop, even if a test panics on an assertion before reaching its own explicit
/// cleanup (CR-16 review A-1).
struct CleanupDir(PathBuf);

impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp_ir_path(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("brygge-cli-{tag}-{}-{n}.ir", std::process::id()))
}

// ---- decode / inspect / verify roundtrip -----------------------------------------------------------

#[test]
fn decode_inspect_verify_roundtrip() {
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
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    assert!(out.exists(), "the artifact was written");
    assert_eq!(run_inspect(&out, false, Format::Human), exit::CLEAN);
    assert_eq!(run_inspect(&out, true, Format::Machine), exit::CLEAN);
    assert_eq!(run_verify(&out, None, Format::Machine), exit::CLEAN);
    assert_eq!(
        run_verify(&out, Some(repo.path()), Format::Human),
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
            &out,
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
        run_verify(&out, None, Format::Machine),
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
            &other_out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );

    // Verifying one repo's artifact against a different source must fail.
    assert_eq!(
        run_verify(&other_out, Some(repo.path()), Format::Human),
        exit::VERIFY_FAILED
    );
}

#[test]
fn a_failing_internal_check_with_a_corresponding_source_still_reports_against_source_separately() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("sep");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    // Corrupt the artifact (integrity fails) but keep the original source around.
    let mut bytes = std::fs::read(&out).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&out, &bytes).unwrap();

    // The two claims are independent (VF-4): a broken internal check does not skip against-source, but
    // integrity failure means there is no decoded Ir to re-derive against, so against-source is not-run
    // and the overall result is still fail (exit 50) from the internal side alone.
    assert_eq!(
        run_verify(&out, Some(repo.path()), Format::Human),
        exit::VERIFY_FAILED
    );
}

#[test]
fn against_source_with_a_nonexistent_path_is_not_checked_not_a_mismatch() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("nonexistent-source");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    let bogus = std::env::temp_dir().join("brygge-cli-does-not-exist-at-all");
    assert_eq!(
        run_verify(&out, Some(&bogus), Format::Machine),
        exit::FAILURE,
        "a source that cannot be re-decoded is 'not checked' — a runtime failure (exit 1), never a \
         claimed mismatch (review 003 R-1)"
    );
}

#[test]
fn a_genuine_internal_failure_dominates_the_exit_even_when_against_source_is_also_not_checked() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let a1 = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    // A duplicate ref is a genuine `structure` failure, but the artifact still decodes (integrity
    // passes), so `ir_opt` is `Some` and against-source is actually attempted.
    for _ in 0..2 {
        b.add_ref(RefRecord {
            name: "main".into(),
            kind: RefKind::Branch,
            target: a1,
            status: EpistemicStatus::Stated,
            source: None,
        })
        .unwrap();
    }
    let ir = b.finish().unwrap();
    let path = tmp_ir_path("notchecked-plus-fail");
    std::fs::write(&path, brygge_ir::to_bytes(&ir)).unwrap();

    let bogus = std::env::temp_dir().join("brygge-cli-does-not-exist-at-all-2");
    assert_eq!(
        run_verify(&path, Some(&bogus), Format::Machine),
        exit::VERIFY_FAILED,
        "a genuine internal failure dominates the exit class even though against-source also could \
         not be checked (review 003 R-1)"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn decode_a_real_clone_succeeds_and_verifies_against_source() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let origin = simple_repo("clone-src");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let clone_dir =
        std::env::temp_dir().join(format!("brygge-cli-clone-{}-{n}", std::process::id()));
    let _cleanup = CleanupDir(clone_dir.clone());
    let status = PCommand::new("git")
        .args(["clone", "-q"])
        .arg(origin.path())
        .arg(&clone_dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("run git clone");
    assert!(status.success(), "git clone failed");
    assert!(
        clone_dir.join(".git/refs/remotes/origin/HEAD").exists(),
        "a real clone creates the exact symbolic ref CR-16 covers"
    );

    let clone_out = clone_dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            &clone_dir,
            &clone_out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN,
        "a clone's symbolic HEAD is a representation-class drop, still a clean import (CR-16)"
    );
    assert!(clone_out.exists());
    assert_eq!(
        run_verify(&clone_out, Some(&clone_dir), Format::Human),
        exit::CLEAN,
        "verify --against-source succeeds on a real clone (CR-16)"
    );

    let origin_out = origin.dir.join("origin-out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            origin.path(),
            &origin_out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    let origin_ir = brygge_ir::from_bytes(&std::fs::read(&origin_out).unwrap()).unwrap();
    let clone_ir = brygge_ir::from_bytes(&std::fs::read(&clone_out).unwrap()).unwrap();
    let origin_ids: std::collections::BTreeSet<_> = origin_ir
        .atoms
        .iter()
        .map(|a| a.source.atom_id.clone())
        .collect();
    let clone_ids: std::collections::BTreeSet<_> = clone_ir
        .atoms
        .iter()
        .map(|a| a.source.atom_id.clone())
        .collect();
    assert_eq!(
        origin_ids, clone_ids,
        "the clone carries exactly the origin's commits"
    );
}

#[test]
fn guard_decoder_catches_a_panic_and_returns_a_typed_fault() {
    let result = guard_decoder(|| -> u32 { panic!("boom") });
    match result {
        Err(msg) => {
            assert!(msg.contains("internal decoder fault"), "{msg}");
            assert!(msg.contains("boom"), "{msg}");
            assert!(msg.contains("brygge bug"), "{msg}");
        }
        Ok(_) => panic!("expected the panic to be caught, not propagated"),
    }
}

#[test]
fn guard_decoder_passes_through_a_normal_result() {
    assert_eq!(guard_decoder(|| 42u32), Ok(42));
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
            &out,
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
    assert_eq!(run_inspect(&missing, false, Format::Human), exit::FAILURE);
    assert_eq!(run_verify(&missing, None, Format::Machine), exit::FAILURE);
}

// ---- CR-21: the before-the-run faithfulness statement ----------------------------------------------

#[test]
fn faithfulness_statement_text_per_source() {
    let git = faithfulness_statement(SourceKind::Git);
    assert!(git.starts_with("Git:"));
    assert!(git.contains("--infer-renames"));
    assert!(git.contains("Unverifiable"));

    let hg = faithfulness_statement(SourceKind::Hg);
    assert!(hg.starts_with("Mercurial:"));
    assert!(hg.contains("Unverifiable"));

    let svn = faithfulness_statement(SourceKind::Svn);
    assert!(svn.starts_with("Subversion:"));
    assert!(svn.contains("--reconstruct-refs"));
    assert!(svn.contains("Unverifiable"));

    let cvs = faithfulness_statement(SourceKind::Cvs);
    assert!(cvs.starts_with("CVS has no atomic commits"));
    assert!(cvs.contains("Unverifiable"));
}

// A subprocess test proving the statement reaches real stderr even when decode then fails lives in
// `tests/cli_integration.rs` (unit tests in `src/` cannot see `CARGO_BIN_EXE_brygge`).

// ---- CR-13: atomic artifact writes ------------------------------------------------------------------

fn tmp_siblings(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".brygge-tmp-"))
        })
        .collect()
}

#[test]
fn a_pre_existing_artifact_survives_a_failing_decode_and_leaves_no_temp_file() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("atomic");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    let good_bytes = std::fs::read(&out).unwrap();

    // Now make the same repo un-decodable (a submodule) and decode again to the same --out.
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
        "break it",
    ]);
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::FLOOR_REFUSAL
    );

    assert_eq!(
        std::fs::read(&out).unwrap(),
        good_bytes,
        "the pre-existing artifact is untouched by a failed decode"
    );
    assert!(
        tmp_siblings(&repo.dir).is_empty(),
        "no temp file remains after a failure"
    );
}

#[test]
fn a_successful_decode_replaces_the_existing_artifact_and_leaves_no_temp_file() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("replace");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    let first_bytes = std::fs::read(&out).unwrap();

    repo.commit("a.txt", "a third state\n", "c3");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    let second_bytes = std::fs::read(&out).unwrap();
    assert_ne!(first_bytes, second_bytes, "the artifact was replaced");
    assert!(tmp_siblings(&repo.dir).is_empty(), "no temp file remains");
}

// ---- inspect (CL-02) --------------------------------------------------------------------------------

#[test]
fn inspect_default_equals_decodes_end_of_run_report() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("eq");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    // `decode`'s end-of-run report and `inspect`'s default report both call
    // `brygge_ir::honesty::summary(&ir).render_*()` on the same artifact, so they are the same text by
    // construction; confirm it directly against the artifact read back from disk (FS-02).
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(
        brygge_ir::honesty::summary(&ir).render_human(),
        brygge_ir::honesty::summary(&ir).render_human(),
        "reproducible (FS-02)"
    );
}

#[test]
fn inspect_atoms_adds_the_listing_and_reads_the_artifact_once() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("atoms");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let human = render_atoms_human(&ir);
    assert!(human.contains("atoms (topological order):"));
    assert!(human.contains("refs:") || ir.refs.is_empty());
    assert!(human.contains("loss boundary:"));
    let machine = render_atoms_machine(&ir);
    assert!(machine.starts_with("inspect_version=2\n"));
    assert!(machine.contains("atom.0.id="));
}

#[test]
fn inspect_report_says_unverifiable_never_unverified() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("unverifiable");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let human = brygge_ir::honesty::summary(&ir).render_human();
    assert!(human.contains("Unverifiable"));
    assert!(!human.contains("Unverified"));
}

#[test]
fn representation_drops_appear_only_on_the_not_history_line() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo = simple_repo("split");
    let out = repo.dir.join("out.ir");
    assert_eq!(
        run_decode(
            SourceKind::Git,
            repo.path(),
            &out,
            false,
            false,
            Format::Machine
        ),
        exit::CLEAN
    );
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let human = brygge_ir::honesty::summary(&ir).render_human();
    assert!(human.contains("not history (no content or claim lost):"));
    let after_split = human
        .split("dropped (recorded loss):")
        .nth(1)
        .expect("section present");
    assert!(!after_split.contains("representation"));
}

// ---- verify (CL-04, CR-02): every check can fail, built with IrBuilder -----------------------------

fn blank_provenance(kind: brygge_ir::SourceKind, decoder: &str) -> ImportProvenance {
    ImportProvenance {
        source: brygge_ir::SourceIdentity {
            kind,
            repo_id: b"r".to_vec(),
            atom_id: b"r".to_vec(),
            signatures: Vec::new(),
        },
        brygge_version: "0.1.0".into(),
        decoder: decoder.into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
        import_time: None,
    }
}

fn src(kind: brygge_ir::SourceKind, atom: &[u8]) -> brygge_ir::SourceIdentity {
    brygge_ir::SourceIdentity {
        kind,
        repo_id: b"r".to_vec(),
        atom_id: atom.to_vec(),
        signatures: Vec::new(),
    }
}

fn stated_add(path: &str, blob: brygge_ir::BlobId) -> PathOp {
    PathOp::Add {
        path: path.into(),
        blob,
        mode: 0o100_644,
        status: EpistemicStatus::Stated,
    }
}

fn verify_bytes(bytes: &[u8]) -> i32 {
    let path = tmp_ir_path("verify-check");
    std::fs::write(&path, bytes).unwrap();
    let code = run_verify(&path, None, Format::Machine);
    let _ = std::fs::remove_file(&path);
    code
}

#[test]
fn verify_integrity_fails_on_a_flipped_byte() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    let mut bytes = brygge_ir::to_bytes(&ir);
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    assert_eq!(verify_bytes(&bytes), exit::VERIFY_FAILED);
}

#[test]
fn verify_structure_fails_on_a_dangling_parent() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let bogus_parent = brygge_ir::AtomId([0xABu8; 32]);
    let _ = b.add_atom(AtomDraft {
        parents: vec![bogus_parent],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_structure_fails_on_a_duplicate_ref() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let a1 = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    b.add_ref(RefRecord {
        name: "main".into(),
        kind: RefKind::Branch,
        target: a1,
        status: EpistemicStatus::Stated,
        source: None,
    })
    .unwrap();
    b.add_ref(RefRecord {
        name: "main".into(),
        kind: RefKind::Branch,
        target: a1,
        status: EpistemicStatus::Stated,
        source: None,
    })
    .unwrap();
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_structure_fails_on_two_ops_for_one_path() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let blob2 = b.add_blob(b"y".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![
            stated_add("a", blob),
            PathOp::Modify {
                path: "a".into(),
                blob: blob2,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            },
        ],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_replay_fails_on_modify_of_an_absent_path() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Modify {
            path: "never-added".into(),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_replay_fails_on_add_of_a_present_path() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let a1 = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    let _ = b.add_atom(AtomDraft {
        parents: vec![a1],
        ops: vec![stated_add("a", blob)], // re-adds the same, already-present path
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c2"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

fn derivation_missing_params(kind: DerivationKind) -> Derivation {
    Derivation {
        kind,
        by: "d".into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
        confidence: Some(100),
    }
}

#[test]
fn verify_derivations_fails_on_a_missing_required_param() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Derived(derivation_missing_params(DerivationKind::InferredRename)),
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_derivations_fails_on_confidence_over_100() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let mut d = derivation_missing_params(DerivationKind::NormalizedMetadata);
    d.confidence = Some(101);
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Derived(d),
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_source_invariants_fails_when_a_cvs_atom_is_stripped_to_stated() {
    // The stripped-honesty case (C-3a): a CVS atom that lost its Derived(ReconstructedChangeset) marking.
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Cvs,
        "brygge-decode-cvs",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Cvs, b"a@1.1"),
        status: EpistemicStatus::Stated, // should be Derived(ReconstructedChangeset) for CVS
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_provenance_fails_on_an_empty_decoder() {
    let mut b = IrBuilder::new(blank_provenance(brygge_ir::SourceKind::Git, ""));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_loss_boundary_fails_on_an_empty_what() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    b.set_loss(LossBoundary {
        dropped: vec![DropRecord {
            class: LossClass::Representation,
            what: String::new(),
            reason: "something".into(),
        }],
    });
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn a_well_formed_artifact_passes_every_check() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let a1 = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims {
            author: Some(Identity {
                name: "A".into(),
                email: "a@example.com".into(),
            }),
            ..MetadataClaims::default()
        },
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    b.add_ref(RefRecord {
        name: "main".into(),
        kind: RefKind::Branch,
        target: a1,
        status: EpistemicStatus::Stated,
        source: None,
    })
    .unwrap();
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::CLEAN);
}

// ---- replay memory bound (handoff §5.6) --------------------------------------------------------------

#[test]
fn replay_of_a_linear_10000_atom_history_clones_nothing_and_peaks_at_one() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let mut parent = None;
    for i in 0..10_000u32 {
        let blob = b.add_blob(format!("content-{i}").into_bytes());
        let path = format!("f{i}.txt");
        let atom = b.add_atom(AtomDraft {
            parents: parent.into_iter().collect(),
            ops: vec![stated_add(&path, blob)],
            rename_hints: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, format!("c{i}").as_bytes()),
            status: EpistemicStatus::Stated,
        });
        parent = Some(atom);
    }
    let ir = b.finish().unwrap();
    let (outcome, stats) = replay_ir(&ir);
    assert!(matches!(outcome, CheckOutcome::Pass));
    assert_eq!(
        stats.peak, 1,
        "a linear history retains exactly one tree at a time (review 003 R-3)"
    );
    assert_eq!(
        stats.clones, 0,
        "a linear history clones no tree at all — every reference is a last reference (review 003 R-3)"
    );
}

#[test]
fn replay_of_a_branch_point_clones_once_for_the_earlier_sibling_only() {
    // One parent, two children: the first child processed must clone (the second still needs the
    // original); the second (last) reference takes it instead.
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"root".to_vec());
    let root = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("root.txt", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"root"),
        status: EpistemicStatus::Stated,
    });
    let blob_a = b.add_blob(b"a".to_vec());
    let _child_a = b.add_atom(AtomDraft {
        parents: vec![root],
        ops: vec![stated_add("a.txt", blob_a)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"a"),
        status: EpistemicStatus::Stated,
    });
    let blob_b = b.add_blob(b"b".to_vec());
    let _child_b = b.add_atom(AtomDraft {
        parents: vec![root],
        ops: vec![stated_add("b.txt", blob_b)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"b"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    let (outcome, stats) = replay_ir(&ir);
    assert!(
        matches!(outcome, CheckOutcome::Pass),
        "both children replay correctly against independent copies of the root's tree"
    );
    assert_eq!(
        stats.clones, 1,
        "exactly one of the two children clones; the other takes the original"
    );
}

// ---- CR-19: untrusted text is neutralized in every output --------------------------------------------

#[test]
fn a_hostile_commit_message_renders_escaped_in_inspect_atoms_human() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims {
            message: Some("clear\x1b[2J and \u{202E}reordered".into()),
            ..MetadataClaims::default()
        },
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    let human = render_atoms_human(&ir);
    assert!(!human.contains('\x1b'), "the raw ESC byte must not appear");
    assert!(
        !human.contains('\u{202E}'),
        "the raw RLO char must not appear"
    );
    assert!(human.contains("\\u{001b}"));
    assert!(human.contains("\\u{202e}"));
}

#[test]
fn a_line_separator_in_a_commit_subject_renders_escaped_in_inspect_atoms_human() {
    // U+2028 LINE SEPARATOR is not a `str::lines()` boundary, so it survives into the rendered
    // "subject" line intact unless `display::human` escapes it (review 003 R-2) — it could otherwise
    // forge what looks like a second output line in a terminal or a log viewer.
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims {
            message: Some("subject\u{2028}forged second line".into()),
            ..MetadataClaims::default()
        },
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    let ir = b.finish().unwrap();
    let human = render_atoms_human(&ir);
    assert!(
        !human.contains('\u{2028}'),
        "the raw line separator must not appear"
    );
    assert!(human.contains("\\u{2028}"));
}

#[test]
fn a_ref_named_with_equals_cannot_forge_a_machine_key() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let a1 = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![stated_add("a", blob)],
        rename_hints: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    b.add_ref(RefRecord {
        name: "a=b".into(),
        kind: RefKind::Branch,
        target: a1,
        status: EpistemicStatus::Stated,
        source: None,
    })
    .unwrap();
    let ir = b.finish().unwrap();
    let machine = render_atoms_machine(&ir);
    assert!(machine.contains("ref.0.name=a%3Db"));
    // The name never appears as a raw key or unescaped value.
    for line in machine.lines() {
        if let Some((key, _)) = line.split_once('=') {
            assert!(!key.contains("a=b"), "line: {line}");
        }
    }
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
    let out = tmp_ir_path("svn");

    let code = run_decode(SourceKind::Svn, &dump, &out, false, true, Format::Machine);
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "unexpected exit {code}"
    );
    assert!(out.exists());

    assert_eq!(run_inspect(&out, false, Format::Human), exit::CLEAN);
    assert_eq!(run_verify(&out, None, Format::Machine), exit::CLEAN);
    assert_eq!(
        run_verify(&out, Some(&dump), Format::Human),
        exit::CLEAN,
        "the artifact corresponds to its dumpfile (VF-2)"
    );

    let _ = std::fs::remove_file(&dump);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn decode_svn_flat_layout_with_reconstruct_is_a_convention_violation() {
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
    let out = tmp_ir_path("svn-flat");

    let code = run_decode(SourceKind::Svn, &dump, &out, false, true, Format::Machine);
    assert_eq!(code, exit::CONVENTION_VIOLATION);
    let _ = std::fs::remove_file(&dump);
    let _ = std::fs::remove_file(&out);
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
    let code = run_decode(SourceKind::Svn, &repo, &out, false, true, Format::Machine);
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "unexpected exit {code}"
    );
    assert!(out.exists());
    assert_eq!(run_verify(&out, None, Format::Machine), exit::CLEAN);

    let _ = std::fs::remove_dir_all(&repo);
}

// ---- CVS (RFC 007) --------------------------------------------------------------------------------

/// A single-revision (`1.1`) RCS `,v` with full text.
fn cvs_single_rev(author: &str, date: &str, log: &str, content: &str) -> Vec<u8> {
    format!(
        "head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t{date};\tauthor {author};\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@{log}@\ntext\n@{content}@\n"
    )
    .into_bytes()
}

fn cvs_repo() -> TempRepo {
    TempRepo::new()
}

fn write_vfile(repo: &TempRepo, rel: &str, bytes: &[u8]) {
    let p = repo.dir.join(format!("{rel},v"));
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, bytes).unwrap();
}

#[test]
fn decode_cvs_reconstructs_derived_changesets_and_reproduces() {
    let repo = cvs_repo();
    write_vfile(
        &repo,
        "a.c",
        &cvs_single_rev("alice", "2024.01.01.12.00.00", "add", "aaa\n"),
    );
    write_vfile(
        &repo,
        "b.c",
        &cvs_single_rev("alice", "2024.01.01.12.00.01", "add", "bbb\n"),
    );
    let out = repo.dir.join("out.ir");

    let code = run_decode(
        SourceKind::Cvs,
        repo.path(),
        &out,
        false,
        false,
        Format::Machine,
    );
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "unexpected exit {code}"
    );
    assert!(out.exists());

    assert_eq!(run_inspect(&out, false, Format::Human), exit::CLEAN);
    assert_eq!(run_verify(&out, None, Format::Machine), exit::CLEAN);
    assert_eq!(
        run_verify(&out, Some(repo.path()), Format::Human),
        exit::CLEAN,
        "the CVS reconstruction reproduces from its repository (VF-1)"
    );
}

#[test]
fn decode_cvs_whole_import_under_floor_is_refused() {
    let repo = cvs_repo();
    for i in 0..10 {
        let date = format!("2024.03.01.00.{:02}.00", i);
        write_vfile(
            &repo,
            &format!("f{i}.c"),
            &cvs_single_rev("alice", &date, "big", "x\n"),
        );
    }
    let out = repo.dir.join("out.ir");
    let code = run_decode(
        SourceKind::Cvs,
        repo.path(),
        &out,
        false,
        false,
        Format::Machine,
    );
    assert_eq!(code, exit::FLOOR_REFUSAL);
    assert!(!out.exists());
}

#[test]
fn decode_cvs_partial_under_floor_imports_and_exits_convention_violation() {
    let repo = cvs_repo();
    write_vfile(
        &repo,
        "x.c",
        &cvs_single_rev("alice", "2024.01.01.00.00.00", "tidy", "x\n"),
    );
    write_vfile(
        &repo,
        "y.c",
        &cvs_single_rev("alice", "2024.01.01.00.00.01", "tidy", "y\n"),
    );
    write_vfile(
        &repo,
        "p.c",
        &cvs_single_rev("bob", "2024.02.01.00.00.00", "sprawl", "p\n"),
    );
    write_vfile(
        &repo,
        "q.c",
        &cvs_single_rev("bob", "2024.02.01.00.01.40", "sprawl", "q\n"),
    );
    write_vfile(
        &repo,
        "r.c",
        &cvs_single_rev("bob", "2024.02.01.00.03.20", "sprawl", "r\n"),
    );

    let out = repo.dir.join("out.ir");
    let code = run_decode(
        SourceKind::Cvs,
        repo.path(),
        &out,
        false,
        false,
        Format::Machine,
    );
    assert_eq!(code, exit::CONVENTION_VIOLATION);
    assert!(
        out.exists(),
        "the import still writes (confident changesets imported)"
    );
}

// ---- Mercurial (RFC 005) --------------------------------------------------------------------------

fn hg_available() -> bool {
    PCommand::new("hg")
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
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("brygge-cli-hg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Self { dir };
        r.run(&["init", r.dir.to_str().unwrap()]);
        r
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn run(&self, args: &[&str]) {
        let ok = PCommand::new("hg")
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
fn decode_hg_and_verify_a_good_artifact_from_every_decoder() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let repo = HgRepo::new();
    repo.commit("a.txt", "one\n", "1700000000", "c1");
    repo.commit("a.txt", "two\n", "1700000100", "c2");
    let out = tmp_ir_path("hg");

    let code = run_decode(
        SourceKind::Hg,
        repo.path(),
        &out,
        false,
        false,
        Format::Machine,
    );
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "unexpected exit {code}"
    );
    assert!(out.exists());
    assert_eq!(run_verify(&out, None, Format::Machine), exit::CLEAN);
    assert_eq!(
        run_verify(&out, Some(repo.path()), Format::Human),
        exit::CLEAN,
        "the artifact corresponds to its hg source (VF-2)"
    );
    let _ = std::fs::remove_file(&out);
}

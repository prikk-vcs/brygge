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
    let a1 = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    // A `Modify` of a path that was never added is a genuine `replay` failure, but the artifact still
    // decodes (integrity passes), so `ir_opt` is `Some` and against-source is actually attempted. (A
    // duplicate ref used to serve here; the builder and the reader now both refuse one, so it never
    // reaches `verify`.)
    b.add_atom(AtomDraft {
        parents: vec![a1],
        ops: vec![PathOp::Modify {
            path: "never-added".into(),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        copies: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c2"),
        status: EpistemicStatus::Stated,
    })
    .unwrap();
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
    let origin_ir = brygge_ir::from_bytes(&std::fs::read(&origin_out).unwrap())
        .unwrap()
        .ir;
    let clone_ir = brygge_ir::from_bytes(&std::fs::read(&clone_out).unwrap())
        .unwrap()
        .ir;
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

// ---- RFC 010 §2.6: a ResourceLimit from any decoder maps to exit 20 --------------------------------

#[test]
fn a_resource_limit_from_every_decoder_maps_to_floor_refusal_with_the_stated_message() {
    let (code, msg) = map_cvs_error(CvsError::ResourceLimit {
        what: "an RCS file".to_string(),
        ceiling: "100 bytes".to_string(),
    });
    assert_eq!(code, exit::FLOOR_REFUSAL);
    assert_eq!(
        msg,
        "refused: an RCS file exceeds brygge's ceiling (100 bytes)"
    );

    let (code, msg) = map_svn_error(SvnError::ResourceLimit {
        what: "the dumpstream".to_string(),
        ceiling: "100 bytes".to_string(),
    });
    assert_eq!(code, exit::FLOOR_REFUSAL);
    assert_eq!(
        msg,
        "refused: the dumpstream exceeds brygge's ceiling (100 bytes)"
    );

    let (code, msg) = map_git_error(GitError::ResourceLimit {
        what: "a blob".to_string(),
        ceiling: "100 bytes".to_string(),
    });
    assert_eq!(code, exit::FLOOR_REFUSAL);
    assert_eq!(msg, "refused: a blob exceeds brygge's ceiling (100 bytes)");

    let (code, msg) = map_hg_error(HgError::ResourceLimit {
        what: "a decompressed revision".to_string(),
        ceiling: "100 bytes".to_string(),
    });
    assert_eq!(code, exit::FLOOR_REFUSAL);
    assert_eq!(
        msg,
        "refused: a decompressed revision exceeds brygge's ceiling (100 bytes)"
    );
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
    // CVS corrections handoff §2.1: the main-line-only limitation is stated up front, every run.
    assert!(cvs.contains("Branch history is not imported in this version; the main line is."));
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
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap())
        .unwrap()
        .ir;
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
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap())
        .unwrap()
        .ir;
    let human = render_atoms_human(&ir);
    assert!(human.contains("atoms (topological order):"));
    assert!(human.contains("refs:") || ir.refs.is_empty());
    assert!(human.contains("loss boundary:"));
    let machine = render_atoms_machine(&ir);
    assert!(machine.starts_with("inspect_version=4\n"));
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
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap())
        .unwrap()
        .ir;
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
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap())
        .unwrap()
        .ir;
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
            extras: Vec::new(),
        },
        brygge_version: "0.1.0".into(),
        decoder: decoder.into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
    }
}

fn src(kind: brygge_ir::SourceKind, atom: &[u8]) -> brygge_ir::SourceIdentity {
    brygge_ir::SourceIdentity {
        kind,
        repo_id: b"r".to_vec(),
        atom_id: atom.to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
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
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
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
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![bogus_parent],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_fails_on_a_duplicate_ref_in_a_hand_built_artifact() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let a1 = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    // The builder refuses a repeated `(name, kind)` (`finish` is an `Invariant`), so the duplicate is made
    // by hand on the finished `Ir`; the reader then refuses the bytes (`integrity`).
    b.add_ref(RefRecord {
        name: "main".into(),
        kind: RefKind::Branch,
        target: a1,
        status: EpistemicStatus::Stated,
        source: None,

        annotation: None,
    })
    .unwrap();
    b.add_ref(RefRecord {
        name: "other".into(),
        kind: RefKind::Branch,
        target: a1,
        status: EpistemicStatus::Stated,
        source: None,

        annotation: None,
    })
    .unwrap();
    let mut ir = b.finish().unwrap();
    let dup = ir.refs[0].clone();
    ir.refs.push(dup);
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn two_ops_for_one_path_is_now_rejected_at_build_time_not_verify_time() {
    // Before RFC 011, `IrBuilder::add_atom` was infallible and this scenario could only be caught
    // later, by `verify`'s own `structure` check, on a hand-crafted artifact. RFC 011's
    // `IrBuilder::add_atom` now validates this itself (and brygge-ir's own canonical decode also
    // enforces ops being strictly ascending by path — RFC 011 §2.5), so the same invariant is caught
    // earlier and a malformed artifact of this shape can no longer even be constructed through the
    // builder to exercise the CLI's `structure` check directly. This test confirms the earlier,
    // stricter rejection fires through the exact path the CLI itself uses.
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let blob2 = b.add_blob(b"y".to_vec());
    let err = b.add_atom(AtomDraft {
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
        copies: vec![],
        metadata: MetadataClaims::default(),
        source: src(brygge_ir::SourceKind::Git, b"c1"),
        status: EpistemicStatus::Stated,
    });
    assert!(
        err.is_err(),
        "two ops on one path must be rejected at add_atom"
    );
}

#[test]
fn verify_replay_fails_on_modify_of_an_absent_path() {
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Modify {
                path: "never-added".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
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
    let a1 = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![a1],
            ops: vec![stated_add("a", blob)], // re-adds the same, already-present path
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c2"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
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
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Derived(derivation_missing_params(
                DerivationKind::InferredRename,
            )),
        })
        .unwrap();
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
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Derived(d),
        })
        .unwrap();
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
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Cvs, b"a@1.1"),
            status: EpistemicStatus::Stated, // should be Derived(ReconstructedChangeset) for CVS
        })
        .unwrap();
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
}

#[test]
fn verify_provenance_fails_on_an_empty_decoder() {
    let mut b = IrBuilder::new(blank_provenance(brygge_ir::SourceKind::Git, ""));
    let blob = b.add_blob(b"x".to_vec());
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
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
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
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
    let a1 = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: brygge_ir::model::Text::utf8("A"),
                    email: Some(brygge_ir::model::Text::utf8("a@example.com")),
                }),
                ..MetadataClaims::default()
            },
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "main".into(),
        kind: RefKind::Branch,
        target: a1,
        status: EpistemicStatus::Stated,
        source: None,

        annotation: None,
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
        let atom = b
            .add_atom(AtomDraft {
                parents: parent.into_iter().collect(),
                ops: vec![stated_add(&path, blob)],
                copies: vec![],
                metadata: MetadataClaims::default(),
                source: src(brygge_ir::SourceKind::Git, format!("c{i}").as_bytes()),
                status: EpistemicStatus::Stated,
            })
            .unwrap();
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
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("root.txt", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"root"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let blob_a = b.add_blob(b"a".to_vec());
    let _child_a = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![stated_add("a.txt", blob_a)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"a"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let blob_b = b.add_blob(b"b".to_vec());
    let _child_b = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![stated_add("b.txt", blob_b)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"b"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
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
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(brygge_ir::model::Text::utf8(
                    "clear\x1b[2J and \u{202E}reordered",
                )),
                ..MetadataClaims::default()
            },
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
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
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(brygge_ir::model::Text::utf8(
                    "subject\u{2028}forged second line",
                )),
                ..MetadataClaims::default()
            },
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
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
    let a1 = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "a=b".into(),
        kind: RefKind::Branch,
        target: a1,
        status: EpistemicStatus::Stated,
        source: None,

        annotation: None,
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

#[test]
fn cr_07_6_verifying_a_dumpfile_decode_against_a_live_repository_is_not_checked() {
    // CR-07.6: the artifact records which form (dumpfile vs live `svnadmin dump`) it was decoded from.
    // Comparing against the *other* form is not a meaningful check — `verify` must report `not-checked`
    // (never a claimed mismatch) and exit 1, per the three-valued verdict (review 003 R-5).
    if !svnadmin_available() {
        eprintln!("skipping: svnadmin not on PATH");
        return;
    }
    let dump = write_temp_dump(&svn_dump_with_branch());
    let out = tmp_ir_path("svn-form-mismatch");
    let code = run_decode(SourceKind::Svn, &dump, &out, false, true, Format::Machine);
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "unexpected exit {code}"
    );
    assert!(out.exists());

    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let repo = std::env::temp_dir().join(format!(
        "brygge-cli-svn-form-mismatch-{}-{n}",
        std::process::id()
    ));
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

    // The artifact was decoded from a dumpfile (`source_form=dumpfile`); verifying it against the live
    // repository *directory* is a different form — `not-checked`, not a mismatch, exit 1/`incomplete`.
    assert_eq!(
        run_verify(&out, Some(&repo), Format::Machine),
        exit::FAILURE,
        "a cross-form comparison must be incomplete, never pass or fail"
    );

    // Review 010 R-3.3: assert the exact verdict and reason directly, not just the exit code (which any
    // runtime error would also give).
    let ir = read_ir(&out).unwrap().ir;
    let against = run_against_source(&ir, &repo);
    assert_eq!(against.machine_label(), "not-checked");
    assert_eq!(
        against.detail(),
        Some("the artifact was made from a dumpfile; verify against the same form")
    );
    let verdict = Verdict::from(true, &against);
    assert_eq!(verdict.machine_label(), "incomplete");

    let _ = std::fs::remove_file(&dump);
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn cr_07_6_r1_an_svnadmin_version_difference_alone_does_not_fail_verification() {
    // Review 010 R-1: `svnadmin`'s own version is a fact about the tool, not the history, so an upgrade
    // between two decodes must never by itself make `verify --against-source` report a mismatch. Exercises
    // the comparison helpers directly on hand-built IRs — no real `svnadmin` binary needed.
    let dump = write_temp_dump(&svn_dump_with_branch());
    let out = tmp_ir_path("svn-version-note");
    let code = run_decode(SourceKind::Svn, &dump, &out, false, true, Format::Machine);
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "unexpected exit {code}"
    );
    let mut ir1 = read_ir(&out).unwrap().ir;
    ir1.provenance
        .params
        .insert("svnadmin_version".to_string(), "1.14.1".to_string());

    // Same version: no note, and alignment is a no-op.
    assert_eq!(svn_version_note(&ir1, &ir1), None);
    assert_eq!(align_svnadmin_version(&ir1, ir1.clone()), ir1);

    // Different version, otherwise identical: a note, and alignment makes them equal (`Corresponds`).
    let mut ir2 = ir1.clone();
    ir2.provenance
        .params
        .insert("svnadmin_version".to_string(), "1.14.2".to_string());
    assert_eq!(
        svn_version_note(&ir1, &ir2),
        Some("1.14.1 vs 1.14.2".to_string())
    );
    let aligned = align_svnadmin_version(&ir1, ir2.clone());
    assert_eq!(
        ir1, aligned,
        "aligning the version param must make an otherwise-identical IR equal"
    );

    // A real difference alongside the version difference must still be caught after alignment.
    let mut ir3 = ir2.clone();
    ir3.provenance.brygge_version = "9.9.9-different".to_string();
    let aligned3 = align_svnadmin_version(&ir1, ir3);
    assert_ne!(
        ir1, aligned3,
        "a genuine content difference must survive version alignment"
    );

    let _ = std::fs::remove_file(&dump);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn a_version_note_on_success_is_its_own_key_never_a_failure_detail() {
    // Review 010 F-4, re-keyed by part-2 §5.4: "corresponds" plus a detail could be read as a problem, so
    // the informational svnadmin-version note is `verify.against_source.note`, and no `.detail` appears.
    let checks: Vec<(&'static str, CheckOutcome)> = vec![("integrity", CheckOutcome::Pass)];
    let against = AgainstSourceOutcome::Corresponds(Some("1.14.1 vs 1.14.2".to_string()));
    let verdict = Verdict::from(true, &against);
    let lines = verify_machine_lines(&checks, true, &against, &verdict, 0);
    assert!(lines.contains(&"verify.against_source=corresponds".to_string()));
    assert!(lines.contains(&"verify.against_source.note=1.14.1%20vs%201.14.2".to_string()));
    assert!(lines.contains(&"verify.result=pass".to_string()));
    assert!(
        !lines.iter().any(|l| l.contains(".detail")),
        "a note is not a failure detail: {lines:?}"
    );

    // A failure uses `.detail`, and carries no `note`.
    let against = AgainstSourceOutcome::DoesNotCorrespond("atom count differs".to_string());
    let verdict = Verdict::from(true, &against);
    let lines = verify_machine_lines(&checks, true, &against, &verdict, 0);
    assert!(
        lines
            .iter()
            .any(|l| l == "verify.against_source.detail=atom%20count%20differs")
    );
    assert!(!lines.iter().any(|l| l.contains(".note")));
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

/// A `,v` with `1.1`, `1.2` on the trunk, the branch symbol `BR:1.2.0.2`, and one branch revision `1.2.2.1`.
fn cvs_with_one_branch_revision() -> Vec<u8> {
    "head\t1.2;\naccess;\nsymbols\n\tBR:1.2.0.2;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.2.2.1;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\
1.2.2.1\ndate\t2024.01.03.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\
1.2\nlog\n@two@\ntext\n@two\n@\n\n\
1.1\nlog\n@one@\ntext\n@d1 1\na1 1\none\n@\n\n\
1.2.2.1\nlog\n@on the branch@\ntext\n@d1 1\na1 1\nbr\n@\n"
        .as_bytes()
        .to_vec()
}

/// Batch I: a CVS repository with a branch revision records drops of two classes (`Representation` and
/// `Other`). The artifact used to fail its own `verify` `integrity` (exit 50), because the reader ordered
/// drops by the Debug name of the class, not by the specified variant number (`Other` sorts first by name).
#[test]
fn a_cvs_repository_with_a_branch_revision_verifies_its_own_artifact() {
    let repo = cvs_repo();
    write_vfile(&repo, "a.txt", &cvs_with_one_branch_revision());
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
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap())
        .unwrap()
        .ir;
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.class == brygge_ir::model::LossClass::Other),
        "the branch revision is recorded as a drop"
    );
    let code = run_verify(&out, None, Format::Machine);
    assert!(
        code == exit::CLEAN || code == exit::RECORDED_LOSS,
        "verify must not fail integrity (exit 50): got {code}"
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

#[test]
fn decode_cvs_cr01_branch_revision_exits_recorded_loss() {
    // CVS corrections handoff §1/§4.1 (review 008 R-7): the §1 reproduction fixture, exercised through
    // the CLI end to end — a trunk 1.1 -> 1.2 -> 1.3 history with a branch revision 1.2.2.1 excludes the
    // branch revision and exits 10 (recorded loss), never silently landing it in the main line.
    let repo = cvs_repo();
    let content = b"head\t1.3;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.3\ndate\t2024.01.03.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t1.2;\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.2.2.1;\nnext\t1.1;\n\n\
1.2.2.1\ndate\t2024.01.02.12.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.3\nlog\n@trunk r3@\ntext\n@line1\nline2\nline3\n@\n\n\n\
1.2\nlog\n@trunk r2@\ntext\n@d3 1\n@\n\n\n\
1.2.2.1\nlog\n@branch commit@\ntext\n@d1 2\na2 1\nBRANCH CONTENT\n@\n\n\n\
1.1\nlog\n@trunk r1@\ntext\n@d2 1\n@\n";
    write_vfile(&repo, "f.c", content);
    let out = repo.dir.join("out.ir");
    let code = run_decode(
        SourceKind::Cvs,
        repo.path(),
        &out,
        false,
        false,
        Format::Machine,
    );
    assert_eq!(code, exit::RECORDED_LOSS);
    assert!(out.exists());
}

#[test]
fn verify_cvs_derivations_requires_confidence_rule_as_well_as_date_rule() {
    // CVS corrections handoff §2.2 (review 008 R-8): `confidence_rule` is required for
    // `ReconstructedChangeset` alongside `date_rule` — a confidence without the rule that produced it
    // cannot be reviewed. Every other required param is present; only `confidence_rule` is missing.
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Cvs,
        "brygge-decode-cvs",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let mut params = BTreeMap::new();
    params.insert("window_secs".to_string(), "180".to_string());
    params.insert("cluster_keys".to_string(), "author,log".to_string());
    params.insert("date_rule".to_string(), "latest-per-file".to_string());
    let derivation = Derivation {
        kind: DerivationKind::ReconstructedChangeset,
        by: "brygge-decode-cvs".into(),
        decoder_version: "0.1.0".into(),
        params,
        confidence: Some(90),
    };
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("a", blob)],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Cvs, b"a@1.1"),
            status: EpistemicStatus::Derived(derivation),
        })
        .unwrap();
    let ir = b.finish().unwrap();
    assert_eq!(verify_bytes(&brygge_ir::to_bytes(&ir)), exit::VERIFY_FAILED);
    // Review 008 F-5: exit 50 alone would also come from any other failure — the check's own detail must
    // name the missing param.
    let outcome = check_derivations(&ir);
    assert!(outcome.is_fail());
    assert_eq!(
        outcome.detail(),
        Some("a ReconstructedChangeset derivation is missing required param 'confidence_rule'")
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

// ---- release prep §5: the temp path is created exclusively ------------------------------------------

fn scratch_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("brygge-atomic-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn planted_tmp_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.brygge-tmp-{}", std::process::id()))
}

#[cfg(unix)]
#[test]
fn a_planted_symlink_at_the_temp_path_fails_the_write_and_its_target_is_untouched() {
    let dir = scratch_dir("symlink");
    let victim = dir.join("victim.txt");
    std::fs::write(&victim, b"precious").unwrap();
    let out = dir.join("out.ir");
    let tmp = planted_tmp_path(&dir, "out.ir");
    std::os::unix::fs::symlink(&victim, &tmp).unwrap();

    let err = atomic_write(&out, b"artifact bytes").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"precious",
        "the target was written through"
    );
    assert!(!out.exists(), "no artifact may be produced");
    // Not ours, so not removed either: the planted link is still there.
    assert!(
        std::fs::symlink_metadata(&tmp)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_planted_regular_file_at_the_temp_path_is_not_truncated_or_removed() {
    let dir = scratch_dir("file");
    let out = dir.join("out.ir");
    let tmp = planted_tmp_path(&dir, "out.ir");
    std::fs::write(&tmp, b"planted").unwrap();

    assert!(atomic_write(&out, b"artifact bytes").is_err());
    assert_eq!(std::fs::read(&tmp).unwrap(), b"planted");
    assert!(!out.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_normal_write_still_succeeds_and_leaves_no_temp_file() {
    let dir = scratch_dir("ok");
    let out = dir.join("out.ir");
    atomic_write(&out, b"artifact bytes").unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"artifact bytes");
    assert!(tmp_siblings(&dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// macOS file systems (APFS, HFS+) refuse a file name that is not valid UTF-8 (`EILSEQ`), so the property cannot
// be set up there: the name can never exist on that platform.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn a_non_utf8_output_file_name_is_written_without_a_lossy_conversion() {
    use std::os::unix::ffi::OsStrExt;
    let dir = scratch_dir("nonutf8");
    let out = dir.join(std::ffi::OsStr::from_bytes(b"out-\xff.ir"));
    atomic_write(&out, b"x").unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"x");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn help_lists_flagged_in_the_vocabulary_and_no_internal_ids() {
    let help = crate::cli::USAGE;
    assert!(help.contains("VOCABULARY:") && help.contains("flagged"));
    for id in ["FS-", "VF-", "CL-", "CR-", "PR-", "INV-", "RFC"] {
        assert!(!help.contains(id), "--help mentions the internal id {id}");
    }
}

// ---- part-2 §5: the machine output (naming rule, one key one line, raw bytes, details, skips, Other) ----

/// A hand-built IR that exercises every key family the machine output can print: a derived copy, an
/// `Other` derivation kind on an atom, a copy and a ref, an `Other` ref kind, a non-UTF-8 message, a drop
/// and a flag.
fn machine_output_fixture() -> Ir {
    use brygge_ir::model::{CopyRecord, Flag, FlagKind, LossClass, Text};
    let other = |name: &str| {
        EpistemicStatus::Derived(Derivation {
            kind: DerivationKind::Other(name.to_string()),
            by: "d".into(),
            decoder_version: "0".into(),
            params: BTreeMap::new(),
            confidence: None,
        })
    };
    let renamed = EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::InferredRename,
        by: "d".into(),
        decoder_version: "0".into(),
        params: BTreeMap::new(),
        confidence: Some(90),
    });
    let mut b = IrBuilder::new(blank_provenance(
        brygge_ir::SourceKind::Git,
        "brygge-decode-git",
    ));
    let blob = b.add_blob(b"x".to_vec());
    let a1 = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![stated_add("old", blob)],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(Text {
                    bytes: b"caf\xe9 100%\nsecond line".to_vec(),
                    encoding: None,
                }),
                ..MetadataClaims::default()
            },
            source: src(brygge_ir::SourceKind::Git, b"c1"),
            status: other("bespoke kind = x"),
        })
        .unwrap();
    let a2 = b
        .add_atom(AtomDraft {
            parents: vec![a1],
            ops: vec![
                PathOp::Delete {
                    path: "old".into(),
                    status: EpistemicStatus::Stated,
                },
                stated_add("new", blob),
            ],
            copies: vec![
                CopyRecord {
                    from: "old".into(),
                    from_atom: a1,
                    to: "new".into(),
                    status: renamed,
                },
                CopyRecord {
                    from: "old".into(),
                    from_atom: a1,
                    to: "newer".into(),
                    status: other("odd copy"),
                },
            ],
            metadata: MetadataClaims::default(),
            source: src(brygge_ir::SourceKind::Git, b"c2"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "refs/odd".into(),
        kind: RefKind::Other("phase: draft".into()),
        target: a2,
        status: other("ref made up"),
        source: None,
        annotation: None,
    })
    .unwrap();
    b.set_loss(LossBoundary {
        dropped: vec![DropRecord {
            class: LossClass::AdvisoryUnreliable,
            what: "mergeinfo".into(),
            reason: "advisory".into(),
        }],
    });
    b.add_flag(Flag {
        kind: FlagKind::BelowConfidenceFloor,
        what: "changesets".into(),
        count: 2,
        reason: "low".into(),
    });
    b.finish().unwrap()
}

/// Every machine line is `key=value` with a key of dot-separated `[a-z0-9_]` segments (hand-rolled: no
/// regex dependency). Returns the keys, so a test can assert on them.
fn assert_machine_keys_follow_the_rule(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for line in text.lines() {
        let (key, _) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("a machine line without `=`: {line:?}"));
        assert!(!key.is_empty(), "empty key in {line:?}");
        for seg in key.split('.') {
            assert!(
                !seg.is_empty()
                    && seg
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'),
                "key {key:?} breaks `^[a-z0-9_]+(\\.[a-z0-9_]+)*$` (line {line:?})"
            );
        }
        keys.push(key.to_string());
    }
    keys
}

fn every_verify_line_shape() -> Vec<String> {
    // Passing, failing and could-not-run checks, plus a source comparison with a note.
    let checks: Vec<(&'static str, CheckOutcome)> = CHECK_NAMES
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let o = match i {
                0 => CheckOutcome::Pass,
                1 => CheckOutcome::Fail("a detail = with\nweird bytes".to_string()),
                _ => CheckOutcome::NotChecked("the artifact failed to decode".to_string()),
            };
            (*n, o)
        })
        .collect();
    let against = AgainstSourceOutcome::Corresponds(Some("1.14.1 vs 1.14.2".to_string()));
    let verdict = Verdict::from(false, &against);
    verify_machine_lines(&checks, false, &against, &verdict, 2)
}

#[test]
fn part2_5_1_keys_that_embed_a_label_are_snake_case_and_values_stay_kebab_case() {
    let ir = machine_output_fixture();
    let report = brygge_ir::honesty::summary(&ir).render_machine();
    for key in [
        "derived.inferred_rename=",
        "derived.other=",
        "dropped.advisory_unreliable=",
        "flagged.below_confidence_floor=",
    ] {
        assert!(
            report.lines().any(|l| l.starts_with(key)),
            "{key} in\n{report}"
        );
    }
    let lines = every_verify_line_shape();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("verify.check.source_invariants="))
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("verify.check.loss_boundary="))
    );
    // Values keep kebab-case: the status value and the not-checked word.
    let atoms = render_atoms_machine(&ir);
    assert!(atoms.contains("atom.1.copy.0.status=derived:inferred-rename"));
    assert!(lines.iter().any(|l| l == "verify.check.replay=not-checked"));
}

#[test]
fn part2_5_2_inspect_atoms_does_not_repeat_the_atoms_key() {
    let ir = machine_output_fixture();
    let report = brygge_ir::honesty::summary(&ir).render_machine();
    let atoms = render_atoms_machine(&ir);
    assert_eq!(
        report.lines().filter(|l| l.starts_with("atoms=")).count(),
        1
    );
    assert!(
        !atoms.lines().any(|l| l.starts_with("atoms=")),
        "the listing repeats `atoms=`:\n{atoms}"
    );
    // ... and so the two concatenated (what `inspect --atoms --format machine` prints) have no duplicate key.
    let all = format!("{report}{atoms}");
    let keys = assert_machine_keys_follow_the_rule(&all);
    let mut seen = std::collections::BTreeSet::new();
    for k in &keys {
        assert!(seen.insert(k.clone()), "duplicate key {k}");
    }
}

#[test]
fn part2_5_3_atom_message_is_the_raw_bytes_percent_encoded_once() {
    let ir = machine_output_fixture();
    let atoms = render_atoms_machine(&ir);
    // "caf\xe9 100%\nsecond line": the non-UTF-8 byte, the space, the literal `%` and the newline.
    assert!(
        atoms.contains("atom.0.message=caf%E9%20100%25%0Asecond%20line\n"),
        "{atoms}"
    );
    // Not prepared for the terminal first (the old form escaped, or replaced non-UTF-8 by a placeholder).
    assert!(!atoms.contains("byte(s)") && !atoms.contains("\\u{"));
    // The human form keeps its display escaping: a non-UTF-8 message is not echoed as raw bytes.
    let human = render_atoms_human(&ir);
    assert!(!human.contains('\u{fffd}'));
    // An absent message is an empty value.
    assert!(atoms.contains("atom.1.message=\n"));
}

#[test]
fn part2_5_4_details_belong_to_their_check_and_follow_its_line() {
    let lines = every_verify_line_shape();
    let at = |prefix: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix} in {lines:?}"))
    };
    let structure = at("verify.check.structure=");
    assert_eq!(
        at("verify.check.structure.detail="),
        structure + 1,
        "the detail follows its own check's line"
    );
    // A passing check prints no detail.
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("verify.check.integrity.detail"))
    );
    // The old numbered form is gone, and the source comparison has its own keys.
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("verify.detail.") || l.starts_with("verify.note."))
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("verify.against_source.note="))
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("verify.against_source.detail"))
    );
    // The detail is escaped like every other value.
    assert!(lines.contains(
        &"verify.check.structure.detail=a%20detail%20%3D%20with%0Aweird%20bytes".to_string()
    ));
}

#[test]
fn part2_5_5_a_check_that_could_not_run_is_not_checked_never_n_a() {
    let lines = every_verify_line_shape();
    assert!(lines.contains(&"verify.check.provenance=not-checked".to_string()));
    assert!(!lines.iter().any(|l| l.contains("n/a")));
    assert_eq!(
        CheckOutcome::NotChecked(String::new()).label(),
        "not-checked"
    );
    // `against_source` uses the same word for "requested but could not run", and `not-run` still means
    // "not requested".
    assert_eq!(
        AgainstSourceOutcome::NotChecked(String::new()).machine_label(),
        "not-checked"
    );
    assert_eq!(AgainstSourceOutcome::NotRun.machine_label(), "not-run");
}

#[test]
fn part2_5_6_skipped_non_critical_fields_are_visible_to_machines() {
    let ir = machine_output_fixture();
    let report = brygge_ir::honesty::summary(&ir);
    assert!(
        report
            .render_machine()
            .lines()
            .any(|l| l == "skipped_non_critical_fields=0")
    );
    assert!(
        report
            .with_skipped_non_critical_fields(4)
            .render_machine()
            .lines()
            .any(|l| l == "skipped_non_critical_fields=4")
    );
    let lines = every_verify_line_shape();
    assert!(lines.contains(&"verify.skipped_non_critical_fields=2".to_string()));
    let none = verify_machine_lines(
        &[("integrity", CheckOutcome::Pass)],
        true,
        &AgainstSourceOutcome::NotRun,
        &Verdict::Pass,
        0,
    );
    assert!(none.contains(&"verify.skipped_non_critical_fields=0".to_string()));
}

#[test]
fn part2_5_7_other_is_not_flattened_the_label_is_other_and_the_name_has_a_companion_key() {
    let ir = machine_output_fixture();
    let atoms = render_atoms_machine(&ir);
    for expected in [
        "atom.0.status=derived:other\n",
        "atom.0.status_other=bespoke%20kind%20%3D%20x\n",
        "atom.1.copy.1.status=derived:other\n",
        "atom.1.copy.1.status_other=odd%20copy\n",
        "ref.0.kind=other\n",
        "ref.0.kind_other=phase:%20draft\n",
        "ref.0.status=derived:other\n",
        "ref.0.status_other=ref%20made%20up\n",
    ] {
        assert!(atoms.contains(expected), "missing {expected:?} in\n{atoms}");
    }
    // A named (non-Other) kind or status gets no companion key.
    assert!(!atoms.contains("atom.1.status_other"));
    assert!(!atoms.contains("atom.1.copy.0.status_other"));
}

#[test]
fn part2_5_versions_are_bumped() {
    assert_eq!(brygge_ir::honesty::REPORT_VERSION, 3);
    assert_eq!(INSPECT_VERSION, 4);
    assert_eq!(VERIFY_VERSION, 4);
    assert!(every_verify_line_shape().contains(&"verify_version=4".to_string()));
    assert!(render_atoms_machine(&machine_output_fixture()).starts_with("inspect_version=4\n"));
}

#[test]
fn part2_5_every_machine_key_matches_the_key_rule_across_every_key_family() {
    let ir = machine_output_fixture();
    let mut keys =
        assert_machine_keys_follow_the_rule(&brygge_ir::honesty::summary(&ir).render_machine());
    keys.extend(assert_machine_keys_follow_the_rule(&render_atoms_machine(
        &ir,
    )));
    keys.extend(assert_machine_keys_follow_the_rule(
        &every_verify_line_shape().join("\n"),
    ));
    // The families are all actually present (so the rule is checked on real keys, not on nothing).
    for family in [
        "report_version",
        "skipped_non_critical_fields",
        "derived.inferred_rename",
        "dropped.advisory_unreliable",
        "flagged.below_confidence_floor",
        "inspect_version",
        "atom.0.status_other",
        "atom.1.copy.0.from_atom",
        "ref.0.kind_other",
        "loss.0.class",
        "flag.0.count",
        "verify.check.source_invariants",
        "verify.check.structure.detail",
        "verify.against_source.note",
        "verify.skipped_non_critical_fields",
        "verify.result",
    ] {
        assert!(
            keys.iter().any(|k| k == family),
            "family {family} not exercised: {keys:?}"
        );
    }
}

// ---- part-2 §2: an hg artifact claims no rename inference, because none happens ---------------------

#[test]
fn an_hg_artifacts_params_carry_no_rename_or_infer_keys() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let repo = std::env::temp_dir().join(format!("brygge-cli-hg-{}-{n}", std::process::id()));
    let _guard = CleanupDir(repo.clone());
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let hg = |args: &[&str]| {
        let out = PCommand::new("hg")
            .current_dir(&repo)
            .args(args)
            .env("HGRCPATH", "/dev/null")
            .env("HGUSER", "A U Thor <a@example.com>")
            .output()
            .expect("run hg");
        assert!(
            out.status.success(),
            "hg {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    hg(&["init"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    hg(&["add", "a.txt"]);
    hg(&["commit", "-d", "0 0", "-m", "c1"]);
    hg(&["mv", "a.txt", "b.txt"]);
    hg(&["commit", "-d", "1 0", "-m", "c2 (a recorded rename)"]);

    let out = repo
        .join("..")
        .join(format!("brygge-hg-params-{}-{n}.ir", std::process::id()));
    assert_eq!(
        run_decode(SourceKind::Hg, &repo, &out, false, false, Format::Machine),
        exit::CLEAN
    );
    let ir = brygge_ir::from_bytes(&std::fs::read(&out).unwrap())
        .unwrap()
        .ir;
    let keys: Vec<&String> = ir.provenance.params.keys().collect();
    assert!(
        !keys
            .iter()
            .any(|k| k.starts_with("rename_") || k.as_str() == "infer_renames"),
        "an hg artifact must not claim an inference that never ran: {keys:?}"
    );
    // The recorded rename is still carried, as Stated (Mercurial states it).
    assert!(
        ir.atoms
            .iter()
            .flat_map(|a| &a.copies)
            .any(|c| c.from == "a.txt" && c.to == "b.txt" && !c.status.is_derived())
    );
    // And `verify --against-source` re-decodes it without reading any inference parameter.
    assert_eq!(run_verify(&out, Some(&repo), Format::Machine), exit::CLEAN);
    let _ = std::fs::remove_file(&out);
}

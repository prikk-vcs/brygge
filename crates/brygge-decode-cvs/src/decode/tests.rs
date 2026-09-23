//! Integration tests for the CVS decoder (RFC 007): write a repository of RCS `,v` files, decode it, and
//! assert the M4 properties — every atom `Derived(ReconstructedChangeset)`, clustering, the per-file spine,
//! the confidence floor, determinism, and the refusals. No `cvs` tool is needed: `,v` files are plain text.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use brygge_ir::model::PathOp;
use brygge_ir::status::EpistemicStatus;

use super::decode as decode_unchecked;

/// `decode`, plus the artifact-text check on every result: every fixture in this file is scanned.
fn decode(source: &Source, opts: &Options) -> Result<brygge_ir::Ir, crate::Error> {
    let ir = decode_unchecked(source, opts)?;
    assert_no_internal_references(&ir);
    Ok(ir)
}
use crate::{Options, Source};

static COUNTER: AtomicU64 = AtomicU64::new(0);

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
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("brygge-cvs-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write_vfile(&self, rel: &str, bytes: &[u8]) {
        let p = self.dir.join(format!("{rel},v"));
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, bytes).unwrap();
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

/// A single-revision (`1.1`) `,v` with full text and optional symbols.
fn single_rev(
    author: &str,
    date: &str,
    log: &str,
    content: &str,
    symbols: &[(&str, &str)],
) -> Vec<u8> {
    let mut sym = String::new();
    for (n, r) in symbols {
        sym.push_str(&format!("\t{n}:{r}\n"));
    }
    format!(
        "head\t1.1;\naccess;\nsymbols\n{sym};\nlocks; strict;\n\n\n\
1.1\ndate\t{date};\tauthor {author};\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@{log}@\ntext\n@{content}@\n"
    )
    .into_bytes()
}

/// A two-revision `,v`: 1.1 = `v1\n` (add), 1.2 = `v2\n` (modify), distinct logs/dates.
fn two_rev(author: &str) -> Vec<u8> {
    format!(
        "head\t1.2;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.02.02.00.00.00;\tauthor {author};\tstate Exp;\nbranches;\nnext\t1.1;\n\n\
1.1\ndate\t2024.02.01.00.00.00;\tauthor {author};\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@edit@\ntext\n@v2\n@\n\n\n\
1.1\nlog\n@create@\ntext\n@d1 1\na1 1\nv1\n@\n"
    )
    .into_bytes()
}

/// A trunk `1.1 -> 1.2 -> 1.3` history with a branch revision `1.2.2.1` dated between `1.2` and `1.3`,
/// carrying distinct content (CVS corrections handoff §1's reproduction fixture).
fn trunk_with_branch_revision() -> Vec<u8> {
    b"head\t1.3;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.3\ndate\t2024.01.03.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t1.2;\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.2.2.1;\nnext\t1.1;\n\n\
1.2.2.1\ndate\t2024.01.02.12.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.3\nlog\n@trunk r3@\ntext\n@line1\nline2\nline3\n@\n\n\n\
1.2\nlog\n@trunk r2@\ntext\n@d3 1\n@\n\n\n\
1.2.2.1\nlog\n@branch commit@\ntext\n@d1 2\na2 1\nBRANCH CONTENT\n@\n\n\n\
1.1\nlog\n@trunk r1@\ntext\n@d2 1\n@\n"
        .to_vec()
}

/// A `cvs import`-shaped file: `head` is `1.1` (no real trunk commit yet), `branch 1.1.1;` is set,
/// `1.1` and `1.1.1.1` are identical content and same-dated (the vendor-import exception applies), and
/// `1.1.1.2` is a later, real edit on the vendor branch.
fn vendor_import_fixture() -> Vec<u8> {
    b"head\t1.1;\nbranch\t1.1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.1.1.1;\nnext\t;\n\n\
1.1.1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t1.1.1.2;\n\n\
1.1.1.2\ndate\t2024.01.05.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@import@\ntext\n@vendor\n@\n\n\n\
1.1.1.1\nlog\n@Initial revision@\ntext\n@@\n\n\n\
1.1.1.2\nlog\n@vendor update@\ntext\n@d1 1\na1 1\nvendor v2\n@\n"
        .to_vec()
}

/// The same vendor-branch shape, but a later real trunk commit (`1.2`) has cleared the admin `branch`
/// field — so `1.1.1.1`/`1.1.1.2` are no longer main line at all, even though they still exist.
fn vendor_import_with_cleared_branch_fixture() -> Vec<u8> {
    b"head\t1.2;\nbranch\t;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.02.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.1.1.1;\nnext\t;\n\n\
1.1.1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t1.1.1.2;\n\n\
1.1.1.2\ndate\t2024.01.05.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@real trunk edit@\ntext\n@vendor\nreal edit\n@\n\n\n\
1.1\nlog\n@import@\ntext\n@d2 1\n@\n\n\n\
1.1.1.1\nlog\n@Initial revision@\ntext\n@@\n\n\n\
1.1.1.2\nlog\n@vendor update@\ntext\n@d1 1\na1 1\nvendor v2\n@\n"
        .to_vec()
}

/// A single file whose two trunk revisions are committed by *different* (author, log) pairs, with a
/// clock-skew date: `1.2` (the later revision) is dated *earlier* than `1.1` (review 008 R-7). Different
/// (author, log) puts them in different clusters regardless of window, and the earlier-dated cluster
/// (holding `1.2`) would otherwise sort before the later-dated one (holding `1.1`) — the per-file order
/// fix (handoff §2.2 step 5) extracts `1.2` into its own singleton instead.
fn skewed_order_fixture() -> Vec<u8> {
    b"head\t1.2;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.2\ndate\t1970.01.01.00.16.40;\tauthor alice;\tstate Exp;\nbranches;\nnext\t1.1;\n\n\
1.1\ndate\t1970.01.01.00.33.20;\tauthor bob;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@logA@\ntext\n@v2\n@\n\n\n\
1.1\nlog\n@logB@\ntext\n@d1 1\na1 1\nv1\n@\n"
        .to_vec()
}

fn derived_changeset_count(ir: &brygge_ir::Ir) -> u64 {
    brygge_ir::summary(ir)
        .derived
        .get("reconstructed-changeset")
        .copied()
        .unwrap_or(0)
}

#[test]
fn two_files_committed_together_cluster_into_one_derived_changeset() {
    let r = Repo::new();
    // Same author, log, and near-identical time → one changeset.
    r.write_vfile(
        "a.c",
        &single_rev("alice", "2024.01.01.12.00.00", "add", "aaa\n", &[]),
    );
    r.write_vfile(
        "b.c",
        &single_rev("alice", "2024.01.01.12.00.01", "add", "bbb\n", &[]),
    );

    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(ir.atoms.len(), 1, "one reconstructed changeset");
    // The atom is Derived(ReconstructedChangeset) with a confidence.
    match &ir.atoms[0].status {
        EpistemicStatus::Derived(d) => {
            assert_eq!(d.kind.label(), "reconstructed-changeset");
            assert!(d.confidence.is_some());
            assert!(d.params.contains_key("window_secs"));
        }
        EpistemicStatus::Stated => panic!("a CVS atom must be Derived"),
    }
    // Both files are Adds, in one atom.
    let adds = ir.atoms[0]
        .ops
        .iter()
        .filter(|o| matches!(o, PathOp::Add { .. }))
        .count();
    assert_eq!(adds, 2);
    // The fidelity report makes the reconstruction prominent (SRC-C2).
    assert_eq!(derived_changeset_count(&ir), 1);
    // atom_id packs the source-native (path@rev) pairs (D-9/OQ-D).
    let id = String::from_utf8_lossy(&ir.atoms[0].source.atom_id);
    assert!(
        id.contains("a.c@1.1") && id.contains("b.c@1.1"),
        "atom_id packs (path@rev): {id}"
    );
}

#[test]
fn a_files_history_becomes_add_then_modify_across_changesets() {
    let r = Repo::new();
    r.write_vfile("prog.c", &two_rev("bob"));
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(ir.atoms.len(), 2, "two changesets (distinct logs/dates)");
    // Every atom is Derived.
    assert!(ir.atoms.iter().all(|a| a.status.is_derived()));
    assert_eq!(derived_changeset_count(&ir), 2);
    // First changeset adds prog.c (v1), second modifies it (v2).
    let first_add = ir.atoms[0]
        .ops
        .iter()
        .any(|o| matches!(o, PathOp::Add { path, .. } if path == "prog.c"));
    let second_modify = ir.atoms[1]
        .ops
        .iter()
        .any(|o| matches!(o, PathOp::Modify { path, .. } if path == "prog.c"));
    assert!(first_add && second_modify);
}

#[test]
fn decode_is_deterministic() {
    let r = Repo::new();
    r.write_vfile(
        "a.c",
        &single_rev("alice", "2024.01.01.12.00.00", "x", "aaa\n", &[]),
    );
    r.write_vfile(
        "dir/b.c",
        &single_rev("alice", "2024.01.01.12.00.02", "x", "bbb\n", &[]),
    );
    let a = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    let b = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(brygge_ir::to_bytes(&a), brygge_ir::to_bytes(&b));
    // and it round-trips through the artifact codec.
    let bytes = brygge_ir::to_bytes(&a);
    assert_eq!(
        brygge_ir::to_bytes(&brygge_ir::from_bytes(&bytes).unwrap().ir),
        bytes
    );
}

#[test]
fn a_whole_import_under_the_confidence_floor_is_refused() {
    let r = Repo::new();
    // Ten files, same author/log, each 100s after the previous: one wide, low-confidence changeset.
    for i in 0..10 {
        let date = format!("2024.03.01.00.{:02}.00", i);
        r.write_vfile(
            &format!("f{i}.c"),
            &single_rev("alice", &date, "big import", "x\n", &[]),
        );
    }
    let opts = Options {
        confidence_floor: 90,
        ..Options::default()
    };
    let err = decode(&Source::LocalRepo(r.path().to_path_buf()), &opts).unwrap_err();
    assert!(matches!(err, crate::Error::FloorRefusal { .. }));
}

#[test]
fn a_partially_low_confidence_import_is_flagged_not_refused() {
    let r = Repo::new();
    // A wide, low-confidence group (as above) alongside one tight, high-confidence single-file
    // changeset — the import as a whole is not refused, but the low-confidence group is flagged
    // (RFC 011 D-8).
    for i in 0..10 {
        let date = format!("2024.03.01.00.{:02}.00", i);
        r.write_vfile(
            &format!("f{i}.c"),
            &single_rev("alice", &date, "big import", "x\n", &[]),
        );
    }
    r.write_vfile(
        "solo.c",
        &single_rev("bob", "2024.03.02.00.00.00", "solo change", "y\n", &[]),
    );
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(ir.flags.len(), 1);
    assert_eq!(
        ir.flags[0].kind,
        brygge_ir::model::FlagKind::BelowConfidenceFloor
    );
    assert_eq!(ir.flags[0].count, 1);
    // Review 007-recut R-1: nothing was actually dropped for being low-confidence — it was imported and
    // flagged. One fact, one place: no `Other`-class drop duplicates the flag.
    assert!(
        !ir.loss
            .dropped
            .iter()
            .any(|d| d.class == brygge_ir::LossClass::Other),
        "no drop should duplicate the confidence-floor flag: {:?}",
        ir.loss.dropped
    );
}

#[test]
fn ref_reconstruction_is_opt_in_and_derived() {
    let r = Repo::new();
    r.write_vfile(
        "a.c",
        &single_rev(
            "alice",
            "2024.01.01.12.00.00",
            "add",
            "aaa\n",
            &[("REL_1", "1.1")],
        ),
    );

    // Off by default: no refs.
    let ir_off = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    assert!(ir_off.refs.is_empty());

    // On: a Derived tag ref appears.
    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir_on = decode(&Source::LocalRepo(r.path().to_path_buf()), &opts).unwrap();
    let rel = ir_on.refs.iter().find(|r| r.name == "REL_1").unwrap();
    assert!(rel.status.is_derived());
    assert_eq!(rel.kind, brygge_ir::RefKind::Tag);
}

#[test]
fn a_remote_source_is_refused() {
    let out = decode(
        &Source::LocalRepo(PathBuf::from(":pserver:anonymous@cvs.example.com:/cvsroot")),
        &Options::default(),
    );
    assert!(matches!(out, Err(crate::Error::FloorRefusal { .. })));
}

// --- CR-12.2: the floor is one declared list, recorded in provenance --------------------------------

#[test]
fn provenance_floor_param_equals_the_declared_list() {
    let r = Repo::new();
    r.write_vfile(
        "a.c",
        &single_rev("alice", "2024.01.01.12.00.00", "x", "aaa\n", &[]),
    );
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    let expected = crate::floor::joined();
    assert_eq!(ir.provenance.params.get("floor"), Some(&expected));
}

// --- CR-01: main line only (owner ruling D-2) ---------------------------------------------------------

#[test]
fn reproduction_a_branch_revision_no_longer_lands_in_a_main_line_tree() {
    // §1: before the fix, this repository's replayed tree would contain "BRANCH CONTENT" — content the
    // main line never had. After the fix, the branch revision is excluded and counted.
    let r = Repo::new();
    r.write_vfile("f.c", &trunk_with_branch_revision());
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();

    // Three main-line changesets (1.1, 1.2, 1.3); the branch revision contributes no atom.
    assert_eq!(ir.atoms.len(), 3, "1.1, 1.2, 1.3 — no atom for 1.2.2.1");
    let all_content: Vec<Vec<u8>> = ir
        .atoms
        .iter()
        .flat_map(|a| &a.ops)
        .filter_map(|op| match op {
            PathOp::Add { blob, .. } | PathOp::Modify { blob, .. } => {
                ir.content.get(blob).map(<[u8]>::to_vec)
            }
            _ => None,
        })
        .collect();
    assert!(
        !all_content
            .iter()
            .any(|c| c.windows(14).any(|w| w == b"BRANCH CONTENT")),
        "no replayed tree may contain the branch-only content: {all_content:?}"
    );

    // The drop record carries the correct counts: 1 revision, 0 named branches (this branch has no
    // symbol), so it falls into "unnamed branch revisions".
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| d.what.starts_with("CVS branch revisions not imported"))
        .expect("branch-revisions drop present");
    assert_eq!(
        drop.what,
        "CVS branch revisions not imported (1 revision(s) on 0 named branch(es) and unnamed branch \
         revisions)"
    );

    // Exit 10 (recorded loss): the drop is `Other`-class, non-representation.
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| !matches!(d.class, brygge_ir::LossClass::Representation))
    );
}

#[test]
fn a_vendor_import_gives_one_add_not_an_add_plus_a_spurious_modify() {
    let r = Repo::new();
    r.write_vfile("f.c", &vendor_import_fixture());
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    // Two changesets: the import itself (an Add, from 1.1.1.1 — 1.1 contributes no separate op), and the
    // later vendor update (a Modify).
    assert_eq!(ir.atoms.len(), 2);
    let adds = ir
        .atoms
        .iter()
        .flat_map(|a| &a.ops)
        .filter(|o| matches!(o, PathOp::Add { .. }))
        .count();
    assert_eq!(adds, 1, "no spurious extra Add/Modify pair for 1.1");
    let modifies = ir
        .atoms
        .iter()
        .flat_map(|a| &a.ops)
        .filter(|o| matches!(o, PathOp::Modify { .. }))
        .count();
    assert_eq!(modifies, 1, "the later vendor update (1.1.1.2) is a Modify");
}

#[test]
fn a_cleared_branch_field_drops_all_vendor_revisions_after_1_1() {
    let r = Repo::new();
    r.write_vfile("f.c", &vendor_import_with_cleared_branch_fixture());
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    // Only 1.1 and 1.2 are main line; 1.1.1.1 and 1.1.1.2 are excluded (branch is unset).
    assert_eq!(ir.atoms.len(), 2);
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| d.what.starts_with("CVS branch revisions not imported"))
        .expect("branch-revisions drop present");
    // Exact string (review 008 R-7): no symbol names this file's vendor branch, so it falls into
    // "unnamed branch revisions", not a named-branch count.
    assert_eq!(
        drop.what,
        "CVS branch revisions not imported (2 revision(s) on 0 named branch(es) and unnamed branch \
         revisions)"
    );
}

#[test]
fn a_default_branch_with_a_later_trunk_revision_is_refused() {
    // Review 008 R-5: `cvs admin -b` can set/reset the default branch after real trunk commits already
    // moved past its branch point — there is then no single honest main line, and brygge refuses rather
    // than guess.
    let r = Repo::new();
    let content = b"head\t1.2;\nbranch\t1.1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.02.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.1.1.1;\nnext\t;\n\n\
1.1.1.1\ndate\t2024.01.01.00.00.01;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@later trunk@\ntext\n@v2\n@\n\n\n\
1.1\nlog\n@root@\ntext\n@d1 1\na1 1\nv1\n@\n\n\n\
1.1.1.1\nlog\n@vendor@\ntext\n@vendor\n@\n";
    r.write_vfile("f.c", content);
    let err = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap_err();
    match err {
        crate::Error::FloorRefusal { feature, .. } => {
            assert_eq!(feature, "default-branch-with-later-trunk");
        }
        other => panic!("expected FloorRefusal, got {other:?}"),
    }
}

// --- CR-08.1: repo_id is content-derived, not a filesystem path ----------------------------------------

#[test]
fn repo_id_is_identical_across_two_different_filesystem_locations() {
    let r1 = Repo::new();
    r1.write_vfile(
        "a.c",
        &single_rev("alice", "2024.01.01.00.00.00", "x", "aaa\n", &[]),
    );
    let r2 = Repo::new();
    r2.write_vfile(
        "a.c",
        &single_rev("alice", "2024.01.01.00.00.00", "x", "aaa\n", &[]),
    );
    assert_ne!(r1.path(), r2.path(), "genuinely different filesystem paths");
    let ir1 = decode(
        &Source::LocalRepo(r1.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    let ir2 = decode(
        &Source::LocalRepo(r2.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(ir1.provenance.source.repo_id, ir2.provenance.source.repo_id);
    assert_eq!(brygge_ir::to_bytes(&ir1), brygge_ir::to_bytes(&ir2));
    // Not a filesystem path.
    assert_ne!(
        ir1.provenance.source.repo_id,
        r1.path().to_string_lossy().as_bytes().to_vec()
    );
}

#[test]
fn repo_id_uses_the_lowest_trunk_revision_not_literally_1_1() {
    // Review 008 R-6: a file need not start at 1.1 (e.g. `cvs admin -o` can strip early revisions) — the
    // fingerprint must use whichever trunk revision is lowest-numbered, not hard-code `1.1`.
    let content = b"head\t1.3;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.3\ndate\t2024.01.03.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t1.2;\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.3\nlog\n@r3@\ntext\n@v3\n@\n\n\n\
1.2\nlog\n@r2@\ntext\n@d1 1\na1 1\nv2\n@\n";
    let r = Repo::new();
    r.write_vfile("a.c", content);
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();

    // Hand-compute the expected fingerprint using 1.2 (the file's lowest trunk revision), per the
    // amended §2.3 definition, and confirm it matches.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"a.c");
    hasher.update([0u8]);
    hasher.update(b"1.2");
    hasher.update([0u8]);
    hasher.update(
        crate::rcs::parse_rcs(content)
            .unwrap()
            .revisions
            .get(&crate::rcs::RevNum(vec![1, 2]))
            .unwrap()
            .date
            .to_string()
            .as_bytes(),
    );
    hasher.update([0u8]);
    hasher.update(b"alice");
    hasher.update([0x0au8]);
    let expected = hasher.finalize().to_vec();
    assert_eq!(ir.provenance.source.repo_id, expected);
}

#[test]
fn a_repository_with_no_trunk_revision_at_all_is_refused() {
    // Review 008 R-6: nothing to fingerprint means nothing to import either.
    let r = Repo::new();
    r.write_vfile("f.c", &vendor_import_fixture_without_trunk());
    let err = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap_err();
    match err {
        crate::Error::Read(msg) => assert_eq!(msg, "no main-line revisions"),
        other => panic!("expected Error::Read(\"no main-line revisions\"), got {other:?}"),
    }
}

/// A file whose only revisions are on a vendor branch with **no admin `branch` field set** at all — so
/// none of its revisions are main line, and it has no trunk revision either (used to construct a
/// repository with no main-line revisions anywhere).
fn vendor_import_fixture_without_trunk() -> Vec<u8> {
    b"head\t1.1.1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1.1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1.1.1\nlog\n@only revision@\ntext\n@only\n@\n"
        .to_vec()
}

// --- CR-04: no committer/commit_time the source never stated; RCS dates are UTC ------------------------

#[test]
fn committer_and_commit_time_are_absent_and_offset_is_utc() {
    let r = Repo::new();
    r.write_vfile(
        "a.c",
        &single_rev("alice", "2024.01.01.12.00.00", "x", "aaa\n", &[]),
    );
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    let claims = &ir.atoms[0].metadata;
    assert!(claims.committer.is_none());
    assert!(claims.commit_time.is_none());
    assert!(claims.author.is_some());
    assert_eq!(claims.author_time.unwrap().offset_minutes, Some(0));
}

// --- clustering derivation params -----------------------------------------------------------------------

#[test]
fn derivation_params_carry_confidence_rule_and_date_rule() {
    let r = Repo::new();
    r.write_vfile(
        "a.c",
        &single_rev("alice", "2024.01.01.00.00.00", "x", "aaa\n", &[]),
    );
    let ir = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    let EpistemicStatus::Derived(d) = &ir.atoms[0].status else {
        panic!("must be Derived");
    };
    assert_eq!(
        d.params.get("confidence_rule").map(String::as_str),
        Some("span-overlap-v1")
    );
    assert_eq!(
        d.params.get("date_rule").map(String::as_str),
        Some("latest-per-file")
    );
    assert_eq!(d.params.get("order_splits").map(String::as_str), Some("0"));
}

// --- refs: main-line tags reconstruct; branch symbols are recorded, not reconstructed ------------------

#[test]
fn a_branch_symbol_is_not_reconstructed_and_is_counted_while_a_main_line_tag_still_is() {
    let r = Repo::new();
    // f.c: a main-line tag REL_1 at 1.1, and a branch revision at 1.2.2.1 tagged with a magic branch
    // number (DEV -> 1.2.0.2), so it names ONLY a non-main-line revision.
    let content =
        b"head\t1.2;\naccess;\nsymbols\n\tREL_1:1.1\n\tDEV:1.2.0.2;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.2.2.1;\nnext\t1.1;\n\n\
1.2.2.1\ndate\t2024.01.02.12.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@r2@\ntext\n@v1\nv2\n@\n\n\n\
1.2.2.1\nlog\n@branch@\ntext\n@branch content\n@\n\n\n\
1.1\nlog\n@r1@\ntext\n@d2 1\n@\n";
    r.write_vfile("f.c", content);
    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir = decode(&Source::LocalRepo(r.path().to_path_buf()), &opts).unwrap();

    let rel = ir.refs.iter().find(|r| r.name == "REL_1");
    assert!(rel.is_some(), "the main-line tag still reconstructs");

    assert!(
        !ir.refs.iter().any(|r| r.name == "DEV"),
        "the branch symbol is not reconstructed"
    );
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| d.what.starts_with("CVS branch symbols not reconstructed"))
        .expect("the branch symbol is counted");
    assert_eq!(
        drop.what, "CVS branch symbols not reconstructed (1)",
        "exact count (review 008 R-7)"
    );
}

#[test]
fn a_literal_vendor_branch_symbol_and_its_tag_are_both_counted_not_silently_skipped() {
    // Review 008 R-1/R-2: RCS stores a vendor branch's own symbol *literally* (e.g. `VENDOR:1.1.1`, an
    // odd-length number), not in magic form — `is_branch_rev` must classify it as a branch (not a tag),
    // and a *tag* on a now-excluded vendor revision (`REL:1.1.1.1`, once the vendor branch is cleared)
    // must be counted too, never silently skipped.
    let r = Repo::new();
    let mut content = vendor_import_with_cleared_branch_fixture();
    // Insert `symbols\n\tVENDOR:1.1.1\n\tREL:1.1.1.1\n;` into the admin section (replacing the empty
    // `symbols;`).
    let content_str = String::from_utf8(content).unwrap();
    content = content_str
        .replacen("symbols;", "symbols\n\tVENDOR:1.1.1\n\tREL:1.1.1.1\n;", 1)
        .into_bytes();
    r.write_vfile("f.c", &content);
    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir = decode(&Source::LocalRepo(r.path().to_path_buf()), &opts).unwrap();

    // Neither symbol is reconstructed as a ref: VENDOR names branch 1.1.1, whose revisions are all
    // excluded (the vendor branch was cleared); REL names 1.1.1.1, also excluded.
    assert!(
        ir.refs.is_empty(),
        "no ref should be fabricated: {:?}",
        ir.refs
    );

    let branch_drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| d.what.starts_with("CVS branch symbols not reconstructed"))
        .expect("VENDOR is classified as a branch symbol and counted");
    assert_eq!(branch_drop.what, "CVS branch symbols not reconstructed (1)");

    let tag_drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| {
            d.what
                .starts_with("CVS tags on branch revisions not reconstructed")
        })
        .expect("REL is counted, never silently skipped");
    assert_eq!(
        tag_drop.what,
        "CVS tags on branch revisions not reconstructed (1)"
    );

    // The excluded-revisions drop names 1 branch (VENDOR), not 0 named / unnamed.
    let branch_revs_drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| d.what.starts_with("CVS branch revisions not imported"))
        .expect("branch revisions drop present");
    assert_eq!(
        branch_revs_drop.what,
        "CVS branch revisions not imported (2 revision(s) on 1 branch(es))"
    );
}

// --- determinism with branches, the vendor branch, and an order split -----------------------------------

#[test]
fn decode_is_deterministic_with_branches_and_vendor_import_and_a_split() {
    let r = Repo::new();
    r.write_vfile("branchy.c", &trunk_with_branch_revision());
    r.write_vfile("vendor.c", &vendor_import_fixture());
    r.write_vfile("skewed.c", &skewed_order_fixture());
    let a = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    let b = decode(
        &Source::LocalRepo(r.path().to_path_buf()),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(brygge_ir::to_bytes(&a), brygge_ir::to_bytes(&b));

    // Review 008 R-7: this test's name has claimed "a split" since it was written; actually exercise
    // one, on a real atom, rather than merely not crashing.
    let has_split = a.atoms.iter().any(|atom| {
        matches!(
            &atom.status,
            EpistemicStatus::Derived(d) if d.params.get("order_splits").map(String::as_str) == Some("1")
        )
    });
    assert!(has_split, "expected one atom with order_splits == \"1\"");
}

// ---- artifact text carries no internal references ---------------------------------------------------

/// The first internal reference identifier in `s` — `\b(RFC|OQ|CR|PR|INV|NG|SRC|FS|VF|HO|CL|CT|FA)[- ]?[0-9]`
/// (a space is allowed, so `RFC 011` is caught; the handoff's own pattern missed it) — if any.
/// Hand-rolled so this crate gains no regex dependency.
fn internal_reference(s: &str) -> Option<&str> {
    const PREFIXES: [&str; 13] = [
        "RFC", "OQ", "CR", "PR", "INV", "NG", "SRC", "FS", "VF", "HO", "CL", "CT", "FA",
    ];
    let bytes = s.as_bytes();
    for start in 0..bytes.len() {
        let at_boundary = start == 0
            || bytes
                .get(start - 1)
                .is_some_and(|b| !(b.is_ascii_alphanumeric() || *b == b'_'));
        if !at_boundary {
            continue;
        }
        let Some(rest) = s.get(start..) else {
            continue; // not a char boundary
        };
        for prefix in PREFIXES {
            if let Some(after) = rest.strip_prefix(prefix) {
                let after = after
                    .strip_prefix('-')
                    .or_else(|| after.strip_prefix(' '))
                    .unwrap_or(after);
                if after.bytes().next().is_some_and(|b| b.is_ascii_digit()) {
                    return Some(rest.get(..prefix.len() + 2).unwrap_or(rest));
                }
            }
        }
    }
    None
}

/// Fail if any `DropRecord` or `Flag` `what`/`reason` in `ir` carries an internal reference: artifact
/// text is read by users and by other tools, which have no access to our internal numbering.
fn assert_no_internal_references(ir: &brygge_ir::Ir) {
    for d in &ir.loss.dropped {
        for text in [&d.what, &d.reason] {
            assert!(
                internal_reference(text).is_none(),
                "internal reference {:?} in a drop record: {text}",
                internal_reference(text)
            );
        }
    }
    for f in &ir.flags {
        for text in [&f.what, &f.reason] {
            assert!(
                internal_reference(text).is_none(),
                "internal reference {:?} in a flag: {text}",
                internal_reference(text)
            );
        }
    }
}

#[test]
fn the_internal_reference_matcher_matches_the_documented_pattern() {
    assert!(internal_reference("preserved (PR-7)").is_some());
    assert!(internal_reference("see RFC 006").is_some()); // a space between prefix and number counts
    assert!(internal_reference("(RFC 011 D-4)").is_some());
    assert!(internal_reference("per OQ-B").is_none()); // OQ-B: a letter, not a number, follows
    assert!(internal_reference("OQ-2").is_some());
    assert!(internal_reference("(FA-1)").is_some());
    assert!(internal_reference("FA 3").is_some());
    assert!(internal_reference("RFC006 x").is_some());
    assert!(internal_reference("per INV-3.").is_some());
    assert!(internal_reference("NG5").is_some());
    assert!(internal_reference("a CR-12 b").is_some());
    assert!(internal_reference("SCR-1 PRX-1 HOME1").is_none()); // no word boundary / not digits
    assert!(internal_reference("plain words only").is_none());
}

#[test]
fn every_kind_of_artifact_text_this_decoder_writes_is_free_of_internal_references() {
    // The `decode` wrapper above scans every fixture in this file; this one makes sure the categories that
    // only appear in particular fixtures — keyword expansion, branch revisions, branch symbols, a tag on a
    // branch revision, low-confidence changesets — are all actually produced and therefore scanned.
    let r = Repo::new();
    r.write_vfile(
        "kw.c",
        b"head\t1.1;\naccess;\nsymbols;\nlocks; strict;\nexpand\t@kv@;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@kw@\ntext\n@$Id$\n@\n",
    );
    r.write_vfile("branchy.c", &trunk_with_branch_revision());
    for i in 0..10 {
        let date = format!("2024.03.01.00.{:02}.00", i);
        r.write_vfile(
            &format!("wide{i}.c"),
            &single_rev("carol", &date, "big import", "x\n", &[]),
        );
    }
    r.write_vfile(
        "solo.c",
        &single_rev("bob", "2024.03.02.00.00.00", "solo", "y\n", &[]),
    );
    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir = decode(&Source::LocalRepo(r.path().to_path_buf()), &opts).unwrap();
    let whats: Vec<&str> = ir.loss.dropped.iter().map(|d| d.what.as_str()).collect();
    assert!(
        whats.iter().any(|w| w.contains("keyword expansion")),
        "{whats:?}"
    );
    assert!(
        whats
            .iter()
            .any(|w| w.starts_with("CVS branch revisions not imported")),
        "{whats:?}"
    );
    assert!(
        ir.flags
            .iter()
            .any(|f| f.what == "reconstruction confidence below the floor"),
        "{:?}",
        ir.flags
    );
    // (the scan itself ran inside `decode`)
    assert_no_internal_references(&ir);
}

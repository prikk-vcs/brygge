//! Integration tests for the CVS decoder (RFC 007): write a repository of RCS `,v` files, decode it, and
//! assert the M4 properties — every atom `Derived(ReconstructedChangeset)`, clustering, the per-file spine,
//! the confidence floor, determinism, and the refusals. No `cvs` tool is needed: `,v` files are plain text.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use brygge_ir::model::PathOp;
use brygge_ir::status::EpistemicStatus;

use super::decode;
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
        brygge_ir::to_bytes(&brygge_ir::from_bytes(&bytes).unwrap()),
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

//! Unit tests for the running tree model: directory-copy expansion and subtree deletion.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;

use brygge_ir::builder::IrBuilder;
use brygge_ir::model::{AtomId, ImportProvenance, PathOp, SourceIdentity, SourceKind};

use super::{Tree, apply_revision};
use crate::dumpstream::{NodeAction, NodeKind, NodeRecord};

fn builder() -> IrBuilder {
    IrBuilder::new(ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Svn,
            repo_id: Vec::new(),
            atom_id: Vec::new(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        brygge_version: "test".to_string(),
        decoder: "test".to_string(),
        decoder_version: "test".to_string(),
        params: std::collections::BTreeMap::new(),
    })
}

fn file_node(path: &str, action: NodeAction, text: Option<&[u8]>) -> NodeRecord {
    NodeRecord {
        path: path.to_string(),
        kind: Some(NodeKind::File),
        action,
        copyfrom: None,
        props: Some(Vec::new()),
        text: text.map(<[u8]>::to_vec),
    }
}

fn dir_copy(path: &str, from_rev: u64, from_path: &str) -> NodeRecord {
    NodeRecord {
        path: path.to_string(),
        kind: Some(NodeKind::Dir),
        action: NodeAction::Add,
        copyfrom: Some((from_rev, from_path.to_string())),
        props: None,
        text: None,
    }
}

fn added_paths(ops: &[PathOp]) -> Vec<String> {
    ops.iter()
        .filter_map(|o| match o {
            PathOp::Add { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_file_add_produces_an_add_op_and_populates_the_tree() {
    let mut b = builder();
    let kept = HashMap::new();
    let applied = apply_revision(
        &Tree::new(),
        vec![file_node("trunk/a.txt", NodeAction::Add, Some(b"hi"))],
        &kept,
        &HashMap::new(),
        &mut b,
    )
    .unwrap();
    assert!(applied.tree.contains_key("trunk/a.txt"));
    assert_eq!(added_paths(&applied.ops), vec!["trunk/a.txt".to_string()]);
    assert!(applied.copies.is_empty());
}

#[test]
fn a_directory_copy_expands_to_per_file_adds_with_stated_copy_records() {
    let mut b = builder();
    // r0: trunk/a and trunk/sub/c exist.
    let r0 = apply_revision(
        &Tree::new(),
        vec![
            file_node("trunk/a.txt", NodeAction::Add, Some(b"a")),
            file_node("trunk/sub/c.txt", NodeAction::Add, Some(b"c")),
        ],
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
    )
    .unwrap();
    // r0 is referenced by r1's copyfrom, so it is retained (RFC 010 increment 1).
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0u64, r0.tree.clone());
    // r0's atom (a stand-in id — this test exercises apply_revision directly, without going through
    // IrBuilder::add_atom for each revision).
    let r0_atom = AtomId([1u8; 32]);
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    revnum_to_atom.insert(0, r0_atom);

    // r1: copy trunk -> branches/x (a branch creation), from r0.
    let r1 = apply_revision(
        &r0.tree,
        vec![dir_copy("branches/x", 0, "trunk")],
        &kept,
        &revnum_to_atom,
        &mut b,
    )
    .unwrap();

    // every trunk file is now present under branches/x ...
    assert!(r1.tree.contains_key("branches/x/a.txt"));
    assert!(r1.tree.contains_key("branches/x/sub/c.txt"));
    // ... as Add ops ...
    let mut adds = added_paths(&r1.ops);
    adds.sort();
    assert_eq!(adds, vec!["branches/x/a.txt", "branches/x/sub/c.txt"]);
    // ... each with a Stated copy record back to its trunk origin, naming r0's atom.
    assert_eq!(r1.copies.len(), 2);
    assert!(
        r1.copies
            .iter()
            .all(|c| c.status == brygge_ir::EpistemicStatus::Stated && c.from_atom == r0_atom)
    );
    assert!(
        r1.copies
            .iter()
            .any(|c| c.from == "trunk/a.txt" && c.to == "branches/x/a.txt")
    );

    // r2: delete branches/x (whole subtree) -> Delete ops for both files. No copyfrom, so `kept` is
    // unchanged; the base is simply the previous tree.
    revnum_to_atom.insert(1, AtomId([2u8; 32]));
    let r2 = apply_revision(
        &r1.tree,
        vec![NodeRecord {
            path: "branches/x".to_string(),
            kind: None,
            action: NodeAction::Delete,
            copyfrom: None,
            props: None,
            text: None,
        }],
        &kept,
        &revnum_to_atom,
        &mut b,
    )
    .unwrap();
    assert!(!r2.tree.contains_key("branches/x/a.txt"));
    assert!(!r2.tree.contains_key("branches/x/sub/c.txt"));
    let deletes = r2
        .ops
        .iter()
        .filter(|o| matches!(o, PathOp::Delete { .. }))
        .count();
    assert_eq!(deletes, 2);
}

#[test]
fn a_copy_from_an_older_revision_resolves_to_the_correct_from_atom() {
    // Proves the revnum -> AtomId map is consulted by revision number, not by "the previous atom" —
    // a copy sourced from a revision several steps back must still resolve correctly (RFC 011 §5).
    let mut b = builder();
    let r0 = apply_revision(
        &Tree::new(),
        vec![file_node("trunk/a.txt", NodeAction::Add, Some(b"a"))],
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
    )
    .unwrap();
    let r0_atom = AtomId([9u8; 32]);
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0u64, r0.tree.clone());
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    revnum_to_atom.insert(0, r0_atom);

    // r1, r2: unrelated intervening revisions with their own atoms.
    let r1 = apply_revision(
        &r0.tree,
        vec![file_node("unrelated/b.txt", NodeAction::Add, Some(b"b"))],
        &kept,
        &revnum_to_atom,
        &mut b,
    )
    .unwrap();
    revnum_to_atom.insert(1, AtomId([1u8; 32]));
    let r2 = apply_revision(
        &r1.tree,
        vec![file_node("unrelated/c.txt", NodeAction::Add, Some(b"c"))],
        &kept,
        &revnum_to_atom,
        &mut b,
    )
    .unwrap();
    revnum_to_atom.insert(2, AtomId([2u8; 32]));

    // r3: copy a single file from r0 — two revisions back.
    let copy_node = NodeRecord {
        path: "trunk/copy-of-a.txt".to_string(),
        kind: Some(NodeKind::File),
        action: NodeAction::Add,
        copyfrom: Some((0, "trunk/a.txt".to_string())),
        props: Some(Vec::new()),
        text: None,
    };
    let r3 = apply_revision(&r2.tree, vec![copy_node], &kept, &revnum_to_atom, &mut b).unwrap();

    assert_eq!(r3.copies.len(), 1);
    assert_eq!(r3.copies[0].from_atom, r0_atom);
    assert_eq!(r3.copies[0].from, "trunk/a.txt");
    assert_eq!(r3.copies[0].to, "trunk/copy-of-a.txt");
}

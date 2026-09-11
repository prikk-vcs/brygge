//! Unit tests for the running tree model: directory-copy expansion and subtree deletion.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;

use brygge_ir::builder::IrBuilder;
use brygge_ir::model::{ImportProvenance, PathOp, SourceIdentity, SourceKind};

use super::{Tree, apply_revision};
use crate::dumpstream::{NodeAction, NodeKind, NodeRecord};

fn builder() -> IrBuilder {
    IrBuilder::new(ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Svn,
            repo_id: Vec::new(),
            atom_id: Vec::new(),
            signatures: Vec::new(),
        },
        brygge_version: "test".to_string(),
        decoder: "test".to_string(),
        decoder_version: "test".to_string(),
        params: std::collections::BTreeMap::new(),
        import_time: None,
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
        &mut b,
    )
    .unwrap();
    assert!(applied.tree.contains_key("trunk/a.txt"));
    assert_eq!(added_paths(&applied.ops), vec!["trunk/a.txt".to_string()]);
    assert!(applied.hints.is_empty());
}

#[test]
fn a_directory_copy_expands_to_per_file_adds_with_stated_rename_hints() {
    let mut b = builder();
    // r0: trunk/a and trunk/sub/c exist.
    let r0 = apply_revision(
        &Tree::new(),
        vec![
            file_node("trunk/a.txt", NodeAction::Add, Some(b"a")),
            file_node("trunk/sub/c.txt", NodeAction::Add, Some(b"c")),
        ],
        &HashMap::new(),
        &mut b,
    )
    .unwrap();
    // r0 is referenced by r1's copyfrom, so it is retained (RFC 010 increment 1).
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0u64, r0.tree.clone());

    // r1: copy trunk -> branches/x (a branch creation), from r0.
    let r1 = apply_revision(
        &r0.tree,
        vec![dir_copy("branches/x", 0, "trunk")],
        &kept,
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
    // ... each with a Stated rename/copy hint back to its trunk origin.
    assert_eq!(r1.hints.len(), 2);
    assert!(
        r1.hints
            .iter()
            .all(|h| h.status == brygge_ir::EpistemicStatus::Stated)
    );
    assert!(
        r1.hints
            .iter()
            .any(|h| h.from == "trunk/a.txt" && h.to == "branches/x/a.txt")
    );

    // r2: delete branches/x (whole subtree) -> Delete ops for both files. No copyfrom, so `kept` is
    // unchanged; the base is simply the previous tree.
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

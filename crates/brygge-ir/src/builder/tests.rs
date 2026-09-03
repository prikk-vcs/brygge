//! Tests for deterministic IR construction.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::model::{Identity, MetadataClaims, RefKind, SourceKind};
use crate::status::{Derivation, DerivationKind};

fn provenance() -> ImportProvenance {
    ImportProvenance {
        source: src(b"repo"),
        brygge_version: "0.1.0".into(),
        decoder: "brygge-decode-git".into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
        import_time: Some(1_000),
    }
}

fn src(atom: &[u8]) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"repo".to_vec(),
        atom_id: atom.to_vec(),
        signatures: Vec::new(),
    }
}

fn draft(
    parents: Vec<AtomId>,
    source_atom: &[u8],
    ops: Vec<PathOp>,
    hints: Vec<RenameHint>,
) -> AtomDraft {
    AtomDraft {
        parents,
        ops,
        rename_hints: hints,
        metadata: MetadataClaims {
            author: Some(Identity {
                name: "A".into(),
                email: "a@x".into(),
            }),
            message: Some("m".into()),
            ..MetadataClaims::default()
        },
        source: src(source_atom),
        status: EpistemicStatus::Stated,
    }
}

fn build() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"content".to_vec());
    let root = b.add_atom(draft(
        vec![],
        b"aaa",
        vec![PathOp::Add {
            path: "readme".into(),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        vec![],
    ));
    let child_blob = b.add_blob(b"content2".to_vec());
    let _child = b.add_atom(draft(
        vec![root],
        b"bbb",
        vec![PathOp::Modify {
            path: "docs/readme".into(),
            blob: child_blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        vec![RenameHint {
            from: "readme".into(),
            to: "docs/readme".into(),
            status: EpistemicStatus::Derived(Derivation {
                kind: DerivationKind::InferredRename,
                by: "brygge-decode-git".into(),
                decoder_version: "0.1.0".into(),
                params: BTreeMap::new(),
                confidence: Some(90),
            }),
        }],
    ));
    b.add_ref(RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: root,
        status: EpistemicStatus::Stated,
        source: None,
    })
    .unwrap();
    b.finish().unwrap()
}

#[test]
fn builds_with_topological_order_root_first() {
    let ir = build();
    assert_eq!(ir.atoms.len(), 2);
    // The root (no parents) must be emitted before its child.
    let child = ir.atoms.iter().find(|a| !a.parents.is_empty()).unwrap();
    let root_pos = ir.atoms.iter().position(|a| a.parents.is_empty()).unwrap();
    let child_pos = ir.atoms.iter().position(|a| a.id == child.id).unwrap();
    assert!(root_pos < child_pos);
    assert_eq!(ir.refs.len(), 1);
}

#[test]
fn two_identical_builds_produce_identical_atom_ids() {
    let a = build();
    let b = build();
    let ids_a: Vec<_> = a.atoms.iter().map(|x| x.id).collect();
    let ids_b: Vec<_> = b.atoms.iter().map(|x| x.id).collect();
    assert_eq!(ids_a, ids_b);
}

#[test]
fn ref_to_unknown_atom_is_rejected() {
    let mut b = IrBuilder::new(provenance());
    let err = b.add_ref(RefRecord {
        name: "x".into(),
        kind: RefKind::Branch,
        target: AtomId([9u8; 32]),
        status: EpistemicStatus::Stated,
        source: None,
    });
    assert!(err.is_err());
}

//! Tests for deterministic IR construction (RFC 003 D-5, RFC 011 §2.5).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::model::{Flag, FlagKind, Identity, MetadataClaims, RefKind, SourceKind, Text};
use crate::status::{Derivation, DerivationKind};

fn provenance() -> ImportProvenance {
    ImportProvenance {
        source: src(b"repo"),
        brygge_version: "0.1.0".into(),
        decoder: "brygge-decode-git".into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
    }
}

fn src(atom: &[u8]) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"repo".to_vec(),
        atom_id: atom.to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

fn draft(
    parents: Vec<AtomId>,
    source_atom: &[u8],
    ops: Vec<PathOp>,
    copies: Vec<CopyRecord>,
) -> AtomDraft {
    AtomDraft {
        parents,
        ops,
        copies,
        metadata: MetadataClaims {
            author: Some(Identity {
                name: Text::utf8("A"),
                email: Some(Text::utf8("a@x")),
            }),
            message: Some(Text::utf8("m")),
            ..MetadataClaims::default()
        },
        source: src(source_atom),
        status: EpistemicStatus::Stated,
    }
}

fn build() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"content".to_vec());
    let root = b
        .add_atom(draft(
            vec![],
            b"aaa",
            vec![PathOp::Add {
                path: "readme".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            vec![],
        ))
        .unwrap();
    let child_blob = b.add_blob(b"content2".to_vec());
    let _child = b
        .add_atom(draft(
            vec![root],
            b"bbb",
            vec![PathOp::Modify {
                path: "docs/readme".into(),
                blob: child_blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            vec![CopyRecord {
                from: "readme".into(),
                from_atom: root,
                to: "docs/readme".into(),
                status: EpistemicStatus::Derived(Derivation {
                    kind: DerivationKind::InferredRename,
                    by: "brygge-decode-git".into(),
                    decoder_version: "0.1.0".into(),
                    params: BTreeMap::new(),
                    confidence: Some(90),
                }),
            }],
        ))
        .unwrap();
    b.add_ref(RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: root,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
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
        annotation: None,
    });
    assert!(err.is_err());
}

#[test]
fn two_ops_on_one_path_is_rejected() {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"x".to_vec());
    let err = b.add_atom(draft(
        vec![],
        b"aaa",
        vec![
            PathOp::Add {
                path: "a".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            },
            PathOp::Delete {
                path: "a".into(),
                status: EpistemicStatus::Stated,
            },
        ],
        vec![],
    ));
    assert!(err.is_err());
}

#[test]
fn a_copy_from_an_atom_not_yet_added_is_rejected() {
    let mut b = IrBuilder::new(provenance());
    let err = b.add_atom(draft(
        vec![],
        b"aaa",
        vec![],
        vec![CopyRecord {
            from: "x".into(),
            from_atom: AtomId([9u8; 32]),
            to: "y".into(),
            status: EpistemicStatus::Stated,
        }],
    ));
    assert!(err.is_err());
}

#[test]
fn finish_prunes_a_blob_no_op_references() {
    let mut b = IrBuilder::new(provenance());
    let referenced = b.add_blob(b"kept".to_vec());
    let _orphan = b.add_blob(b"never referenced".to_vec());
    b.add_atom(draft(
        vec![],
        b"aaa",
        vec![PathOp::Add {
            path: "a".into(),
            blob: referenced,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        vec![],
    ))
    .unwrap();
    let ir = b.finish().unwrap();
    assert_eq!(ir.content.len(), 1);
    assert!(ir.content.contains(&referenced));
}

#[test]
fn blob_reads_back_what_add_blob_stored_and_nothing_else() {
    let mut b = IrBuilder::new(provenance());
    let id = b.add_blob(b"stored".to_vec());
    assert_eq!(b.blob(&id), Some(&b"stored"[..]));
    assert_eq!(b.blob(&BlobId::of(b"never added")), None);
    // Adding the same bytes again is the same blob (idempotent), and the accessor changes nothing.
    assert_eq!(b.add_blob(b"stored".to_vec()), id);
    assert_eq!(b.blob(&id), Some(&b"stored"[..]));
}

#[test]
fn finish_sorts_flags_canonically() {
    let mut b = IrBuilder::new(provenance());
    b.add_flag(Flag {
        kind: FlagKind::ConventionViolation,
        what: "z".into(),
        count: 1,
        reason: "r".into(),
    });
    b.add_flag(Flag {
        kind: FlagKind::BelowConfidenceFloor,
        what: "a".into(),
        count: 1,
        reason: "r".into(),
    });
    b.add_flag(Flag {
        kind: FlagKind::ConventionViolation,
        what: "a".into(),
        count: 1,
        reason: "r".into(),
    });
    let ir = b.finish().unwrap();
    let whats: Vec<&str> = ir.flags.iter().map(|f| f.what.as_str()).collect();
    // Sorted by (kind variant, what): ConventionViolation/a, ConventionViolation/z, BelowConfidenceFloor/a.
    assert_eq!(whats, vec!["a", "z", "a"]);
    assert_eq!(ir.flags[0].kind, FlagKind::ConventionViolation);
    assert_eq!(ir.flags[1].kind, FlagKind::ConventionViolation);
    assert_eq!(ir.flags[2].kind, FlagKind::BelowConfidenceFloor);
}

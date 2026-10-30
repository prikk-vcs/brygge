//! Tests for [`super::Ir::restamped`] (review 106 V-1).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use crate::ImportProvenance;
use crate::artifact::{digest_of, from_bytes, to_bytes};
use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{
    ChangedPart, CopyRecord, Ir, MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity,
    SourceKind, Text, TransformRecord,
};
use crate::status::{Derivation, DerivationKind, EpistemicStatus};

const DECODER: &str = "brygge-decode-git";

fn provenance(brygge_version: &str, decoder_version: &str) -> ImportProvenance {
    ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Git,
            repo_id: b"repo".to_vec(),
            atom_id: b"root".to_vec(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        brygge_version: brygge_version.into(),
        decoder: DECODER.into(),
        decoder_version: decoder_version.into(),
        params: BTreeMap::new(),
    }
}

fn src(atom: &str) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"repo".to_vec(),
        atom_id: atom.as_bytes().to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

fn derivation(version: &str) -> Derivation {
    Derivation {
        kind: DerivationKind::InferredRename,
        by: DECODER.into(),
        decoder_version: version.into(),
        params: BTreeMap::new(),
        confidence: Some(100),
    }
}

/// A two-atom, one-ref fixture built at version `v`, rich enough to exercise every place a
/// `Derivation` can live: a `Derived` atom (`root`), a `Derived` op and a `Derived` copy on `child`
/// (nested one level inside a `Transformed` status, to prove priors are walked too), and a `Derived`
/// ref. One `Derivation` has `by` set to something else entirely, to prove restamping leaves another
/// decoder's work alone.
fn fixture(v: &str) -> Ir {
    let mut b = IrBuilder::new(provenance(v, v));
    b.add_transform(TransformRecord {
        name: "a-transform".into(),
        version: "1".into(),
        params: BTreeMap::new(),
        consumed: [0u8; 32],
    });
    let blob_a = b.add_blob(b"aaa".to_vec()).unwrap();
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                blob: blob_a,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(Text::utf8("root")),
                ..MetadataClaims::default()
            },
            source: src("c1"),
            status: EpistemicStatus::Derived(derivation(v)),
        })
        .unwrap();
    let blob_b = b.add_blob(b"bbb".to_vec()).unwrap();
    let blob_c = b.add_blob(b"ccc".to_vec()).unwrap();
    let foreign = Derivation {
        kind: DerivationKind::Other("x".into()),
        by: "someone-else".into(),
        decoder_version: "unrelated".into(),
        params: BTreeMap::new(),
        confidence: None,
    };
    let child = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![
                PathOp::Add {
                    path: "b".into(),
                    blob: blob_b,
                    mode: 0o100_644,
                    status: EpistemicStatus::Transformed {
                        step: 0,
                        changed: vec![ChangedPart::Blob],
                        prior: Box::new(EpistemicStatus::Derived(derivation(v))),
                    },
                },
                PathOp::Add {
                    path: "d".into(),
                    blob: blob_c,
                    mode: 0o100_644,
                    status: EpistemicStatus::Derived(foreign),
                },
            ],
            copies: vec![CopyRecord {
                from: "a".into(),
                from_atom: root,
                to: "copy-of-a".into(),
                status: EpistemicStatus::Derived(derivation(v)),
            }],
            metadata: MetadataClaims {
                message: Some(Text::utf8("child")),
                ..MetadataClaims::default()
            },
            source: src("c2"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: child,
        status: EpistemicStatus::Derived(derivation(v)),
        source: None,
        annotation: None,
    })
    .unwrap();
    b.finish().unwrap()
}

fn all_derivations(ir: &Ir, mut f: impl FnMut(&Derivation)) {
    fn walk(status: &EpistemicStatus, f: &mut impl FnMut(&Derivation)) {
        match status {
            EpistemicStatus::Stated => {}
            EpistemicStatus::Derived(d) => f(d),
            EpistemicStatus::Transformed { prior, .. } => walk(prior, f),
        }
    }
    for atom in &ir.atoms {
        walk(&atom.status, &mut f);
        for op in &atom.ops {
            walk(op.status(), &mut f);
        }
        for copy in &atom.copies {
            walk(&copy.status, &mut f);
        }
    }
    for r in &ir.refs {
        walk(&r.status, &mut f);
    }
}

#[test]
fn restamping_sets_provenance_and_every_matching_derivation() {
    let ir = fixture("0.6.0");
    let restamped = ir.restamped("0.7.0", "0.7.0").unwrap();

    assert_eq!(restamped.provenance.brygge_version, "0.7.0");
    assert_eq!(restamped.provenance.decoder_version, "0.7.0");

    let mut matching = 0;
    let mut foreign = 0;
    all_derivations(&restamped, |d| {
        if d.by == DECODER {
            assert_eq!(
                d.decoder_version, "0.7.0",
                "every brygge-decode-git derivation is restamped"
            );
            matching += 1;
        } else {
            assert_eq!(
                d.decoder_version, "unrelated",
                "another decoder's own work is left alone"
            );
            foreign += 1;
        }
    });
    // root's atom status, child's op (through the Transformed prior), and child's copy, and the ref:
    // four `DECODER`-attributed derivations, plus the one foreign one on child's other op.
    assert_eq!(matching, 4);
    assert_eq!(foreign, 1);
}

#[test]
fn restamping_is_a_no_op_byte_for_byte_on_equal_versions() {
    let ir = fixture("0.6.0");
    let before = to_bytes(&ir);
    let restamped = ir.restamped("0.6.0", "0.6.0").unwrap();
    let after = to_bytes(&restamped);
    assert_eq!(
        before, after,
        "restamping to the same versions changes no byte"
    );
}

#[test]
fn restamping_is_idempotent() {
    let once = fixture("0.6.0").restamped("0.7.0", "0.7.0").unwrap();
    let twice = fixture("0.6.0")
        .restamped("0.7.0", "0.7.0")
        .unwrap()
        .restamped("0.7.0", "0.7.0")
        .unwrap();
    assert_eq!(to_bytes(&once), to_bytes(&twice));
}

#[test]
fn restamping_round_trips_and_its_digest_matches_a_direct_write() {
    let restamped = fixture("0.6.0").restamped("0.7.0", "0.7.0").unwrap();
    let expected_digest = digest_of(&restamped).unwrap();
    let bytes = to_bytes(&restamped);
    let decoded = from_bytes(&bytes).expect("a restamped Ir round-trips through the writer/reader");
    assert_eq!(decoded.digest, expected_digest);
}

#[test]
fn restamping_the_same_ir_twice_to_different_versions_gives_different_bytes() {
    let base = fixture("0.6.0");
    // `restamped` consumes `self`, so build two independent copies from the same recipe.
    let a = fixture("0.6.0").restamped("0.7.0", "0.7.0").unwrap();
    let b = fixture("0.6.0").restamped("0.8.0", "0.8.0").unwrap();
    assert_ne!(to_bytes(&a), to_bytes(&b));
    // Sanity: the un-restamped original differs from both too.
    assert_ne!(to_bytes(&base), to_bytes(&a));
}

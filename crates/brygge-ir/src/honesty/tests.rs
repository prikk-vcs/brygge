//! Tests for the fidelity report (RFC 002 D-3, FS-02, report v2 per RFC 011 §3).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{
    CopyRecord, DropRecord, Flag, FlagKind, ImportProvenance, LossBoundary, LossClass,
    MetadataClaims, PathOp, SourceIdentity, SourceKind,
};
use crate::status::{Derivation, DerivationKind, EpistemicStatus};

fn src(atom: &[u8]) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"r".to_vec(),
        atom_id: atom.to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

fn provenance() -> ImportProvenance {
    ImportProvenance {
        source: src(b"r"),
        brygge_version: "0.1.0".into(),
        decoder: "d".into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
    }
}

fn build() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"x".to_vec());
    let derived_copy = EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::InferredRename,
        by: "d".into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
        confidence: Some(75),
    });
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let child_blob = b.add_blob(b"y".to_vec());
    let _child = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "b".into(),
                blob: child_blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![CopyRecord {
                from: "a".into(),
                from_atom: root,
                to: "b".into(),
                status: derived_copy,
            }],
            metadata: MetadataClaims::default(),
            // an atom-level derived status too (a reconstructed changeset)
            source: src(b"c2"),
            status: EpistemicStatus::Derived(Derivation {
                kind: DerivationKind::ReconstructedChangeset,
                by: "d".into(),
                decoder_version: "0.1.0".into(),
                params: BTreeMap::new(),
                confidence: None,
            }),
        })
        .unwrap();
    b.set_loss(LossBoundary {
        dropped: vec![DropRecord {
            class: LossClass::AdvisoryUnreliable,
            what: "mergeinfo".into(),
            reason: "advisory and frequently wrong".into(),
        }],
    });
    b.add_flag(Flag {
        kind: FlagKind::ConventionViolation,
        what: "trunk/branches/tags layout".into(),
        count: 2,
        reason: "no matching branch convention".into(),
    });
    b.finish().unwrap()
}

#[test]
fn summary_counts_derived_and_dropped_and_flagged_by_kind() {
    let ir = build();
    let r = summary(&ir);
    assert_eq!(r.atoms, 2);
    assert_eq!(r.blobs, 2);
    assert_eq!(r.content_bytes, 2);
    assert_eq!(r.derived.get("inferred-rename"), Some(&1));
    assert_eq!(r.derived.get("reconstructed-changeset"), Some(&1));
    assert_eq!(r.dropped.get("advisory-unreliable"), Some(&1));
    assert_eq!(r.flagged.get("convention-violation"), Some(&2));
    assert_eq!(r.report_version, 2);
}

#[test]
fn summary_is_pure_and_render_is_deterministic() {
    let ir = build();
    assert_eq!(summary(&ir), summary(&ir));
    assert_eq!(summary(&ir).render_machine(), summary(&ir).render_machine());
    // Authorship is always shown Unverifiable in the human form (VF-4/HO-3).
    assert!(summary(&ir).render_human().contains("Unverifiable"));
}

#[test]
fn machine_form_includes_flagged_lines() {
    let machine = summary(&build()).render_machine();
    assert!(machine.contains("flagged.convention-violation=2"));
}

#[test]
fn human_form_shows_flagged_section_only_when_non_empty() {
    let with_flag = summary(&build()).render_human();
    assert!(with_flag.contains("flagged:"));
    assert!(with_flag.contains("convention-violation: 2"));

    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"z".to_vec());
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let without_flag = summary(&b.finish().unwrap()).render_human();
    assert!(!without_flag.contains("flagged:"));
}

#[test]
fn a_clean_import_reports_no_derivations() {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"y".to_vec());
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let r = summary(&b.finish().unwrap());
    assert!(r.derived.is_empty());
    assert!(
        r.render_human()
            .contains("every assertion is source-stated")
    );
}

#[test]
fn representation_drops_appear_only_on_the_not_history_line() {
    // `build()` records one AdvisoryUnreliable drop and no Representation drop.
    let human = summary(&build()).render_human();
    assert!(human.contains("not history (no content or claim lost): 0"));
    assert!(human.contains("dropped (recorded loss):"));
    assert!(human.contains("advisory-unreliable: 1"));
    // The representation label must never appear inside the "recorded loss" section itself.
    let recorded_loss_section = human
        .split("dropped (recorded loss):")
        .nth(1)
        .expect("section present");
    assert!(!recorded_loss_section.contains("representation"));
}

#[test]
fn a_representation_drop_is_counted_on_the_not_history_line_not_recorded_loss() {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"z".to_vec());
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.set_loss(LossBoundary {
        dropped: vec![DropRecord {
            class: LossClass::Representation,
            what: "reflogs".into(),
            reason: "local operation log, not history".into(),
        }],
    });
    let human = summary(&b.finish().unwrap()).render_human();
    assert!(human.contains("not history (no content or claim lost): 1"));
    assert!(human.contains("dropped (recorded loss): nothing"));
}

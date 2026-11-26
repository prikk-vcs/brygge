//! Tests for the fidelity report (RFC 002 D-3, FS-02, report v2 per RFC 011 §3).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{
    Content, CopyRecord, DropRecord, Flag, FlagKind, ImportProvenance, LossBoundary, LossClass,
    MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity, SourceKind,
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
    let blob = b.add_blob(b"x".to_vec()).unwrap();
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
                content: Content::blob(blob, 0o100_644),
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let child_blob = b.add_blob(b"y".to_vec()).unwrap();
    let _child = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "b".into(),
                content: Content::blob(child_blob, 0o100_644),
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
            step: None,
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
    assert_eq!(r.report_version, 3);
    assert_eq!(r.skipped_non_critical_fields, 0);
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
    // Part-2 handoff §5.1: a label embedded in a key is snake_case; the labels themselves (values, and
    // the grouping keys inside `FidelityReport`) stay kebab-case.
    assert!(machine.contains("flagged.convention_violation=2"));
    assert!(machine.contains("derived.inferred_rename=1"));
    assert!(machine.contains("derived.reconstructed_changeset=1"));
    assert!(machine.contains("dropped.advisory_unreliable=1"));
    assert!(
        !machine.contains('-'),
        "no kebab-case remains in any key:\n{machine}"
    );
}

#[test]
fn skipped_non_critical_fields_is_always_printed_and_settable_without_touching_summary() {
    let ir = build();
    // `summary` is a pure function of the Ir: it cannot know how the artifact was read.
    assert_eq!(summary(&ir).skipped_non_critical_fields, 0);
    assert!(
        summary(&ir)
            .render_machine()
            .lines()
            .any(|l| l == "skipped_non_critical_fields=0")
    );
    let with = summary(&ir).with_skipped_non_critical_fields(3);
    assert!(
        with.render_machine()
            .lines()
            .any(|l| l == "skipped_non_critical_fields=3")
    );
    // and it is only that one line that changes.
    let changed: Vec<String> = with
        .render_machine()
        .lines()
        .map(|l| {
            if l.starts_with("skipped_non_critical_fields=") {
                "skipped_non_critical_fields=0".to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    let plain: Vec<String> = summary(&ir)
        .render_machine()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(changed, plain);
}

#[test]
fn human_form_shows_flagged_section_only_when_non_empty() {
    let with_flag = summary(&build()).render_human();
    assert!(with_flag.contains("flagged:"));
    assert!(with_flag.contains("convention-violation: 2"));

    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"z".to_vec()).unwrap();
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                content: Content::blob(blob, 0o100_644),
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
    let blob = b.add_blob(b"y".to_vec()).unwrap();
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                content: Content::blob(blob, 0o100_644),
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
    let blob = b.add_blob(b"z".to_vec()).unwrap();
    let _ = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                content: Content::blob(blob, 0o100_644),
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
            step: None,
        }],
    });
    let human = summary(&b.finish().unwrap()).render_human();
    assert!(human.contains("not history (no content or claim lost): 1"));
    assert!(human.contains("dropped (recorded loss): nothing"));
}

#[test]
fn an_import_with_no_submodule_omits_the_sentence_and_the_machine_keys_entirely() {
    // `build()` holds no submodule at all — RFC 021 D-4's report lines must be byte-absent, not
    // present with a zero count, so an ordinary repository's report is unchanged by this RFC.
    let r = summary(&build());
    assert_eq!(r.submodule_paths, 0);
    assert_eq!(r.submodule_at_tips, 0);
    assert!(!r.render_human().contains("submodules:"));
    assert!(!r.render_machine().contains("submodules"));
    assert!(!r.render_machine().contains("decode_submodules_version"));
}

#[test]
fn summary_counts_a_submodule_present_at_a_ref_target_in_both_history_and_at_tips() {
    let mut b = IrBuilder::new(provenance());
    let readme_blob = b.add_blob(b"hi".to_vec()).unwrap();
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "readme".into(),
                content: Content::blob(readme_blob, 0o100_644),
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let with_sub = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "sub".into(),
                content: Content::submodule(vec![0xab; 20]),
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c2"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "main".into(),
        kind: RefKind::Branch,
        target: with_sub,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    let r = summary(&b.finish().unwrap());
    assert_eq!(r.submodule_paths, 1);
    assert_eq!(r.submodule_at_tips, 1);
    let human = r.render_human();
    assert!(human.contains(
        "submodules: 1 path(s) in the history, 1 at the branch and tag tips; their own \
         repositories were not read, imported or verified"
    ));
    let machine = r.render_machine();
    assert!(machine.contains("decode_submodules_version=1"));
    assert!(machine.contains("decode.submodules.paths=1"));
    assert!(machine.contains("decode.submodules.at_tips=1"));
}

#[test]
fn summary_counts_a_removed_submodule_in_history_but_not_at_tips() {
    // A submodule added, then removed before any ref's own target: it is part of the history (`P`),
    // but holds no ref's tip (`T`) — the two counts must disagree here, or they would be the same
    // field twice under different names.
    let mut b = IrBuilder::new(provenance());
    let added = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "sub".into(),
                content: Content::submodule(vec![0xcd; 20]),
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let removed = b
        .add_atom(AtomDraft {
            parents: vec![added],
            ops: vec![PathOp::Delete {
                path: "sub".into(),
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src(b"c2"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "main".into(),
        kind: RefKind::Branch,
        target: removed,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    let r = summary(&b.finish().unwrap());
    assert_eq!(r.submodule_paths, 1, "it was in the history once");
    assert_eq!(r.submodule_at_tips, 0, "but it holds no ref's tip");
}

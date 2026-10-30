//! Tests for `brygge_ir::transform` (RFC 018 D-6): `check`'s refusals, one per rule, and `Edit`'s own
//! round trip through `check`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::artifact::{Decoded, digest_of, from_bytes, to_bytes};
use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{
    Annotation, ChangedPart, CopyRecord, DropRecord, Flag, FlagKind, Identity, ImportProvenance,
    Ir, LossBoundary, LossClass, MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity,
    SourceKind, Text, TransformRecord,
};
use crate::status::{Derivation, DerivationKind, EpistemicStatus};

fn provenance() -> ImportProvenance {
    ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Git,
            repo_id: b"repo".to_vec(),
            atom_id: b"root".to_vec(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        brygge_version: "0.1.0".into(),
        decoder: "brygge-decode-git".into(),
        decoder_version: "0.1.0".into(),
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

/// A two-atom, one-ref fixture: `root` (adds `a`), `child` (adds `b`, parent `root`), a branch ref at
/// `child`. Rich enough to exercise atoms/ops/refs/parents all at once.
fn decoded_fixture() -> Decoded {
    let mut b = IrBuilder::new(provenance());
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
                author: Some(Identity {
                    name: Text::utf8("Alice"),
                    email: Some(Text::utf8("alice@example.com")),
                }),
                message: Some(Text::utf8("root")),
                ..MetadataClaims::default()
            },
            source: src("c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let blob_b = b.add_blob(b"bbb".to_vec()).unwrap();
    let child = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "b".into(),
                blob: blob_b,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
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
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    let ir = b.finish().unwrap();
    from_bytes(&to_bytes(&ir)).unwrap()
}

/// Like [`decoded_fixture`], but richer: three atoms (a chain), a copy record, and an annotated tag —
/// exercises every `check` rule at once (used for `Edit`'s property-style test).
fn richer_decoded_fixture() -> Decoded {
    let mut b = IrBuilder::new(provenance());
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
                author: Some(Identity {
                    name: Text::utf8("Alice"),
                    email: Some(Text::utf8("alice@example.com")),
                }),
                message: Some(Text::utf8("root")),
                ..MetadataClaims::default()
            },
            source: src("c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let blob_b = b.add_blob(b"bbb".to_vec()).unwrap();
    let mid = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "b".into(),
                blob: blob_b,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(Text::utf8("mid")),
                ..MetadataClaims::default()
            },
            source: src("c2"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let blob_c = b.add_blob(b"ccc".to_vec()).unwrap();
    let tip = b
        .add_atom(AtomDraft {
            parents: vec![mid],
            ops: vec![PathOp::Add {
                path: "c".into(),
                blob: blob_c,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![CopyRecord {
                from: "a".into(),
                from_atom: root,
                to: "a-copy".into(),
                status: EpistemicStatus::Derived(Derivation {
                    kind: DerivationKind::InferredRename,
                    by: "brygge-decode-git".into(),
                    decoder_version: "0.1.0".into(),
                    params: BTreeMap::from([
                        ("rename_algorithm".to_string(), "similarity".to_string()),
                        ("rename_threshold".to_string(), "50".to_string()),
                    ]),
                    confidence: Some(80),
                }),
            }],
            metadata: MetadataClaims {
                message: Some(Text::utf8("tip")),
                ..MetadataClaims::default()
            },
            source: src("c3"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: tip,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    b.add_ref(RefRecord {
        name: "v1".into(),
        kind: RefKind::Tag,
        target: mid,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: Some(Annotation {
            tagger: None,
            time: None,
            message: Some(Text::utf8("release")),
        }),
    })
    .unwrap();
    b.add_flag(Flag {
        kind: FlagKind::BelowConfidenceFloor,
        what: "an example flag".into(),
        count: 1,
        reason: "fixture".into(),
    });
    let ir = b.finish().unwrap();
    from_bytes(&to_bytes(&ir)).unwrap()
}

#[test]
fn an_honest_author_change_via_edit_passes_check() {
    let input = decoded_fixture();
    let mut edit = Edit::begin(&input, "authors-fixer", "1.0.0", BTreeMap::new());
    edit.atom_mut(0).unwrap().set_author(Some(Identity {
        name: Text::utf8("A. Alias"),
        email: Some(Text::utf8("alias@example.com")),
    }));
    let output_ir = edit.finish().unwrap();
    let output = from_bytes(&to_bytes(&output_ir)).unwrap();
    let summary = check(&input, &output, "authors-fixer").unwrap();
    assert_eq!(summary.step, 0);
    assert_eq!(summary.atoms_changed, 1);
    assert_eq!(summary.ops_changed, 0);
    assert_eq!(summary.refs_changed, 0);
    assert_eq!(summary.dropped, 0);
}

#[test]
fn an_honest_no_op_edit_passes_check_as_a_zero_change_step() {
    let input = decoded_fixture();
    let edit = Edit::begin(&input, "no-op", "1.0.0", BTreeMap::new());
    let output_ir = edit.finish().unwrap();
    let output = from_bytes(&to_bytes(&output_ir)).unwrap();
    let summary = check(&input, &output, "no-op").unwrap();
    assert_eq!(
        summary,
        StepSummary {
            step: 0,
            atoms_changed: 0,
            ops_changed: 0,
            refs_changed: 0,
            dropped: 0,
        }
    );
}

#[test]
fn removing_a_middle_atom_gives_its_child_the_documented_parents() {
    let input = richer_decoded_fixture();
    let mut edit = Edit::begin(&input, "squash", "1.0.0", BTreeMap::new());
    edit.remove_atom(1, "squashed into root").unwrap();
    // `v1` targets the atom being removed; `Edit` has no implicit retargeting, so the ref must go too.
    edit.remove_ref(RefKind::Tag, "v1", "its target was squashed")
        .unwrap();
    let output_ir = edit.finish().unwrap();
    let output = from_bytes(&to_bytes(&output_ir)).unwrap();
    let summary = check(&input, &output, "squash").unwrap();
    assert_eq!(
        summary.atoms_changed, 1,
        "tip's parents changed, so it must be marked"
    );
    assert_eq!(
        summary.dropped, 2,
        "the removed atom and the ref that targeted it"
    );

    let tip = output
        .ir
        .atoms
        .iter()
        .find(|a| a.source == src("c3"))
        .unwrap();
    let root_out = output
        .ir
        .atoms
        .iter()
        .find(|a| a.source == src("c1"))
        .unwrap();
    assert_eq!(tip.parents, vec![root_out.id]);
    match &tip.status {
        EpistemicStatus::Transformed { changed, .. } => {
            assert_eq!(changed, &vec![ChangedPart::Parents]);
        }
        other => panic!("expected Transformed, got {other:?}"),
    }
}

#[test]
fn changed_parents_left_unmarked_is_refused() {
    let input = richer_decoded_fixture();
    let mut edit = Edit::begin(&input, "t", "1.0.0", BTreeMap::new());
    edit.remove_atom(1, "squashed").unwrap();
    edit.remove_ref(RefKind::Tag, "v1", "its target was squashed")
        .unwrap();
    let mut ir = edit.finish().unwrap();
    // Tip's parents genuinely changed (mid -> root); un-mark it, pretending they did not.
    let tip = ir.atoms.iter_mut().find(|a| a.source == src("c3")).unwrap();
    if let EpistemicStatus::Transformed { prior, .. } = tip.status.clone() {
        tip.status = *prior;
    }
    let tip_index = ir.atoms.iter().position(|a| a.source == src("c3")).unwrap();
    fix_id(&mut ir, tip_index);
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn edit_property_a_sequence_of_edits_on_the_richest_fixture_always_passes_check() {
    let input = richer_decoded_fixture();
    let mut edit = Edit::begin(&input, "stress", "1.0.0", BTreeMap::new());
    edit.atom_mut(0)
        .unwrap()
        .set_message(Some(Text::utf8("root, renamed")));
    let new_blob = edit.add_blob(b"zzz".to_vec());
    edit.atom_mut(1)
        .unwrap()
        .set_op_blob("b", new_blob)
        .unwrap();
    edit.atom_mut(2)
        .unwrap()
        .remove_copy("a-copy", "no longer needed")
        .unwrap();
    edit.rename_ref(RefKind::Tag, "v1", "v1.0").unwrap();
    let output_ir = edit.finish().unwrap();
    let output = from_bytes(&to_bytes(&output_ir)).unwrap();
    let summary = check(&input, &output, "stress").unwrap();
    assert!(summary.atoms_changed >= 2);
    assert!(summary.ops_changed >= 1);
    assert!(summary.refs_changed >= 1);
    assert_eq!(summary.dropped, 1);
}

// ---- `check`'s refusals (RFC 018 D-6, §6 item 2) ------------------------------------------------

/// Builds `output` from `input` by replacing its decoded `Ir` with `mutate`'s result, then round-
/// trips it through the real writer/reader (so `check` always sees a genuinely well-formed `Decoded`,
/// matching what `verify`'s internal checks would have already passed by the time `transform::check`
/// runs in the real CLI).
fn with_output(_input: &Decoded, ir: Ir) -> Decoded {
    from_bytes(&to_bytes(&ir)).unwrap()
}

/// An honest step 0 over `input`, as a starting point for refusal tests to then corrupt by hand.
fn honest_step(input: &Decoded, name: &str) -> Ir {
    Edit::begin(input, name, "1.0.0", BTreeMap::new())
        .finish()
        .unwrap()
}

/// After hand-mutating one atom's own content (status/metadata) in a refusal test, its stored `id` (a
/// hash over that same content) must be recomputed — and since `AtomId` is itself part of every
/// *later* atom's own content (via `parents`/`copies`), relinking one id can make every atom after it
/// stale too, cascading. Recomputes every atom's id, in order, relinking each one's old id to its new
/// one in every later atom/ref before moving on — or `from_bytes` refuses the result at decode time
/// (a referential-integrity or self-consistency error) before `check` ever gets a chance to run.
fn fix_id(ir: &mut Ir, atom_index: usize) {
    for i in atom_index..ir.atoms.len() {
        let old_id = ir.atoms[i].id;
        let new_id = ir.atoms[i].compute_id();
        if old_id == new_id {
            continue;
        }
        ir.atoms[i].id = new_id;
        for atom in &mut ir.atoms {
            for p in &mut atom.parents {
                if *p == old_id {
                    *p = new_id;
                }
            }
            for c in &mut atom.copies {
                if c.from_atom == old_id {
                    c.from_atom = new_id;
                }
            }
        }
        for rf in &mut ir.refs {
            if rf.target == old_id {
                rf.target = new_id;
            }
        }
    }
}

#[test]
fn an_unmarked_author_change_is_refused() {
    let input = decoded_fixture();
    let mut ir = honest_step(&input, "t");
    ir.atoms[0].metadata.author = Some(Identity {
        name: Text::utf8("Mallory"),
        email: None,
    });
    fix_id(&mut ir, 0);
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn a_mark_naming_an_unchanged_part_is_refused() {
    let input = decoded_fixture();
    let mut ir = honest_step(&input, "t");
    ir.atoms[0].status = EpistemicStatus::Transformed {
        step: 0,
        changed: vec![ChangedPart::Message],
        prior: Box::new(EpistemicStatus::Stated),
    };
    fix_id(&mut ir, 0);
    let output = with_output(&input, ir);
    match check(&input, &output, "t") {
        Err(_) => {}
        Ok(s) => panic!("expected a refusal, got {s:?}"),
    }
}

#[test]
fn a_missing_part_in_changed_is_refused() {
    let input = decoded_fixture();
    let mut edit = Edit::begin(&input, "t", "1.0.0", BTreeMap::new());
    edit.atom_mut(0).unwrap().set_author(Some(Identity {
        name: Text::utf8("A"),
        email: None,
    }));
    edit.atom_mut(0)
        .unwrap()
        .set_message(Some(Text::utf8("also changed")));
    let mut ir = edit.finish().unwrap();
    // Strip `message` from `changed`, leaving only `author` — a missing part.
    if let EpistemicStatus::Transformed { changed, .. } = &mut ir.atoms[0].status {
        changed.retain(|c| *c != ChangedPart::Message);
    }
    fix_id(&mut ir, 0);
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn a_wrong_prior_is_refused() {
    let input = decoded_fixture();
    let mut edit = Edit::begin(&input, "t", "1.0.0", BTreeMap::new());
    edit.atom_mut(0).unwrap().set_author(Some(Identity {
        name: Text::utf8("A"),
        email: None,
    }));
    let mut ir = edit.finish().unwrap();
    let EpistemicStatus::Transformed { prior, .. } = &mut ir.atoms[0].status else {
        panic!("expected the author edit to mark atom 0 Transformed");
    };
    **prior = EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::NormalizedMetadata,
        by: "x".into(),
        decoder_version: "0".into(),
        params: BTreeMap::new(),
        confidence: None,
    });
    fix_id(&mut ir, 0);
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn an_invented_atom_is_refused() {
    let input = decoded_fixture();
    let mut b = IrBuilder::new(input.ir.provenance.clone());
    for id in input.ir.content.ids() {
        b.add_blob(input.ir.content.get(id).unwrap().unwrap())
            .unwrap();
    }
    for atom in &input.ir.atoms {
        b.add_atom(AtomDraft {
            parents: atom.parents.clone(),
            ops: atom.ops.clone(),
            copies: atom.copies.clone(),
            metadata: atom.metadata.clone(),
            source: atom.source.clone(),
            status: atom.status.clone(),
        })
        .unwrap();
    }
    let blob = b.add_blob(b"invented".to_vec()).unwrap();
    let last = *input.ir.atoms.last().map(|a| &a.id).unwrap();
    let invented = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "invented".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: src("invented-atom"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let _ = (last, invented);
    for rf in &input.ir.refs {
        b.add_ref(rf.clone()).unwrap();
    }
    b.add_transform(TransformRecord {
        name: "t".into(),
        version: "1.0.0".into(),
        params: BTreeMap::new(),
        consumed: input.digest,
    });
    let ir = b.finish().unwrap();
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn an_invented_op_is_refused() {
    let input = decoded_fixture();
    let mut ir = honest_step(&input, "t");
    let blob = BlobId::of(b"invented");
    ir.content.insert(b"invented".to_vec()).unwrap();
    ir.atoms[0].ops.push(PathOp::Add {
        path: "invented".into(),
        blob,
        mode: 0o100_644,
        status: EpistemicStatus::Transformed {
            step: 0,
            changed: vec![ChangedPart::Path],
            prior: Box::new(EpistemicStatus::Stated),
        },
    });
    fix_id(&mut ir, 0);
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn a_gone_op_with_no_drop_is_refused() {
    let input = decoded_fixture();
    let mut edit = Edit::begin(&input, "t", "1.0.0", BTreeMap::new());
    edit.remove_op(0, "a", "legitimately gone").unwrap();
    let mut ir = edit.finish().unwrap();
    // `remove_op` recorded the drop honestly; strip it back out to simulate an op that went missing
    // with nothing recording it (`IrBuilder::finish` already pruned the now-unreferenced blob, so
    // nothing else needs cleaning up).
    ir.loss.dropped.clear();
    fix_id(&mut ir, 0);
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn an_atom_drop_for_an_atom_still_present_is_refused() {
    let input = decoded_fixture();
    let mut ir = honest_step(&input, "t");
    ir.loss.dropped.push(DropRecord {
        class: LossClass::Transform,
        what: format!("atom {}", input.ir.atoms[0].id.to_hex()),
        reason: "bogus".into(),
        step: Some(0),
    });
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn more_moved_ops_than_gone_ones_is_refused() {
    let input = decoded_fixture();
    let mut edit = Edit::begin(&input, "t", "1.0.0", BTreeMap::new());
    edit.atom_mut(0).unwrap().move_op("a", "a-moved").unwrap();
    let mut ir = edit.finish().unwrap();
    // Add a second "new" op with no corresponding gone one at all.
    let blob = BlobId::of(b"extra");
    ir.content.insert(b"extra".to_vec()).unwrap();
    ir.atoms[0].ops.push(PathOp::Add {
        path: "also-new".into(),
        blob,
        mode: 0o100_644,
        status: EpistemicStatus::Transformed {
            step: 0,
            changed: vec![ChangedPart::Path],
            prior: Box::new(EpistemicStatus::Stated),
        },
    });
    fix_id(&mut ir, 0);
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn a_wrong_consumed_is_refused() {
    let input = decoded_fixture();
    let mut edit = Edit::begin(&input, "t", "1.0.0", BTreeMap::new());
    edit.atom_mut(0).unwrap().set_message(Some(Text::utf8("x")));
    let mut ir = edit.finish().unwrap();
    ir.transforms[0].consumed = [0u8; 32];
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn a_wrong_name_is_refused() {
    let input = decoded_fixture();
    let ir = honest_step(&input, "t");
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "a-different-name").is_err());
}

#[test]
fn two_new_records_at_once_is_refused() {
    let input = decoded_fixture();
    let mut ir = honest_step(&input, "t");
    ir.transforms.push(TransformRecord {
        name: "t2".into(),
        version: "1.0.0".into(),
        params: BTreeMap::new(),
        consumed: digest_of(&ir).unwrap(),
    });
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn a_changed_provenance_is_refused() {
    let input = decoded_fixture();
    let mut ir = honest_step(&input, "t");
    ir.provenance.decoder_version = "9.9.9".into();
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn a_changed_flag_is_refused() {
    let input = richer_decoded_fixture();
    let mut ir = honest_step(&input, "t");
    ir.flags[0].count = 99;
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn an_edited_earlier_drop_is_refused() {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"x".to_vec()).unwrap();
    b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "a".into(),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        copies: vec![],
        metadata: MetadataClaims::default(),
        source: src("c1"),
        status: EpistemicStatus::Stated,
    })
    .unwrap();
    b.set_loss(LossBoundary {
        dropped: vec![DropRecord {
            class: LossClass::Representation,
            what: "packfiles".into(),
            reason: "representation".into(),
            step: None,
        }],
    });
    let ir0 = b.finish().unwrap();
    let input = from_bytes(&to_bytes(&ir0)).unwrap();

    let mut ir = honest_step(&input, "t");
    ir.loss.dropped[0].reason = "edited".into();
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn a_ref_renamed_without_a_mark_is_refused() {
    let input = decoded_fixture();
    let mut ir = honest_step(&input, "t");
    ir.refs[0].name = "refs/heads/renamed".into();
    let output = with_output(&input, ir);
    assert!(check(&input, &output, "t").is_err());
}

#[test]
fn duplicate_source_identities_in_the_input_are_refused() {
    let mut b = IrBuilder::new(provenance());
    let blob1 = b.add_blob(b"a".to_vec()).unwrap();
    b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "a".into(),
            blob: blob1,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        copies: vec![],
        metadata: MetadataClaims::default(),
        source: src("same"),
        status: EpistemicStatus::Stated,
    })
    .unwrap();
    let blob2 = b.add_blob(b"b".to_vec()).unwrap();
    b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "b".into(),
            blob: blob2,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        copies: vec![],
        metadata: MetadataClaims::default(),
        source: src("same"), // duplicate on purpose
        status: EpistemicStatus::Stated,
    })
    .unwrap();
    let ir0 = b.finish().unwrap();
    let input = from_bytes(&to_bytes(&ir0)).unwrap();

    // `Edit` itself would refuse to touch this input at all (its own `finish` runs `check`, which
    // refuses first) — build a trivial "no real change" output directly, bypassing `Edit`, so this
    // test actually reaches `check`'s own duplicate-identity refusal rather than `Edit`'s.
    let mut b2 = IrBuilder::new(input.ir.provenance.clone());
    for id in input.ir.content.ids() {
        b2.add_blob(input.ir.content.get(id).unwrap().unwrap())
            .unwrap();
    }
    for atom in &input.ir.atoms {
        b2.add_atom(AtomDraft {
            parents: atom.parents.clone(),
            ops: atom.ops.clone(),
            copies: atom.copies.clone(),
            metadata: atom.metadata.clone(),
            source: atom.source.clone(),
            status: atom.status.clone(),
        })
        .unwrap();
    }
    b2.add_transform(TransformRecord {
        name: "t".into(),
        version: "1.0.0".into(),
        params: BTreeMap::new(),
        consumed: input.digest,
    });
    let output_ir = b2.finish().unwrap();
    let output = with_output(&input, output_ir);
    match check(&input, &output, "t") {
        Err(v) => assert!(v.to_string().contains("same source identity"), "{v}"),
        Ok(s) => panic!("expected a refusal, got {s:?}"),
    }
}

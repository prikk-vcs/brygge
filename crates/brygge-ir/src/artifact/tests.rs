//! Tests for the artifact codec: round-trip, determinism, integrity, versioning, canonical structure,
//! and panic-freedom on malformed input (RFC 011 §2.3/§2.5/§7/§10).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::builder::{AtomDraft, IrBuilder};
use crate::canon::CanonReader;
use crate::model::{
    DropRecord, Flag, FlagKind, ImportProvenance, LossBoundary, LossClass, MetadataClaims, PathOp,
    RefKind, RefRecord, SourceIdentity, SourceKind,
};
use crate::status::EpistemicStatus;

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

fn build() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"hello world".to_vec());
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "readme".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(crate::model::Text::utf8("initial")),
                ..MetadataClaims::default()
            },
            source: SourceIdentity {
                kind: SourceKind::Git,
                repo_id: b"repo".to_vec(),
                atom_id: b"c1".to_vec(),
                signatures: Vec::new(),
                extras: Vec::new(),
            },
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let child_blob = b.add_blob(b"second".to_vec());
    let _child = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "second".into(),
                blob: child_blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: SourceIdentity {
                kind: SourceKind::Git,
                repo_id: b"repo".to_vec(),
                atom_id: b"c2".to_vec(),
                signatures: Vec::new(),
                extras: Vec::new(),
            },
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.finish().unwrap()
}

#[test]
fn roundtrips() {
    let ir = build();
    let bytes = to_bytes(&ir);
    let decoded = from_bytes(&bytes).unwrap();
    assert_eq!(decoded.ir, ir);
    assert_eq!(decoded.skipped_non_critical_fields, 0);
}

#[test]
fn serialization_is_byte_deterministic() {
    let ir = build();
    assert_eq!(to_bytes(&ir), to_bytes(&ir));
}

#[test]
fn truncation_is_rejected_not_panicked() {
    let ir = build();
    let bytes = to_bytes(&ir);
    let cut = &bytes[..bytes.len() - 1];
    assert!(from_bytes(cut).is_err());
    assert!(from_bytes(b"BRYGGEIR").is_err());
    assert!(from_bytes(b"").is_err());
}

#[test]
fn bad_magic_is_rejected() {
    let mut bytes = to_bytes(&build());
    bytes[0] = b'X';
    assert!(matches!(from_bytes(&bytes), Err(Error::Decode(_))));
}

// ---- version gate (RFC 011 §2.3) -----------------------------------------------------------------

#[test]
fn format_1_is_a_pre_release_artifact_and_metadata_is_never_parsed() {
    // Garbage after the format byte: if metadata were ever touched, this would fail differently
    // (a Decode/NonCanonical error from the garbage), proving the read stops right after the format
    // byte.
    let mut bytes = MAGIC.to_vec();
    bytes.push(1);
    bytes.extend_from_slice(&[0xff; 64]);
    match from_bytes(&bytes) {
        Err(Error::PreReleaseArtifact) => {}
        other => panic!("expected PreReleaseArtifact, got {other:?}"),
    }
}

#[test]
fn format_1_with_no_trailing_bytes_at_all_is_still_a_pre_release_artifact() {
    let mut bytes = MAGIC.to_vec();
    bytes.push(1);
    match from_bytes(&bytes) {
        Err(Error::PreReleaseArtifact) => {}
        other => panic!("expected PreReleaseArtifact, got {other:?}"),
    }
}

fn container_with_version(ir: &Ir, major: u64, minor: u64, patch: u64) -> Vec<u8> {
    let mut version = crate::canon::CanonWriter::new();
    version.uvarint(major);
    version.uvarint(minor);
    version.uvarint(patch);

    let mut body = crate::canon::CanonWriter::new();
    let meta = ir.encode_metadata();
    body.uvarint(meta.len() as u64);
    body.raw(&meta);
    body.uvarint(ir.content.len() as u64);
    for (id, bytes) in ir.content.iter_sorted() {
        body.raw32(id.as_bytes());
        body.uvarint(bytes.len() as u64);
        body.raw(bytes);
    }

    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(body.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(body.as_bytes());
    out
}

#[test]
fn a_matching_patch_bump_is_accepted() {
    let ir = build();
    let bytes = container_with_version(&ir, 0, 2, 7);
    assert!(from_bytes(&bytes).is_ok());
}

#[test]
fn a_different_minor_under_major_zero_is_refused() {
    let ir = build();
    let bytes = container_with_version(&ir, 0, 3, 0);
    match from_bytes(&bytes) {
        Err(Error::UnsupportedContract { .. }) => {}
        other => panic!("expected UnsupportedContract, got {other:?}"),
    }
}

#[test]
fn a_newer_major_is_refused() {
    let ir = build();
    let bytes = container_with_version(&ir, 1, 0, 0);
    match from_bytes(&bytes) {
        Err(Error::UnsupportedContract { .. }) => {}
        other => panic!("expected UnsupportedContract, got {other:?}"),
    }
}

// ---- digest (RFC 011 §2.3/§7.5): one byte flipped in each section ---------------------------------

/// Locate `(digest_start, meta_start, blob_start)` in a `to_bytes` output, by re-parsing the fixed
/// header the same way `from_bytes` does.
fn locate_sections(bytes: &[u8]) -> (usize, usize, usize) {
    let mut vr = CanonReader::new(&bytes[9..]);
    vr.uvarint().unwrap();
    vr.uvarint().unwrap();
    vr.uvarint().unwrap();
    let version_len = (bytes.len() - 9) - vr.remaining();
    let digest_start = 9 + version_len;
    let meta_start = digest_start + 32;
    let mut mr = CanonReader::new(&bytes[meta_start..]);
    let meta_len = mr.uvarint_len().unwrap();
    let meta_prefix_len = (bytes.len() - meta_start) - mr.remaining();
    let blob_start = meta_start + meta_prefix_len + meta_len;
    (digest_start, meta_start, blob_start)
}

#[test]
fn a_flipped_version_byte_is_detected_as_a_digest_mismatch() {
    let ir = build();
    let mut bytes = to_bytes(&ir);
    let (digest_start, ..) = locate_sections(&bytes);
    // Flip the low bit of the patch byte: stays a valid minimal single-byte uvarint, and patch is
    // ignored by the version gate (RFC 011 D-3), so this reaches the digest check.
    bytes[digest_start - 1] ^= 0x01;
    match from_bytes(&bytes) {
        Err(Error::DigestMismatch) => {}
        other => panic!("expected DigestMismatch, got {other:?}"),
    }
}

#[test]
fn a_flipped_metadata_byte_is_detected_as_a_digest_mismatch() {
    let ir = build();
    let mut bytes = to_bytes(&ir);
    let (_, meta_start, blob_start) = locate_sections(&bytes);
    assert!(blob_start > meta_start);
    bytes[blob_start - 1] ^= 0xff;
    match from_bytes(&bytes) {
        Err(Error::DigestMismatch) => {}
        other => panic!("expected DigestMismatch, got {other:?}"),
    }
}

#[test]
fn a_flipped_blob_store_byte_is_detected_as_a_digest_mismatch() {
    let ir = build();
    let mut bytes = to_bytes(&ir);
    let (.., blob_start) = locate_sections(&bytes);
    assert!(bytes.len() > blob_start);
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    match from_bytes(&bytes) {
        Err(Error::DigestMismatch) => {}
        other => panic!("expected DigestMismatch, got {other:?}"),
    }
}

// ---- structural checks (RFC 011 §2.5) -------------------------------------------------------------

#[test]
fn atoms_out_of_canonical_order_are_rejected() {
    let mut ir = build();
    ir.atoms.reverse(); // child before its own parent — breaks both order and "earlier atom" rules
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn a_copy_naming_a_later_atom_as_from_atom_is_rejected() {
    // Two independent roots (no parent relationship, so their relative order is a pure tiebreak and
    // stays canonical either way): X's copy names Y — the atom stored *after* it — as its from_atom.
    // A from_atom must be strictly earlier, never later, independent of the parent/topo-order rule.
    let mut ir = build();
    let mut y = ir.atoms[1].clone();
    y.parents.clear(); // make Y a second independent root, unrelated to X
    y.copies.clear();
    y.id = y.compute_id();
    let mut x = ir.atoms[0].clone();
    x.copies.push(crate::model::CopyRecord {
        from: "readme".into(),
        from_atom: y.id,
        to: "impossible".into(),
        status: EpistemicStatus::Stated,
    });
    x.id = x.compute_id(); // keep the stored id honest after mutating content
    ir.atoms = vec![x, y];
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn a_duplicate_atom_id_is_rejected() {
    let mut ir = build();
    let dup = ir.atoms[0].clone();
    ir.atoms.push(dup);
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn an_orphan_blob_is_rejected() {
    let mut ir = build();
    ir.content.insert(b"nobody references me".to_vec());
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn a_ref_targeting_an_unknown_atom_is_rejected() {
    let mut ir = build();
    ir.refs.push(crate::model::RefRecord {
        name: "refs/heads/ghost".into(),
        kind: crate::model::RefKind::Branch,
        target: crate::model::AtomId([0xEE; 32]),
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    });
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

// ---- unknown fields (RFC 011 §2.2 rule 9) ---------------------------------------------------------

/// Append a trailing field (an id greater than every real field id, so ascending order still holds) to
/// an already-encoded record's bytes.
fn inject_trailing_field(record_bytes: &[u8], id: u64, critical: bool, value: &[u8]) -> Vec<u8> {
    let mut r = CanonReader::new(record_bytes);
    let n = r.uvarint().unwrap();
    let consumed = record_bytes.len() - r.remaining();
    let rest = &record_bytes[consumed..];
    let mut w = crate::canon::CanonWriter::new();
    w.uvarint(n + 1);
    w.raw(rest);
    w.uvarint((id << 1) | u64::from(critical));
    w.uvarint(value.len() as u64);
    w.raw(value);
    w.into_bytes()
}

fn container_with_meta(ir: &Ir, meta: &[u8]) -> Vec<u8> {
    let mut version = crate::canon::CanonWriter::new();
    version.uvarint(0);
    version.uvarint(2);
    version.uvarint(0);

    let mut body = crate::canon::CanonWriter::new();
    body.uvarint(meta.len() as u64);
    body.raw(meta);
    body.uvarint(ir.content.len() as u64);
    for (id, bytes) in ir.content.iter_sorted() {
        body.raw32(id.as_bytes());
        body.uvarint(bytes.len() as u64);
        body.raw(bytes);
    }

    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(body.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(body.as_bytes());
    out
}

#[test]
fn trailing_bytes_in_metadata_are_rejected() {
    let ir = build();
    let mut meta = ir.encode_metadata();
    meta.extend_from_slice(b"junk"); // the outer length prefix will cover this, so it lands *inside*
    // the metadata span, where the Ir record itself doesn't consume it.
    let bytes = container_with_meta(&ir, &meta);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn trailing_bytes_after_the_blob_store_are_rejected() {
    // Rebuild the container with trailing junk appended to the *stored* bytes (so the digest — which
    // covers exactly those bytes — still matches, and the failure is unambiguously the trailing-bytes
    // check below it, not a digest mismatch).
    let ir = build();
    let bytes = to_bytes(&ir);
    let digest_end = 9 + 3 + 32; // magic(8) + format(1) + version(3, for 0.2.0) + digest(32)
    let mut body = bytes[digest_end..].to_vec();
    body.extend_from_slice(b"junk");
    let mut version = crate::canon::CanonWriter::new();
    version.uvarint(0);
    version.uvarint(2);
    version.uvarint(0);
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(&body);
    let digest: [u8; 32] = hasher.finalize().into();
    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(&body);
    match from_bytes(&out) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn an_unknown_non_critical_field_on_ir_is_skipped_and_counted() {
    let ir = build();
    let meta = inject_trailing_field(&ir.encode_metadata(), 99, false, b"from the future");
    let bytes = container_with_meta(&ir, &meta);
    let decoded = from_bytes(&bytes).unwrap();
    assert_eq!(decoded.skipped_non_critical_fields, 1);
    assert_eq!(decoded.ir, ir);
}

#[test]
fn an_unknown_critical_field_on_ir_is_rejected() {
    let ir = build();
    let meta = inject_trailing_field(&ir.encode_metadata(), 99, true, b"from the future");
    let bytes = container_with_meta(&ir, &meta);
    match from_bytes(&bytes) {
        Err(Error::UnknownCriticalField {
            record: "Ir",
            tag: 99,
        }) => {}
        other => panic!("expected UnknownCriticalField, got {other:?}"),
    }
}

// ---- panic-freedom (RFC 011 §10, brygge-03) -------------------------------------------------------

#[test]
fn truncations_and_single_byte_mutations_never_panic() {
    let ir = build();
    let bytes = to_bytes(&ir);

    for n in 0..=bytes.len() {
        let _ = from_bytes(&bytes[..n]); // must return Ok or Err, never panic
    }
    for i in 0..bytes.len() {
        let mut mutated = bytes.clone();
        mutated[i] ^= 0xff;
        let _ = from_bytes(&mutated); // must return Ok or Err, never panic
    }
}

// ---- canonical order of refs, drops and flags is on the variant numbers (batch I) ------------------

const ALL_REF_KINDS: [fn() -> RefKind; 5] = [
    || RefKind::Branch,
    || RefKind::Tag,
    || RefKind::Bookmark,
    || RefKind::NamedBranch,
    || RefKind::Other("phase".to_string()),
];
const ALL_LOSS_CLASSES: [LossClass; 3] = [
    LossClass::Representation,
    LossClass::AdvisoryUnreliable,
    LossClass::Other,
];
const ALL_FLAG_KINDS: [FlagKind; 2] = [
    FlagKind::ConventionViolation,
    FlagKind::BelowConfidenceFloor,
];

fn ref_of(ir: &Ir, name: &str, kind: RefKind) -> RefRecord {
    RefRecord {
        name: name.into(),
        kind,
        target: ir.atoms[0].id,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    }
}

fn drop_of(class: LossClass, what: &str) -> DropRecord {
    DropRecord {
        class,
        what: what.into(),
        reason: "r".into(),
    }
}

fn flag_of(kind: FlagKind, what: &str) -> Flag {
    Flag {
        kind,
        what: what.into(),
        count: 1,
        reason: "r".into(),
    }
}

/// A builder over the shared two-atom fixture, so the atoms and blobs are the same as `build()`'s.
fn builder_with_atoms() -> IrBuilder {
    let ir = build();
    let mut b = IrBuilder::new(ir.provenance.clone());
    for atom in &ir.atoms {
        for op in &atom.ops {
            if let PathOp::Add { blob, .. } = op {
                b.add_blob(ir.content.get(blob).unwrap().to_vec());
            }
        }
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
    b
}

#[test]
fn every_pair_of_ref_kinds_roundtrips_in_both_insertion_orders() {
    let target = build().atoms[0].id;
    for a in ALL_REF_KINDS {
        for b in ALL_REF_KINDS {
            if a() == b() {
                continue;
            }
            let mut builder = builder_with_atoms();
            for kind in [a(), b()] {
                builder
                    .add_ref(RefRecord {
                        name: "x".into(),
                        kind,
                        target,
                        status: EpistemicStatus::Stated,
                        source: None,
                        annotation: None,
                    })
                    .unwrap();
            }
            let ir = builder.finish().unwrap();
            let read = from_bytes(&to_bytes(&ir)).unwrap_or_else(|e| panic!("{a:?}/{b:?}: {e}"));
            assert_eq!(read.ir.refs, ir.refs);
            let variants: Vec<u8> = ir.refs.iter().map(|r| r.kind.variant()).collect();
            assert!(variants.windows(2).all(|w| w[0] < w[1]), "{variants:?}");
        }
    }
}

#[test]
fn every_pair_of_loss_classes_roundtrips_in_both_insertion_orders() {
    for a in ALL_LOSS_CLASSES {
        for b in ALL_LOSS_CLASSES {
            if a == b {
                continue;
            }
            let mut builder = builder_with_atoms();
            builder.set_loss(LossBoundary {
                dropped: vec![drop_of(a, "same"), drop_of(b, "same")],
            });
            let ir = builder.finish().unwrap();
            let read = from_bytes(&to_bytes(&ir)).unwrap_or_else(|e| panic!("{a:?}/{b:?}: {e}"));
            assert_eq!(read.ir.loss, ir.loss);
            assert!(ir.loss.dropped[0].class.variant() < ir.loss.dropped[1].class.variant());
        }
    }
}

#[test]
fn every_pair_of_flag_kinds_roundtrips_in_both_insertion_orders() {
    for a in ALL_FLAG_KINDS {
        for b in ALL_FLAG_KINDS {
            if a == b {
                continue;
            }
            let mut builder = builder_with_atoms();
            builder.add_flag(flag_of(a, "same"));
            builder.add_flag(flag_of(b, "same"));
            let ir = builder.finish().unwrap();
            let read = from_bytes(&to_bytes(&ir)).unwrap_or_else(|e| panic!("{a:?}/{b:?}: {e}"));
            assert_eq!(read.ir.flags, ir.flags);
            assert!(ir.flags[0].kind.variant() < ir.flags[1].kind.variant());
        }
    }
}

/// The three keys are `(name, variant)`, `(class variant, what)` and `(kind variant, what)`: within one
/// variant the name (or `what`) decides, and a different name sorts by bytes, not by the variant.
#[test]
fn names_order_before_variants_for_refs_and_variants_before_whats_for_drops_and_flags() {
    let mut builder = builder_with_atoms();
    let target = build().atoms[0].id;
    for (name, kind) in [
        ("b", RefKind::Branch),
        ("a", RefKind::NamedBranch),
        ("a", RefKind::Tag),
    ] {
        builder
            .add_ref(RefRecord {
                name: name.into(),
                kind,
                target,
                status: EpistemicStatus::Stated,
                source: None,
                annotation: None,
            })
            .unwrap();
    }
    builder.set_loss(LossBoundary {
        dropped: vec![
            drop_of(LossClass::Other, "a"),
            drop_of(LossClass::Representation, "z"),
        ],
    });
    builder.add_flag(flag_of(FlagKind::BelowConfidenceFloor, "a"));
    builder.add_flag(flag_of(FlagKind::ConventionViolation, "z"));
    let ir = builder.finish().unwrap();
    from_bytes(&to_bytes(&ir)).unwrap();
    let refs: Vec<_> = ir
        .refs
        .iter()
        .map(|r| (r.name.as_str(), r.kind.variant()))
        .collect();
    assert_eq!(refs, [("a", 1), ("a", 3), ("b", 0)]);
    assert_eq!(ir.loss.dropped[0].what, "z");
    assert_eq!(ir.flags[0].what, "z");
}

fn assert_non_canonical(ir: &Ir, what: &str) {
    match from_bytes(&to_bytes(ir)) {
        Err(Error::NonCanonical(m)) => assert!(m.contains(what), "{m}"),
        other => panic!("expected NonCanonical ({what}), got {other:?}"),
    }
}

#[test]
fn a_hand_built_stream_with_a_pair_in_the_wrong_variant_order_is_non_canonical() {
    for a in ALL_REF_KINDS {
        for b in ALL_REF_KINDS {
            if a().variant() <= b().variant() {
                continue;
            }
            let mut ir = build();
            ir.refs = vec![ref_of(&ir, "x", a()), ref_of(&ir, "x", b())];
            assert_non_canonical(&ir, "Ir.refs");
        }
    }
    for a in ALL_LOSS_CLASSES {
        for b in ALL_LOSS_CLASSES {
            if a.variant() <= b.variant() {
                continue;
            }
            let mut ir = build();
            ir.loss.dropped = vec![drop_of(a, "same"), drop_of(b, "same")];
            assert_non_canonical(&ir, "Ir.dropped");
        }
    }
    for a in ALL_FLAG_KINDS {
        for b in ALL_FLAG_KINDS {
            if a.variant() <= b.variant() {
                continue;
            }
            let mut ir = build();
            ir.flags = vec![flag_of(a, "same"), flag_of(b, "same")];
            assert_non_canonical(&ir, "Ir.flags");
        }
    }
}

#[test]
fn a_hand_built_stream_with_a_duplicate_key_is_non_canonical() {
    let mut ir = build();
    ir.refs = vec![
        ref_of(&ir, "x", RefKind::Other("one".into())),
        ref_of(&ir, "x", RefKind::Other("two".into())),
    ];
    assert_non_canonical(&ir, "Ir.refs");
    let mut ir = build();
    ir.loss.dropped = vec![
        drop_of(LossClass::Other, "w"),
        drop_of(LossClass::Other, "w"),
    ];
    assert_non_canonical(&ir, "Ir.dropped");
    let mut ir = build();
    ir.flags = vec![
        flag_of(FlagKind::ConventionViolation, "w"),
        flag_of(FlagKind::ConventionViolation, "w"),
    ];
    assert_non_canonical(&ir, "Ir.flags");
}

#[test]
fn the_builder_refuses_a_duplicate_key_instead_of_writing_an_unreadable_artifact() {
    let target = build().atoms[0].id;
    let mut b = builder_with_atoms();
    for label in ["one", "two"] {
        b.add_ref(RefRecord {
            name: "x".into(),
            kind: RefKind::Other(label.into()),
            target,
            status: EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    }
    assert!(matches!(b.finish(), Err(Error::Invariant(_))));

    let mut b = builder_with_atoms();
    b.set_loss(LossBoundary {
        dropped: vec![
            drop_of(LossClass::Other, "w"),
            drop_of(LossClass::Other, "w"),
        ],
    });
    assert!(matches!(b.finish(), Err(Error::Invariant(_))));

    let mut b = builder_with_atoms();
    b.add_flag(flag_of(FlagKind::BelowConfidenceFloor, "w"));
    b.add_flag(flag_of(FlagKind::BelowConfidenceFloor, "w"));
    assert!(matches!(b.finish(), Err(Error::Invariant(_))));
}

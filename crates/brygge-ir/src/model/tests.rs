//! Tests for the IR model types, their canonical (RFC 011 §2.4) round trips, and `AtomId` computation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::Error;
use crate::canon::{CanonReader, RecordWriter};

fn sample_source() -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"repo".to_vec(),
        atom_id: b"deadbeef".to_vec(),
        signatures: vec![Signature {
            label: "gpgsig".to_string(),
            bytes: b"sig-bytes".to_vec(),
        }],
        extras: vec![Extra {
            label: "mergetag".to_string(),
            bytes: b"extra-bytes".to_vec(),
        }],
    }
}

fn atom(msg: &str, source_atom: &[u8]) -> ChangeAtom {
    let mut a = ChangeAtom {
        id: AtomId([0u8; 32]),
        parents: Vec::new(),
        ops: Vec::new(),
        copies: Vec::new(),
        metadata: MetadataClaims {
            message: Some(Text::utf8(msg)),
            ..MetadataClaims::default()
        },
        source: SourceIdentity {
            atom_id: source_atom.to_vec(),
            ..sample_source()
        },
        status: EpistemicStatus::Stated,
    };
    a.id = a.compute_id();
    a
}

// ---- round trips ------------------------------------------------------------------------------

#[test]
fn text_roundtrips_with_and_without_encoding() {
    for t in [
        Text::utf8("hello"),
        Text {
            bytes: Vec::new(),
            encoding: None,
        },
        Text {
            bytes: b"\xff\xfe".to_vec(),
            encoding: Some("latin1".to_string()),
        },
    ] {
        let bytes = t.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(Text::decode(&mut r).unwrap(), t);
        assert!(r.is_empty());
    }
}

#[test]
fn time_roundtrips_with_and_without_offset() {
    for t in [
        Time {
            seconds: 0,
            offset_minutes: None,
        },
        Time {
            seconds: -12345,
            offset_minutes: Some(-480),
        },
        Time {
            seconds: i64::from(i32::MAX),
            offset_minutes: Some(330),
        },
    ] {
        let bytes = t.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(Time::decode(&mut r).unwrap(), t);
        assert!(r.is_empty());
    }
}

#[test]
fn time_offset_out_of_i16_range_is_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(1, true, crate::canon::svarint_value(0));
    rw.field(
        2,
        true,
        crate::canon::svarint_value(i64::from(i16::MAX) + 1),
    );
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match Time::decode(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn source_kind_roundtrips_every_variant() {
    for k in [
        SourceKind::Git,
        SourceKind::Hg,
        SourceKind::Svn,
        SourceKind::Cvs,
        SourceKind::Other("perforce".to_string()),
    ] {
        let bytes = k.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(SourceKind::decode(&mut r).unwrap(), k);
        assert!(r.is_empty());
    }
}

#[test]
fn source_identity_roundtrips() {
    let s = sample_source();
    let bytes = s.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(SourceIdentity::decode(&mut r).unwrap(), s);
    assert!(r.is_empty());
}

#[test]
fn source_identity_roundtrips_with_no_signatures_or_extras() {
    let s = SourceIdentity {
        signatures: Vec::new(),
        extras: Vec::new(),
        ..sample_source()
    };
    let bytes = s.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(SourceIdentity::decode(&mut r).unwrap(), s);
}

#[test]
fn an_empty_signatures_list_encoded_is_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(1, true, SourceKind::Git.encode());
    rw.field(2, true, b"r".to_vec());
    rw.field(3, true, b"a".to_vec());
    rw.field(4, true, crate::canon::list_value(std::iter::empty()));
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match SourceIdentity::decode(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn path_op_roundtrips_every_variant() {
    let status = EpistemicStatus::Stated;
    let blob = BlobId([7u8; 32]);
    let ops = [
        PathOp::Add {
            path: "a".into(),
            blob,
            mode: 0o100644,
            status: status.clone(),
        },
        PathOp::Modify {
            path: "a".into(),
            blob,
            mode: 0o100644,
            status: status.clone(),
        },
        PathOp::Delete {
            path: "a".into(),
            status: status.clone(),
        },
        PathOp::Replace {
            path: "a".into(),
            blob,
            mode: 0o120000,
            status,
        },
    ];
    for op in ops {
        let bytes = op.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(PathOp::decode(&mut r).unwrap(), op);
        assert!(r.is_empty());
    }
}

#[test]
fn copy_record_roundtrips() {
    let c = CopyRecord {
        from: "old.txt".into(),
        from_atom: AtomId([3u8; 32]),
        to: "new.txt".into(),
        status: EpistemicStatus::Stated,
    };
    let bytes = c.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(CopyRecord::decode(&mut r).unwrap(), c);
}

#[test]
fn identity_roundtrips_with_and_without_email() {
    for i in [
        Identity {
            name: Text::utf8("Ada"),
            email: Some(Text::utf8("ada@example.com")),
        },
        Identity {
            name: Text::utf8("Ada"),
            email: None,
        },
    ] {
        let bytes = i.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(Identity::decode(&mut r).unwrap(), i);
    }
}

#[test]
fn metadata_claims_roundtrips_full_and_empty() {
    let full = MetadataClaims {
        author: Some(Identity {
            name: Text::utf8("A"),
            email: None,
        }),
        author_time: Some(Time {
            seconds: 1,
            offset_minutes: None,
        }),
        committer: Some(Identity {
            name: Text::utf8("C"),
            email: None,
        }),
        commit_time: Some(Time {
            seconds: 2,
            offset_minutes: Some(60),
        }),
        message: Some(Text::utf8("msg")),
    };
    assert!(!full.is_empty());
    let bytes = full.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(MetadataClaims::decode(&mut r).unwrap(), full);

    let empty = MetadataClaims::default();
    assert!(empty.is_empty());
    let bytes = empty.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(MetadataClaims::decode(&mut r).unwrap(), empty);
}

#[test]
fn ref_kind_roundtrips_every_variant() {
    for k in [
        RefKind::Branch,
        RefKind::Tag,
        RefKind::Bookmark,
        RefKind::NamedBranch,
        RefKind::Other("phase".to_string()),
    ] {
        let bytes = k.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(RefKind::decode(&mut r).unwrap(), k);
    }
}

#[test]
fn ref_record_roundtrips_with_source_and_annotation() {
    let rf = RefRecord {
        name: "refs/tags/v1".into(),
        kind: RefKind::Tag,
        target: AtomId([9u8; 32]),
        status: EpistemicStatus::Stated,
        source: Some(sample_source()),
        annotation: Some(Annotation {
            tagger: Some(Identity {
                name: Text::utf8("T"),
                email: None,
            }),
            time: Some(Time {
                seconds: 5,
                offset_minutes: None,
            }),
            message: Some(Text::utf8("tag message")),
        }),
    };
    let bytes = rf.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(RefRecord::decode(&mut r).unwrap(), rf);
}

#[test]
fn ref_record_roundtrips_with_neither_source_nor_annotation() {
    let rf = RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: AtomId([1u8; 32]),
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    };
    let bytes = rf.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(RefRecord::decode(&mut r).unwrap(), rf);
}

#[test]
fn loss_class_roundtrips_every_variant() {
    for c in [
        LossClass::Representation,
        LossClass::AdvisoryUnreliable,
        LossClass::Other,
    ] {
        let bytes = c.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(LossClass::decode(&mut r).unwrap(), c);
    }
}

#[test]
fn drop_record_roundtrips() {
    let d = DropRecord {
        class: LossClass::AdvisoryUnreliable,
        what: "svn:mergeinfo".into(),
        reason: "advisory, not authoritative".into(),
    };
    let bytes = d.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(DropRecord::decode(&mut r).unwrap(), d);
}

#[test]
fn flag_kind_roundtrips_every_variant() {
    for k in [
        FlagKind::ConventionViolation,
        FlagKind::BelowConfidenceFloor,
    ] {
        let bytes = k.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(FlagKind::decode(&mut r).unwrap(), k);
    }
}

#[test]
fn flag_roundtrips() {
    let f = Flag {
        kind: FlagKind::BelowConfidenceFloor,
        what: "1.0..1.4".into(),
        count: 3,
        reason: "below the CVS clustering confidence floor".into(),
    };
    let bytes = f.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(Flag::decode(&mut r).unwrap(), f);
}

#[test]
fn flag_count_zero_is_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(1, true, FlagKind::ConventionViolation.encode());
    rw.field(2, true, b"x".to_vec());
    rw.field(3, true, crate::canon::uvarint_value(0));
    rw.field(4, true, b"why".to_vec());
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match Flag::decode(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn import_provenance_roundtrips_with_and_without_params() {
    let mut params = std::collections::BTreeMap::new();
    params.insert("floor".to_string(), "2".to_string());
    let with = ImportProvenance {
        source: sample_source(),
        brygge_version: "0.1.0".into(),
        decoder: "brygge-decode-git".into(),
        decoder_version: "0.1.0".into(),
        params,
    };
    let bytes = with.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(ImportProvenance::decode(&mut r).unwrap(), with);

    let without = ImportProvenance {
        params: std::collections::BTreeMap::new(),
        ..with
    };
    let bytes = without.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(ImportProvenance::decode(&mut r).unwrap(), without);
}

// ---- ChangeAtom / AtomId ------------------------------------------------------------------------

#[test]
fn atom_id_is_deterministic_and_content_sensitive() {
    let a1 = atom("same", b"x");
    let a2 = atom("same", b"x");
    let a3 = atom("different", b"x");
    assert_eq!(a1.id, a2.id); // identical content → identical id
    assert_ne!(a1.id, a3.id); // different content → different id
    assert_eq!(a1.id.to_hex().len(), 64);
}

#[test]
fn atom_id_excludes_only_field_1() {
    // Two atoms differing only in their (irrelevant, since it's recomputed) stored id field must
    // compute the same id — the id is a function of fields 2-7 alone.
    let mut a = atom("m", b"y");
    a.id = AtomId([0u8; 32]);
    let id_a = a.compute_id();
    a.id = AtomId([0xff; 32]);
    let id_b = a.compute_id();
    assert_eq!(id_a, id_b);
}

#[test]
fn atom_decode_rejects_a_wrong_stored_id() {
    let mut a = atom("m", b"y");
    a.id = AtomId([0xAB; 32]); // corrupt the stored id
    let bytes = a.encode();
    let mut r = CanonReader::new(&bytes);
    assert!(ChangeAtom::decode(&mut r).is_err());
}

#[test]
fn atom_roundtrips_with_ops_and_copies() {
    let mut a = ChangeAtom {
        id: AtomId([0u8; 32]),
        parents: vec![AtomId([1u8; 32])],
        ops: vec![
            PathOp::Add {
                path: "a.txt".into(),
                blob: BlobId([2u8; 32]),
                mode: 0o100644,
                status: EpistemicStatus::Stated,
            },
            PathOp::Delete {
                path: "old.txt".into(),
                status: EpistemicStatus::Stated,
            },
        ],
        copies: vec![CopyRecord {
            from: "old.txt".into(),
            from_atom: AtomId([1u8; 32]),
            to: "a.txt".into(),
            status: EpistemicStatus::Stated,
        }],
        metadata: MetadataClaims::default(),
        source: sample_source(),
        status: EpistemicStatus::Stated,
    };
    a.id = a.compute_id();
    assert!(a.is_move(&a.copies[0]));

    let bytes = a.encode();
    let mut r = CanonReader::new(&bytes);
    assert_eq!(ChangeAtom::decode(&mut r).unwrap(), a);
}

#[test]
fn ops_out_of_order_are_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(1, true, [0u8; 32].to_vec());
    let ops = [
        PathOp::Delete {
            path: "b".into(),
            status: EpistemicStatus::Stated,
        },
        PathOp::Delete {
            path: "a".into(),
            status: EpistemicStatus::Stated,
        },
    ];
    rw.field(
        3,
        true,
        crate::canon::list_value(ops.iter().map(PathOp::encode)),
    );
    rw.field(6, true, sample_source().encode());
    rw.field(7, true, EpistemicStatus::Stated.encode());
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match ChangeAtom::decode(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn copies_out_of_order_are_rejected() {
    let from_atom = AtomId([1u8; 32]);
    let copies = [
        CopyRecord {
            from: "x".into(),
            from_atom,
            to: "b".into(),
            status: EpistemicStatus::Stated,
        },
        CopyRecord {
            from: "x".into(),
            from_atom,
            to: "a".into(),
            status: EpistemicStatus::Stated,
        },
    ];
    let mut rw = RecordWriter::new();
    rw.field(1, true, [0u8; 32].to_vec());
    rw.field(
        4,
        true,
        crate::canon::list_value(copies.iter().map(CopyRecord::encode)),
    );
    rw.field(6, true, sample_source().encode());
    rw.field(7, true, EpistemicStatus::Stated.encode());
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match ChangeAtom::decode(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn metadata_encoded_but_every_claim_absent_is_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(1, true, [0u8; 32].to_vec());
    rw.field(5, true, MetadataClaims::default().encode());
    rw.field(6, true, sample_source().encode());
    rw.field(7, true, EpistemicStatus::Stated.encode());
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match ChangeAtom::decode(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

/// AtomId compatibility vector (RFC 011 §7.6): pins one literal hash for a fixed atom, so a change to
/// the codec or the model's field order is caught as a regression rather than silently shipped.
#[test]
fn atom_id_compatibility_vector() {
    let a = atom("same", b"x");
    assert_eq!(
        a.id.to_hex(),
        "95b657712f771e76c275d509d6108a05db812c5bf07dc5e3771f88c232626610",
    );
}

// ---- Ir -----------------------------------------------------------------------------------------

#[test]
fn ir_refs_out_of_order_are_rejected() {
    let a = atom("m", b"x");
    let target = a.id;
    let mut rw = RecordWriter::new();
    rw.field(
        1,
        true,
        crate::canon::list_value(std::iter::once(a.encode())),
    );
    let refs = [
        RefRecord {
            name: "b".into(),
            kind: RefKind::Branch,
            target,
            status: EpistemicStatus::Stated,
            source: None,
            annotation: None,
        },
        RefRecord {
            name: "a".into(),
            kind: RefKind::Branch,
            target,
            status: EpistemicStatus::Stated,
            source: None,
            annotation: None,
        },
    ];
    rw.field(
        2,
        true,
        crate::canon::list_value(refs.iter().map(RefRecord::encode)),
    );
    let prov = ImportProvenance {
        source: sample_source(),
        brygge_version: "0".into(),
        decoder: "d".into(),
        decoder_version: "0".into(),
        params: std::collections::BTreeMap::new(),
    };
    rw.field(3, true, prov.encode());
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match Ir::decode_metadata(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

fn sample_provenance() -> ImportProvenance {
    ImportProvenance {
        source: sample_source(),
        brygge_version: "0".into(),
        decoder: "d".into(),
        decoder_version: "0".into(),
        params: std::collections::BTreeMap::new(),
    }
}

#[test]
fn ir_dropped_out_of_order_is_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(3, true, sample_provenance().encode());
    let dropped = [
        DropRecord {
            class: LossClass::Other,
            what: "z".into(),
            reason: "r".into(),
        },
        DropRecord {
            class: LossClass::Other,
            what: "a".into(),
            reason: "r".into(),
        },
    ];
    rw.field(
        4,
        true,
        crate::canon::list_value(dropped.iter().map(DropRecord::encode)),
    );
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match Ir::decode_metadata(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn ir_flags_out_of_order_are_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(3, true, sample_provenance().encode());
    let flags = [
        Flag {
            kind: FlagKind::ConventionViolation,
            what: "z".into(),
            count: 1,
            reason: "r".into(),
        },
        Flag {
            kind: FlagKind::ConventionViolation,
            what: "a".into(),
            count: 1,
            reason: "r".into(),
        },
    ];
    rw.field(
        5,
        true,
        crate::canon::list_value(flags.iter().map(Flag::encode)),
    );
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match Ir::decode_metadata(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

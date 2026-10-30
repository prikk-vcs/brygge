//! Tests for the epistemic-status type and derivation taxonomy (RFC 011 §2.4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::Error;
use crate::canon::{CanonReader, CanonWriter, RecordWriter};

fn roundtrip(s: &EpistemicStatus) -> EpistemicStatus {
    let bytes = s.encode();
    let mut r = CanonReader::new(&bytes);
    let out = EpistemicStatus::decode(&mut r, 0).unwrap();
    assert!(r.is_empty());
    out
}

#[test]
fn stated_roundtrips_and_is_not_derived() {
    let s = EpistemicStatus::Stated;
    assert!(!s.is_derived());
    assert_eq!(roundtrip(&s), s);
}

#[test]
fn derived_roundtrips_with_params_and_confidence() {
    let mut params = BTreeMap::new();
    params.insert("algorithm".to_string(), "similarity".to_string());
    params.insert("threshold".to_string(), "50".to_string());
    let s = EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::InferredRename,
        by: "brygge-decode-git".to_string(),
        decoder_version: "0.1.0".to_string(),
        params,
        confidence: Some(80),
    });
    assert!(s.is_derived());
    assert_eq!(roundtrip(&s), s);
}

#[test]
fn derived_roundtrips_with_no_params_and_no_confidence() {
    let s = EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::NormalizedMetadata,
        by: "x".into(),
        decoder_version: "0".into(),
        params: BTreeMap::new(),
        confidence: None,
    });
    assert_eq!(roundtrip(&s), s);
}

#[test]
fn other_kind_preserves_its_label() {
    let s = EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::Other("bespoke".to_string()),
        by: "x".into(),
        decoder_version: "0".into(),
        params: BTreeMap::new(),
        confidence: None,
    });
    assert_eq!(roundtrip(&s), s);
}

#[test]
fn every_derivation_kind_round_trips() {
    let kinds = [
        DerivationKind::InferredRename,
        DerivationKind::ReconstructedChangeset,
        DerivationKind::ReconstructedBranch,
        DerivationKind::InferredMerge,
        DerivationKind::NormalizedMetadata,
        DerivationKind::Other("x".to_string()),
    ];
    for kind in kinds {
        let bytes = kind.encode();
        let mut r = CanonReader::new(&bytes);
        assert_eq!(DerivationKind::decode(&mut r).unwrap(), kind);
        assert!(r.is_empty());
    }
}

#[test]
fn confidence_over_100_is_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(1, true, DerivationKind::NormalizedMetadata.encode());
    rw.field(2, true, b"x".to_vec());
    rw.field(3, true, b"0".to_vec());
    rw.field(5, true, crate::canon::uvarint_value(101));
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match Derivation::decode(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn an_empty_params_map_encoded_is_rejected() {
    // The canonical rule: a list/map field that is empty is omitted, never encoded as present-but-empty.
    let mut rw = RecordWriter::new();
    rw.field(1, true, DerivationKind::NormalizedMetadata.encode());
    rw.field(2, true, b"x".to_vec());
    rw.field(3, true, b"0".to_vec());
    rw.field(4, true, crate::canon::map_value(std::iter::empty()));
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match Derivation::decode(&mut r) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn transformed_round_trips_over_stated() {
    let s = EpistemicStatus::Transformed {
        step: 0,
        changed: vec![ChangedPart::Author, ChangedPart::Message],
        prior: Box::new(EpistemicStatus::Stated),
    };
    assert_eq!(roundtrip(&s), s);
}

#[test]
fn transformed_round_trips_nested_over_derived() {
    let s = EpistemicStatus::Transformed {
        step: 2,
        changed: vec![ChangedPart::Ops],
        prior: Box::new(EpistemicStatus::Transformed {
            step: 0,
            changed: vec![ChangedPart::Author],
            prior: Box::new(EpistemicStatus::Derived(Derivation {
                kind: DerivationKind::NormalizedMetadata,
                by: "x".into(),
                decoder_version: "0".into(),
                params: BTreeMap::new(),
                confidence: None,
            })),
        }),
    };
    assert_eq!(roundtrip(&s), s);
}

#[test]
fn every_changed_part_round_trips_through_its_own_spelling() {
    let parts = [
        ChangedPart::Author,
        ChangedPart::AuthorTime,
        ChangedPart::Committer,
        ChangedPart::CommitTime,
        ChangedPart::Message,
        ChangedPart::Ops,
        ChangedPart::Copies,
        ChangedPart::Parents,
        ChangedPart::Blob,
        ChangedPart::Mode,
        ChangedPart::Path,
        ChangedPart::Annotation,
        ChangedPart::Kind,
        ChangedPart::Name,
        ChangedPart::Target,
    ];
    for part in parts {
        assert_eq!(ChangedPart::parse(part.as_str()), Some(part));
    }
    assert_eq!(ChangedPart::parse("bogus"), None);
}

#[test]
fn transformed_with_empty_changed_is_rejected() {
    let mut w = CanonWriter::new();
    w.uvarint(2);
    let mut rw = RecordWriter::new();
    rw.field(1, true, crate::canon::uvarint_value(0));
    rw.field(3, true, EpistemicStatus::Stated.encode());
    rw.finish_into(&mut w);
    let bytes = w.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match EpistemicStatus::decode(&mut r, 0) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn transformed_with_unordered_changed_is_rejected() {
    let mut w = CanonWriter::new();
    w.uvarint(2);
    let mut rw = RecordWriter::new();
    rw.field(1, true, crate::canon::uvarint_value(0));
    rw.field(
        2,
        true,
        crate::canon::list_value(
            [ChangedPart::Ops, ChangedPart::Author]
                .iter()
                .map(|c| text_item(c.as_str())),
        ),
    );
    rw.field(3, true, EpistemicStatus::Stated.encode());
    rw.finish_into(&mut w);
    let bytes = w.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match EpistemicStatus::decode(&mut r, 0) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn transformed_with_an_unknown_changed_part_is_rejected() {
    let mut w = CanonWriter::new();
    w.uvarint(2);
    let mut rw = RecordWriter::new();
    rw.field(1, true, crate::canon::uvarint_value(0));
    rw.field(
        2,
        true,
        crate::canon::list_value(std::iter::once(text_item("frobnicate"))),
    );
    rw.field(3, true, EpistemicStatus::Stated.encode());
    rw.finish_into(&mut w);
    let bytes = w.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match EpistemicStatus::decode(&mut r, 0) {
        Err(Error::Decode(_)) => {}
        other => panic!("expected Decode, got {other:?}"),
    }
}

#[test]
fn transformed_whose_prior_step_is_not_less_is_rejected() {
    let mut w = CanonWriter::new();
    w.uvarint(2);
    let mut rw = RecordWriter::new();
    rw.field(1, true, crate::canon::uvarint_value(1));
    rw.field(
        2,
        true,
        crate::canon::list_value(std::iter::once(text_item(ChangedPart::Author.as_str()))),
    );
    let prior = EpistemicStatus::Transformed {
        step: 1,
        changed: vec![ChangedPart::Message],
        prior: Box::new(EpistemicStatus::Stated),
    };
    rw.field(3, true, prior.encode());
    rw.finish_into(&mut w);
    let bytes = w.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match EpistemicStatus::decode(&mut r, 0) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn transformed_nested_past_64_priors_is_rejected() {
    let mut status = EpistemicStatus::Stated;
    for step in 0..65u64 {
        status = EpistemicStatus::Transformed {
            step,
            changed: vec![ChangedPart::Author],
            prior: Box::new(status),
        };
    }
    let bytes = status.encode();
    let mut r = CanonReader::new(&bytes);
    match EpistemicStatus::decode(&mut r, 0) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn an_unknown_critical_field_on_derivation_is_rejected() {
    let mut rw = RecordWriter::new();
    rw.field(1, true, DerivationKind::NormalizedMetadata.encode());
    rw.field(2, true, b"x".to_vec());
    rw.field(3, true, b"0".to_vec());
    rw.field(99, true, b"?".to_vec());
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match Derivation::decode(&mut r) {
        Err(Error::UnknownCriticalField { record, tag }) => {
            assert_eq!(record, "Derivation");
            assert_eq!(tag, 99);
        }
        other => panic!("expected UnknownCriticalField, got {other:?}"),
    }
}

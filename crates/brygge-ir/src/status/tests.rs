//! Tests for the epistemic-status type and derivation taxonomy (RFC 011 §2.4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::Error;
use crate::canon::{CanonReader, RecordWriter};

fn roundtrip(s: &EpistemicStatus) -> EpistemicStatus {
    let bytes = s.encode();
    let mut r = CanonReader::new(&bytes);
    let out = EpistemicStatus::decode(&mut r).unwrap();
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

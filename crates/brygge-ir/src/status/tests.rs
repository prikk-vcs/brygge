//! Tests for the epistemic-status type and derivation taxonomy.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::canon::{CanonReader, CanonWriter};

fn roundtrip(s: &EpistemicStatus) -> EpistemicStatus {
    let mut w = CanonWriter::new();
    s.encode(&mut w);
    let mut r = CanonReader::new(w.as_bytes());
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

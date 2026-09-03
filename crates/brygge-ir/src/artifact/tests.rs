//! Tests for the artifact codec: round-trip, determinism, integrity, versioning.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{ImportProvenance, MetadataClaims, PathOp, SourceIdentity, SourceKind};
use crate::status::EpistemicStatus;
use crate::version::{self, ContractVersion};

fn provenance(import_time: Option<i64>) -> ImportProvenance {
    ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Git,
            repo_id: b"repo".to_vec(),
            atom_id: b"root".to_vec(),
            signatures: Vec::new(),
        },
        brygge_version: "0.1.0".into(),
        decoder: "brygge-decode-git".into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
        import_time,
    }
}

fn build(import_time: Option<i64>) -> Ir {
    let mut b = IrBuilder::new(provenance(import_time));
    let blob = b.add_blob(b"hello world".to_vec());
    let _root = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "readme".into(),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        rename_hints: vec![],
        metadata: MetadataClaims {
            message: Some("initial".into()),
            ..MetadataClaims::default()
        },
        source: SourceIdentity {
            kind: SourceKind::Git,
            repo_id: b"repo".to_vec(),
            atom_id: b"c1".to_vec(),
            signatures: Vec::new(),
        },
        status: EpistemicStatus::Stated,
    });
    b.finish().unwrap()
}

#[test]
fn roundtrips() {
    let ir = build(Some(42));
    let bytes = to_bytes(&ir);
    let back = from_bytes(&bytes).unwrap();
    assert_eq!(back, ir);
}

#[test]
fn serialization_is_byte_deterministic() {
    let ir = build(Some(42));
    assert_eq!(to_bytes(&ir), to_bytes(&ir));
}

#[test]
fn digest_excludes_import_time() {
    // Two imports differing ONLY in when they ran must share an integrity digest (ID-4 / VF-1).
    let a = build(Some(1));
    let b = build(Some(999_999));
    assert_eq!(digest(&a), digest(&b));
    // …but the stored artifacts differ (import time is preserved as provenance).
    assert_ne!(to_bytes(&a), to_bytes(&b));
}

#[test]
fn a_flipped_byte_is_detected() {
    let ir = build(Some(42));
    let mut bytes = to_bytes(&ir);
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff; // corrupt a content byte
    assert!(from_bytes(&bytes).is_err());
}

#[test]
fn truncation_is_rejected_not_panicked() {
    let ir = build(Some(42));
    let bytes = to_bytes(&ir);
    let cut = &bytes[..bytes.len() - 1];
    assert!(from_bytes(cut).is_err());
    assert!(from_bytes(b"BRYGGEIR").is_err());
    assert!(from_bytes(b"").is_err());
}

#[test]
fn an_unknown_contract_major_is_refused() {
    let mut ir = build(Some(42));
    ir.contract_version = ContractVersion::new(version::CURRENT.major + 1, 0, 0);
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(crate::Error::UnsupportedContractMajor { .. }) => {}
        other => panic!("expected UnsupportedContractMajor, got {other:?}"),
    }
}

#[test]
fn bad_magic_is_rejected() {
    let mut bytes = to_bytes(&build(None));
    bytes[0] = b'X';
    assert!(matches!(from_bytes(&bytes), Err(crate::Error::Decode(_))));
}

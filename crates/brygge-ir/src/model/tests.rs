//! Tests for the IR model types and `AtomId` computation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::canon::{CanonReader, CanonWriter};

fn sample_source() -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"repo".to_vec(),
        atom_id: b"deadbeef".to_vec(),
        signatures: vec![b"gpgsig".to_vec()],
    }
}

fn atom(msg: &str, source_atom: &[u8]) -> ChangeAtom {
    let mut a = ChangeAtom {
        id: AtomId([0u8; 32]),
        parents: Vec::new(),
        ops: Vec::new(),
        rename_hints: Vec::new(),
        metadata: MetadataClaims {
            message: Some(msg.to_string()),
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

#[test]
fn source_identity_roundtrips() {
    let s = sample_source();
    let mut w = CanonWriter::new();
    s.encode(&mut w);
    let mut r = CanonReader::new(w.as_bytes());
    assert_eq!(SourceIdentity::decode(&mut r).unwrap(), s);
    assert!(r.is_empty());
}

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
fn atom_decode_rejects_a_wrong_stored_id() {
    let mut a = atom("m", b"y");
    a.id = AtomId([0xAB; 32]); // corrupt the stored id
    let mut w = CanonWriter::new();
    a.encode(&mut w);
    let mut r = CanonReader::new(w.as_bytes());
    // Decode recomputes the id from content and must reject the mismatch.
    assert!(ChangeAtom::decode(&mut r).is_err());
}

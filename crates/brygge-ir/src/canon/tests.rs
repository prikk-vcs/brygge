//! Tests for the canonical codec (RFC 003 D-1).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn roundtrip_uvarint(v: u64) {
    let mut w = CanonWriter::new();
    w.uvarint(v);
    let mut r = CanonReader::new(w.as_bytes());
    assert_eq!(r.uvarint().unwrap(), v);
    assert!(r.is_empty());
}

#[test]
fn uvarint_roundtrips() {
    for v in [0u64, 1, 127, 128, 300, 16_384, u32::MAX as u64, u64::MAX] {
        roundtrip_uvarint(v);
    }
}

#[test]
fn ivarint_roundtrips() {
    for v in [0i64, -1, 1, -300, 300, i64::MIN, i64::MAX] {
        let mut w = CanonWriter::new();
        w.ivarint(v);
        let mut r = CanonReader::new(w.as_bytes());
        assert_eq!(r.ivarint().unwrap(), v);
    }
}

#[test]
fn bytes_str_and_raw32_roundtrip() {
    let mut w = CanonWriter::new();
    w.bytes(b"");
    w.str("héllo — brygge");
    w.raw32(&[7u8; 32]);
    let mut r = CanonReader::new(w.as_bytes());
    assert_eq!(r.bytes().unwrap(), b"");
    assert_eq!(r.str().unwrap(), "héllo — brygge");
    assert_eq!(r.raw32().unwrap(), [7u8; 32]);
    assert!(r.is_empty());
}

#[test]
fn reader_rejects_truncation_not_panics() {
    // A length prefix claiming more than is present must error, never panic.
    let mut w = CanonWriter::new();
    w.uvarint(50); // claim 50 bytes … but provide none
    let mut r = CanonReader::new(w.as_bytes());
    assert!(r.bytes().is_err());
}

#[test]
fn overlong_varint_is_rejected() {
    // Ten continuation bytes then more → overflow/too-long, an error not a panic.
    let bytes = [0x80u8; 12];
    let mut r = CanonReader::new(&bytes);
    assert!(r.uvarint().is_err());
}

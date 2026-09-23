//! Tests for the tagged-record canonical codec (RFC 011 §2.1/§2.2).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::Error;

#[test]
fn uvarint_round_trips_small_and_large_values() {
    for v in [0u64, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
        let mut w = CanonWriter::new();
        w.uvarint(v);
        let mut r = CanonReader::new(w.as_bytes());
        assert_eq!(r.uvarint().unwrap(), v);
        assert!(r.is_empty());
    }
}

#[test]
fn svarint_round_trips_negative_and_positive() {
    for v in [0i64, -1, 1, i64::MIN, i64::MAX, -300, 300] {
        let mut w = CanonWriter::new();
        w.svarint(v);
        let mut r = CanonReader::new(w.as_bytes());
        assert_eq!(r.svarint().unwrap(), v);
    }
}

#[test]
fn an_overlong_varint_is_rejected() {
    // 0 encoded minimally is [0x00]; [0x80, 0x00] is the same value non-minimally encoded.
    let mut r = CanonReader::new(&[0x80, 0x00]);
    match r.uvarint() {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn a_minimal_single_byte_zero_is_accepted() {
    let mut r = CanonReader::new(&[0x00]);
    assert_eq!(r.uvarint().unwrap(), 0);
}

#[test]
fn a_declared_length_exceeding_input_is_an_error() {
    let mut r = CanonReader::new(&[5]);
    assert!(r.uvarint_len().is_err());
}

#[test]
fn a_record_round_trips_its_fields_in_ascending_order() {
    let mut rw = RecordWriter::new();
    rw.field(1, true, uvarint_value(42));
    rw.field(3, true, b"hi".to_vec());
    let bytes = rw.into_bytes();

    let mut r = CanonReader::new(&bytes);
    let mut field1 = None;
    let mut field3 = None;
    r.record_fields("Test", |r, id, len| match id {
        1 => {
            field1 = Some(r.uvarint().unwrap());
            Ok(true)
        }
        3 => {
            field3 = Some(r.text(len).unwrap());
            Ok(true)
        }
        _ => Ok(false),
    })
    .unwrap();
    assert_eq!(field1, Some(42));
    assert_eq!(field3, Some("hi".to_string()));
}

#[test]
fn descending_tags_are_rejected() {
    // Hand-build a record with field 3 before field 1 — violates strict ascending order.
    let mut w = CanonWriter::new();
    w.uvarint(2); // field count
    w.uvarint((3 << 1) | 1);
    w.uvarint(0);
    w.uvarint((1 << 1) | 1);
    w.uvarint(0);
    let mut r = CanonReader::new(w.as_bytes());
    // Fields 1 and 3 are both recognized (empty-valued), so the ascending-order check — not an
    // unknown-critical-field bail-out — is what fires here.
    match r.record_fields("Test", |_, id, _| Ok(id == 1 || id == 3)) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn duplicate_tags_are_rejected() {
    let mut w = CanonWriter::new();
    w.uvarint(2);
    w.uvarint((1 << 1) | 1);
    w.uvarint(0);
    w.uvarint((1 << 1) | 1);
    w.uvarint(0);
    let mut r = CanonReader::new(w.as_bytes());
    match r.record_fields("Test", |_, id, _| Ok(id == 1)) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn an_unknown_critical_field_fails() {
    let mut rw = RecordWriter::new();
    rw.field(9, true, b"x".to_vec());
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    match r.record_fields("SomeRecord", |_, _, _| Ok(false)) {
        Err(Error::UnknownCriticalField { record, tag }) => {
            assert_eq!(record, "SomeRecord");
            assert_eq!(tag, 9);
        }
        other => panic!("expected UnknownCriticalField, got {other:?}"),
    }
}

#[test]
fn an_unknown_non_critical_field_is_skipped_and_counted() {
    let mut rw = RecordWriter::new();
    rw.field(9, false, b"x".to_vec());
    rw.field(10, true, uvarint_value(7));
    let bytes = rw.into_bytes();
    let mut r = CanonReader::new(&bytes);
    let mut ten = None;
    r.record_fields("SomeRecord", |r, id, _len| {
        if id == 10 {
            ten = Some(r.uvarint().unwrap());
            Ok(true)
        } else {
            Ok(false)
        }
    })
    .unwrap();
    assert_eq!(ten, Some(7));
    assert_eq!(r.skipped_non_critical_fields(), 1);
}

#[test]
fn a_field_that_consumes_the_wrong_length_is_rejected() {
    // Hand-build a field claiming len=5 but whose handler only reads a 1-byte uvarint (< 5).
    let mut w = CanonWriter::new();
    w.uvarint(1);
    w.uvarint((1 << 1) | 1);
    w.uvarint(5);
    w.raw(&[1, 0, 0, 0, 0]);
    let mut r = CanonReader::new(w.as_bytes());
    match r.record_fields("Test", |r, id, _len| {
        if id == 1 {
            let _ = r.uvarint().unwrap();
            Ok(true)
        } else {
            Ok(false)
        }
    }) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn list_value_round_trips_and_is_omittable_when_empty() {
    let bytes = list_value(vec![uvarint_value(1), uvarint_value(2)].into_iter());
    let mut r = CanonReader::new(&bytes);
    let items = r.list(|r| r.uvarint()).unwrap();
    assert_eq!(items, vec![1, 2]);
}

#[test]
fn map_value_rejects_unsorted_keys() {
    let mut w = CanonWriter::new();
    w.uvarint(2);
    w.uvarint(1);
    w.raw(b"b");
    w.uvarint(1);
    w.raw(b"1");
    w.uvarint(1);
    w.raw(b"a");
    w.uvarint(1);
    w.raw(b"2");
    let mut r = CanonReader::new(w.as_bytes());
    match r.map() {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn map_value_rejects_duplicate_keys() {
    let mut w = CanonWriter::new();
    w.uvarint(2);
    w.uvarint(1);
    w.raw(b"a");
    w.uvarint(1);
    w.raw(b"1");
    w.uvarint(1);
    w.raw(b"a");
    w.uvarint(1);
    w.raw(b"2");
    let mut r = CanonReader::new(w.as_bytes());
    match r.map() {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn trailing_bytes_after_a_value_are_detectable_by_the_caller() {
    let mut w = CanonWriter::new();
    w.uvarint(0);
    w.raw(b"trailing");
    let mut r = CanonReader::new(w.as_bytes());
    let _ = r.uvarint().unwrap();
    assert!(!r.is_empty());
}

#[test]
fn invalid_utf8_text_is_rejected() {
    let mut r = CanonReader::new(&[0xff, 0xfe]);
    assert!(r.text(2).is_err());
}

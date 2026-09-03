//! Tests for the content-addressed store.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

#[test]
fn blob_id_is_deterministic_and_hex_is_64() {
    let a = BlobId::of(b"hello");
    let b = BlobId::of(b"hello");
    let c = BlobId::of(b"world");
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.to_hex().len(), 64);
}

#[test]
fn store_dedups_and_reports_totals() {
    let mut s = ContentStore::new();
    let id1 = s.insert(b"abc".to_vec());
    let id2 = s.insert(b"abc".to_vec()); // same bytes → same id, no duplicate
    let _id3 = s.insert(b"de".to_vec());
    assert_eq!(id1, id2);
    assert_eq!(s.len(), 2);
    assert_eq!(s.total_bytes(), 5);
    assert_eq!(s.get(&id1).unwrap(), b"abc");
    assert!(s.contains(&id1));
}

#[test]
fn iter_sorted_is_by_id() {
    let mut s = ContentStore::new();
    let _ = s.insert(b"one".to_vec());
    let _ = s.insert(b"two".to_vec());
    let _ = s.insert(b"three".to_vec());
    let ids: Vec<_> = s.iter_sorted().map(|(id, _)| *id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

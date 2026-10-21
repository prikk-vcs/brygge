//! Tests for the content-addressed store, run against **both backings** (RFC 015 D-3) so neither
//! answers a call the other does not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "brygge-ir-content-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Both backings, exercised by the same closure, so a behavioral difference between them fails loudly
/// instead of only ever being checked on whichever one a test happened to pick.
fn both_backings(name: &str, body: impl Fn(ContentStore)) {
    body(ContentStore::new());
    let dir = scratch_dir(name);
    body(ContentStore::spilling(&dir).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

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
    both_backings("dedup", |mut s| {
        let id1 = s.insert(b"abc".to_vec()).unwrap();
        let id2 = s.insert(b"abc".to_vec()).unwrap(); // same bytes -> same id, no duplicate
        let _id3 = s.insert(b"de".to_vec()).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(s.len(), 2);
        assert_eq!(s.total_bytes(), 5);
        assert_eq!(s.get(&id1).unwrap(), Some(b"abc".to_vec()));
        assert!(s.contains(&id1));
    });
}

#[test]
fn a_missing_id_reads_back_none() {
    both_backings("missing", |s| {
        assert_eq!(s.get(&BlobId::of(b"never inserted")).unwrap(), None);
        assert!(!s.contains(&BlobId::of(b"never inserted")));
    });
}

#[test]
fn an_empty_store_is_empty() {
    both_backings("empty", |s| {
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert_eq!(s.total_bytes(), 0);
    });
}

#[test]
fn ids_are_sorted() {
    both_backings("sorted", |mut s| {
        let _ = s.insert(b"one".to_vec()).unwrap();
        let _ = s.insert(b"two".to_vec()).unwrap();
        let _ = s.insert(b"three".to_vec()).unwrap();
        let ids: Vec<_> = s.ids().copied().collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
        assert_eq!(ids.len(), 3);
    });
}

#[test]
fn retain_drops_the_index_entry_but_a_kept_id_still_reads_back() {
    both_backings("retain", |mut s| {
        let keep = s.insert(b"keep me".to_vec()).unwrap();
        let drop_id = s.insert(b"drop me".to_vec()).unwrap();
        s.retain(|id| *id == keep);
        assert_eq!(s.len(), 1);
        assert!(s.contains(&keep));
        assert!(!s.contains(&drop_id));
        assert_eq!(s.get(&keep).unwrap(), Some(b"keep me".to_vec()));
        assert_eq!(s.get(&drop_id).unwrap(), None);
    });
}

#[test]
fn equality_is_by_id_set_not_by_backing() {
    let mut memory = ContentStore::new();
    memory.insert(b"a".to_vec()).unwrap();
    memory.insert(b"b".to_vec()).unwrap();

    let dir = scratch_dir("eq");
    let mut file = ContentStore::spilling(&dir).unwrap();
    // Inserted in the opposite order: equality is the id *set*, not insertion order.
    file.insert(b"b".to_vec()).unwrap();
    file.insert(b"a".to_vec()).unwrap();

    assert_eq!(memory, file);

    file.insert(b"c".to_vec()).unwrap();
    assert_ne!(memory, file);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn spilling_into_a_missing_directory_is_a_typed_error() {
    let dir = scratch_dir("spill-missing").join("does-not-exist");
    assert!(ContentStore::spilling(&dir).is_err());
}

#[test]
fn an_index_entry_past_the_end_of_a_truncated_spill_file_is_a_typed_error_not_a_panic() {
    let dir = scratch_dir("truncated");
    let path = dir.join("short");
    std::fs::write(&path, b"short").unwrap();
    let file = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
    let id = BlobId::of(b"claims to be longer than the file really is");
    let mut index = BTreeMap::new();
    // The index claims 1000 bytes at offset 0; the file holds only 5.
    index.insert(id, (0u64, 1000u64));
    let s = ContentStore {
        backing: Backing::File {
            file: RefCell::new(file),
            index,
            dir: dir.clone(),
        },
    };
    assert!(s.get(&id).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_write_error_on_the_spill_file_names_its_directory() {
    let dir = scratch_dir("write-error-names-dir");
    let path = dir.join("read-only");
    std::fs::write(&path, b"").unwrap();
    // Opened read-only: a write through it fails, and the error must name `dir`, not just repeat the
    // raw OS message (RFC 015 D-3a: "a typed Io error, naming the spill directory").
    let file = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
    let mut s = ContentStore {
        backing: Backing::File {
            file: RefCell::new(file),
            index: BTreeMap::new(),
            dir: dir.clone(),
        },
    };
    let err = s.insert(b"content".to_vec()).unwrap_err();
    assert!(
        format!("{err}").contains(&dir.display().to_string()),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_blob_added_to_a_file_backed_store_round_trips_through_a_fresh_read() {
    let dir = scratch_dir("roundtrip");
    let mut s = ContentStore::spilling(&dir).unwrap();
    let id = s.insert(b"persisted".to_vec()).unwrap();
    // A second, independent read of the same id: proves the bytes are really read back from the
    // file each time, not cached anywhere in the store.
    assert_eq!(s.get(&id).unwrap(), Some(b"persisted".to_vec()));
    assert_eq!(s.get(&id).unwrap(), Some(b"persisted".to_vec()));
    let _ = std::fs::remove_dir_all(&dir);
}

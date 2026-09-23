//! Tests for the obsstore reader (RFC 005 corrections handoff §1.2). The "real record" fixture is a
//! byte-for-byte capture of `.hg/store/obsstore` from a real `hg commit --amend` with core evolution
//! enabled (`experimental.evolution.createmarkers=true`) — verified by hand against the format before
//! being pinned here, so this test does not itself depend on `hg` being installed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{Limits, precursor_nodes};
use crate::Error;

/// A real one-marker obsstore: precursor `efc181da36df...`, successor `8e1c1ad6fd72...`, `numpar = 3`
/// (parents not recorded), three metadata pairs (`ef1=9`, `operation=amend`,
/// `user="Tester <t@example.com>"`).
const REAL_AMEND_MARKER: &[u8] = &[
    0x01, 0x00, 0x00, 0x00, 0x6d, 0x41, 0xda, 0xac, 0xf4, 0x32, 0x2b, 0xac, 0xd0, 0xfd, 0xe4, 0x00,
    0x00, 0x01, 0x03, 0x03, 0xef, 0xc1, 0x81, 0xda, 0x36, 0xdf, 0x45, 0x41, 0x1e, 0x9d, 0xda, 0x75,
    0xe8, 0x99, 0xfd, 0x56, 0x44, 0x39, 0x18, 0x74, 0x8e, 0x1c, 0x1a, 0xd6, 0xfd, 0x72, 0x0a, 0xe8,
    0xfd, 0x44, 0xd4, 0xd3, 0x3f, 0xcc, 0xd8, 0x01, 0x53, 0xd4, 0x6b, 0xc3, 0x03, 0x01, 0x09, 0x05,
    0x04, 0x16, 0x65, 0x66, 0x31, 0x39, 0x6f, 0x70, 0x65, 0x72, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x61,
    0x6d, 0x65, 0x6e, 0x64, 0x75, 0x73, 0x65, 0x72, 0x54, 0x65, 0x73, 0x74, 0x65, 0x72, 0x20, 0x3c,
    0x74, 0x40, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2e, 0x63, 0x6f, 0x6d, 0x3e,
];

/// A real **prune** marker: `hg debugobsolete --record-parents <node>` on a root commit (`numsuc = 0`,
/// `numpar = 1`, recording the null-node parent explicitly) — review 009 R-1/§3.3: this is the capture
/// that exercises the parent-node path, which the amend fixture above cannot (its `numpar` is the `3`
/// sentinel). Captured with real `hg` 7.2.4, verified byte-for-byte against the format before being
/// pinned here.
const REAL_PRUNE_MARKER: &[u8] = &[
    0x01, 0x00, 0x00, 0x00, 0x57, 0x41, 0xda, 0xac, 0xf7, 0x20, 0x67, 0xd9, 0x31, 0xfd, 0xe4, 0x00,
    0x00, 0x00, 0x01, 0x01, 0x71, 0x3e, 0x60, 0xd5, 0x77, 0x5e, 0x7b, 0x66, 0xdb, 0xde, 0x47, 0x67,
    0x2c, 0xe4, 0x68, 0xe3, 0x25, 0x64, 0x9f, 0x53, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x16, 0x75, 0x73,
    0x65, 0x72, 0x54, 0x65, 0x73, 0x74, 0x65, 0x72, 0x20, 0x3c, 0x74, 0x40, 0x65, 0x78, 0x61, 0x6d,
    0x70, 0x6c, 0x65, 0x2e, 0x63, 0x6f, 0x6d, 0x3e,
];

fn temp_store(bytes: &[u8]) -> tempdir::TempDir {
    let dir = tempdir::TempDir::new();
    std::fs::write(dir.path().join("obsstore"), bytes).unwrap();
    dir
}

// A tiny local stand-in for a temp-dir helper (this crate has no `tempfile` dependency).
mod tempdir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "brygge-hg-obsstore-{}-{nanos}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn a_real_amend_marker_yields_its_precursor() {
    let dir = temp_store(REAL_AMEND_MARKER);
    let precursors = precursor_nodes(dir.path(), &Limits::default()).unwrap();
    assert_eq!(precursors.len(), 1);
    let expected: [u8; 20] = [
        0xef, 0xc1, 0x81, 0xda, 0x36, 0xdf, 0x45, 0x41, 0x1e, 0x9d, 0xda, 0x75, 0xe8, 0x99, 0xfd,
        0x56, 0x44, 0x39, 0x18, 0x74,
    ];
    assert!(precursors.contains(&expected));
}

#[test]
fn a_missing_obsstore_is_no_markers() {
    let dir = tempdir::TempDir::new();
    let precursors = precursor_nodes(dir.path(), &Limits::default()).unwrap();
    assert!(precursors.is_empty());
}

#[test]
fn version_0_is_refused() {
    let dir = temp_store(&[0x00]);
    match precursor_nodes(dir.path(), &Limits::default()) {
        Err(Error::UnsupportedFormat { .. }) => {}
        other => panic!("expected UnsupportedFormat, got {other:?}"),
    }
}

#[test]
fn a_truncated_record_is_a_typed_error_not_a_panic() {
    for cut in 1..REAL_AMEND_MARKER.len() {
        let dir = temp_store(&REAL_AMEND_MARKER[..cut]);
        // Every truncation must be a typed error (or, if it happens to still be a complete zero-record
        // file, Ok) — never a panic. The loop itself is the proof: if this panicked, the test would abort.
        let _ = precursor_nodes(dir.path(), &Limits::default());
    }
}

#[test]
fn a_record_declaring_a_size_past_eof_is_rejected() {
    let mut bytes = REAL_AMEND_MARKER.to_vec();
    // Corrupt the size field (bytes 1..5) to claim a record far larger than the file.
    bytes[1] = 0x7f;
    let dir = temp_store(&bytes);
    match precursor_nodes(dir.path(), &Limits::default()) {
        Err(Error::Read(_)) => {}
        other => panic!("expected Error::Read, got {other:?}"),
    }
}

#[test]
fn oversize_file_is_a_resource_limit() {
    let dir = temp_store(REAL_AMEND_MARKER);
    let tiny = Limits {
        max_obsstore_bytes: 4,
    };
    match precursor_nodes(dir.path(), &tiny) {
        Err(Error::ResourceLimit { .. }) => {}
        other => panic!("expected ResourceLimit, got {other:?}"),
    }
}

#[test]
fn a_32_byte_sha256_precursor_is_skipped_correctly_never_matching_anything() {
    // Hand-build a minimal record with the usingsha256 flag bit set (obsutil.py: bumpedfix = 1,
    // usingsha256 = 2 — review 009 R-1), a 32-byte precursor, no successors, the numpar-unknown
    // sentinel, and no metadata.
    let mut w = Vec::new();
    w.push(1u8); // version
    let mut precursor = vec![0xAAu8; 16];
    precursor.extend(std::iter::repeat_n(0xBBu8, 16)); // 32 bytes total, not a real sha256 but the right width
    let fixed_and_precursor_len = 19 + precursor.len();
    let size = fixed_and_precursor_len as u32;
    w.extend_from_slice(&size.to_be_bytes());
    w.extend_from_slice(&[0u8; 8]); // date
    w.extend_from_slice(&[0u8; 2]); // tz
    w.extend_from_slice(&2u16.to_be_bytes()); // flags: usingsha256
    w.push(0); // numsuc
    w.push(3); // numpar: unknown sentinel
    w.push(0); // nummeta
    w.extend_from_slice(&precursor);
    let dir = temp_store(&w);
    let precursors = precursor_nodes(dir.path(), &Limits::default()).unwrap();
    assert!(
        precursors.is_empty(),
        "a 32-byte precursor can never match a 20-byte changelog node"
    );
}

#[test]
fn a_flag_1_bumpedfix_record_still_parses_at_20_byte_node_width() {
    // Review 009 R-1: flag bit 1 (`bumpedfix`) is unrelated to node width — only bit 2 (`usingsha256`)
    // switches to 32-byte nodes. A record with flags=1 must still read a 20-byte precursor correctly.
    let mut w = Vec::new();
    w.push(1u8); // version
    let precursor = [0xCCu8; 20];
    let fixed_and_precursor_len = 19 + precursor.len();
    w.extend_from_slice(&(fixed_and_precursor_len as u32).to_be_bytes());
    w.extend_from_slice(&[0u8; 8]); // date
    w.extend_from_slice(&[0u8; 2]); // tz
    w.extend_from_slice(&1u16.to_be_bytes()); // flags: bumpedfix
    w.push(0); // numsuc
    w.push(3); // numpar: unknown sentinel
    w.push(0); // nummeta
    w.extend_from_slice(&precursor);
    let dir = temp_store(&w);
    let precursors = precursor_nodes(dir.path(), &Limits::default()).unwrap();
    assert_eq!(precursors.len(), 1);
    assert!(precursors.contains(&precursor));
}

#[test]
fn a_real_prune_marker_with_a_recorded_parent_yields_its_precursor() {
    let dir = temp_store(REAL_PRUNE_MARKER);
    let precursors = precursor_nodes(dir.path(), &Limits::default()).unwrap();
    assert_eq!(precursors.len(), 1);
    let expected: [u8; 20] = [
        0x71, 0x3e, 0x60, 0xd5, 0x77, 0x5e, 0x7b, 0x66, 0xdb, 0xde, 0x47, 0x67, 0x2c, 0xe4, 0x68,
        0xe3, 0x25, 0x64, 0x9f, 0x53,
    ];
    assert!(precursors.contains(&expected));
}

#[test]
fn multiple_records_are_all_read() {
    let mut bytes = REAL_AMEND_MARKER.to_vec();
    // A second record back to back: the fixture minus its own leading version byte (there is only one
    // version byte for the whole file, not one per record).
    bytes.extend_from_slice(&REAL_AMEND_MARKER[1..]);
    let dir = temp_store(&bytes);
    let precursors = precursor_nodes(dir.path(), &Limits::default()).unwrap();
    // Same precursor twice still dedups to one entry via the HashSet.
    assert_eq!(precursors.len(), 1);
}

// ---- release prep part 2 §4: the bound is enforced while reading, never as check-then-read ------------

#[test]
fn a_file_exactly_at_the_ceiling_is_read_and_one_byte_over_is_refused() {
    let dir = temp_store(REAL_AMEND_MARKER);
    let exactly = Limits {
        max_obsstore_bytes: REAL_AMEND_MARKER.len() as u64,
    };
    assert!(precursor_nodes(dir.path(), &exactly).is_ok());

    let one_short = Limits {
        max_obsstore_bytes: REAL_AMEND_MARKER.len() as u64 - 1,
    };
    match precursor_nodes(dir.path(), &one_short) {
        Err(Error::ResourceLimit { what, .. }) => assert_eq!(what, "the obsstore file"),
        other => panic!("expected ResourceLimit, got {other:?}"),
    }
}

#[test]
fn an_absent_obsstore_still_means_no_markers() {
    let dir = temp_store(b"");
    std::fs::remove_file(dir.path().join("obsstore")).unwrap();
    assert!(
        precursor_nodes(dir.path(), &Limits::default())
            .unwrap()
            .is_empty()
    );
}

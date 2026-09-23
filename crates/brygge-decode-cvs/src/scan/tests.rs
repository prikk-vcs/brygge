//! Resource-ceiling tests for scanning a CVS repository tree (RFC 010, CR-10). A small injected
//! [`Limits`], never gigabytes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn an_over_limit_rcs_file_is_refused_without_being_read() {
    let root = std::env::temp_dir().join(format!(
        "brygge-cvs-sparse-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let rcs_path = root.join("big.txt,v");
    // A sparse file: its declared length is huge, but no data blocks are allocated, so if brygge ever
    // actually read it fully this test would exhaust memory/time — it must not.
    let file = std::fs::File::create(&rcs_path).unwrap();
    file.set_len(10 * 1024 * 1024 * 1024).unwrap(); // 10 GiB, sparse
    drop(file);

    let limits = Limits {
        max_rcs_bytes: 1024,
    };
    let start = Instant::now();
    let result = scan_with(&root, &limits);
    let elapsed = start.elapsed();
    let _ = std::fs::remove_dir_all(&root);

    match result {
        Err(crate::Error::ResourceLimit { .. }) => {}
        Ok(_) => panic!("expected a resource-limit refusal, got Ok"),
        Err(other) => panic!("expected a resource-limit refusal, got {other}"),
    }
    assert!(
        elapsed < Duration::from_secs(2),
        "refusal took {elapsed:?} — the metadata check should reject a 10 GiB sparse file instantly, \
         proving the file was never read"
    );
}

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

fn minimal_rcs(rev: &str, date: &str) -> Vec<u8> {
    format!(
        "head\t{rev};\naccess;\nsymbols;\nlocks; strict;\n\n\n\
{rev}\ndate\t{date};\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
{rev}\nlog\n@x@\ntext\n@x\n@\n"
    )
    .into_bytes()
}

fn fresh_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "brygge-cvs-scan-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn a_symlink_anywhere_under_the_root_is_refused() {
    let root = fresh_root("symlink");
    std::fs::write(
        root.join("real.c,v"),
        minimal_rcs("1.1", "2024.01.01.00.00.00"),
    )
    .unwrap();
    let link = root.join("link.c,v");
    std::os::unix::fs::symlink(root.join("real.c,v"), &link).unwrap();

    let result = scan(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, crate::floor::SYMLINK_IN_REPOSITORY);
        }
        Ok(_) => panic!("expected FloorRefusal(symlink-in-repository), got Ok"),
        Err(other) => panic!("expected FloorRefusal(symlink-in-repository), got {other}"),
    }
}

#[test]
fn a_symlinked_directory_is_refused() {
    let root = fresh_root("symlinkdir");
    let real_dir = root.join("real_dir");
    std::fs::create_dir_all(&real_dir).unwrap();
    std::os::unix::fs::symlink(&real_dir, root.join("link_dir")).unwrap();

    let result = scan(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, crate::floor::SYMLINK_IN_REPOSITORY);
        }
        Ok(_) => panic!("expected FloorRefusal(symlink-in-repository), got Ok"),
        Err(other) => panic!("expected FloorRefusal(symlink-in-repository), got {other}"),
    }
}

#[test]
fn the_same_path_in_attic_and_live_is_refused() {
    let root = fresh_root("attic");
    std::fs::write(
        root.join("f.c,v"),
        minimal_rcs("1.1", "2024.01.01.00.00.00"),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("Attic")).unwrap();
    std::fs::write(
        root.join("Attic/f.c,v"),
        minimal_rcs("1.1", "2024.01.01.00.00.00"),
    )
    .unwrap();

    let result = scan(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, crate::floor::PATH_IN_ATTIC_AND_LIVE);
        }
        Ok(_) => panic!("expected FloorRefusal(path-in-attic-and-live), got Ok"),
        Err(other) => panic!("expected FloorRefusal(path-in-attic-and-live), got {other}"),
    }
}

#[test]
fn a_non_utf8_path_component_is_refused_and_shown_as_escaped_bytes() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let root = fresh_root("nonutf8");
    let bad_name = OsStr::from_bytes(b"bad_\xff_name,v");
    std::fs::write(
        root.join(bad_name),
        minimal_rcs("1.1", "2024.01.01.00.00.00"),
    )
    .unwrap();

    let result = scan(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Err(crate::Error::FloorRefusal { feature, reason }) => {
            assert_eq!(feature, crate::floor::NON_UTF8_PATH);
            assert!(reason.contains("\\xFF"), "reason: {reason}");
        }
        Ok(_) => panic!("expected FloorRefusal(non-utf8-path), got Ok"),
        Err(other) => panic!("expected FloorRefusal(non-utf8-path), got {other}"),
    }
}

#[test]
fn a_parse_error_names_the_file_it_came_from() {
    // Review 008 F-4: "unparseable RCS date for revision 1.1" alone is not actionable — the error names
    // the `,v` file.
    let root = fresh_root("errpath");
    std::fs::write(root.join("bad.c,v"), minimal_rcs("1.1", "not-a-date")).unwrap();
    let result = scan(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Err(crate::Error::Read(m)) => {
            assert!(m.contains("bad.c,v"), "error must name the file: {m}");
            assert!(m.contains("unparseable RCS date"), "{m}");
        }
        Ok(_) => panic!("expected Error::Read, got Ok"),
        Err(other) => panic!("expected Error::Read, got {other}"),
    }
}

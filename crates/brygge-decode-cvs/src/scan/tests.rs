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

/// Create a link `link` → `target` of the given kind, or say why it could not be. A test that cannot make the
/// link (Windows without the symlink privilege) is skipped with a message on a developer machine, and
/// **fails on CI**, so a skip can never hide a missing proof there.
fn link_created(what: &str, result: std::io::Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("SKIPPED: cannot create {what} here: {e}");
            false
        }
        Err(e) => panic!("cannot create {what} on CI: {e}"),
    }
}

#[cfg(unix)]
fn symlink_file(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(unix)]
fn symlink_dir(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_file(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

/// A directory junction (`mklink /J`): a reparse point that is not a symlink and needs no privilege.
#[cfg(windows)]
fn junction(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "mklink /J exited with {status}"
        )))
    }
}

fn assert_symlink_refusal(result: Result<Vec<CvsFile>, crate::Error>) {
    match result {
        Err(crate::Error::FloorRefusal { feature, .. }) => {
            assert_eq!(feature, crate::floor::SYMLINK_IN_REPOSITORY);
        }
        Ok(_) => panic!("expected FloorRefusal(symlink-in-repository), got Ok"),
        Err(other) => panic!("expected FloorRefusal(symlink-in-repository), got {other}"),
    }
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
    let made = link_created(
        "a file symlink",
        symlink_file(&root.join("real.c,v"), &link),
    );

    let result = if made { Some(scan(&root)) } else { None };
    let _ = std::fs::remove_dir_all(&root);
    if let Some(result) = result {
        assert_symlink_refusal(result);
    }
}

#[test]
fn a_symlinked_directory_is_refused() {
    let root = fresh_root("symlinkdir");
    let real_dir = root.join("real_dir");
    std::fs::create_dir_all(&real_dir).unwrap();
    let made = link_created(
        "a directory symlink",
        symlink_dir(&real_dir, &root.join("link_dir")),
    );

    let result = if made { Some(scan(&root)) } else { None };
    let _ = std::fs::remove_dir_all(&root);
    if let Some(result) = result {
        assert_symlink_refusal(result);
    }
}

/// RFC 012 D-9 / threat model C-2c: a directory junction is refused like a symlink. `std` reports a
/// junction as a symlink (its reparse tag is a "name surrogate"), so no Windows-specific product code is
/// needed; this test is the proof, and it runs on `windows-latest`.
#[cfg(windows)]
#[test]
fn a_directory_junction_is_refused() {
    let root = fresh_root("junction");
    let real_dir = root.join("real_dir");
    std::fs::create_dir_all(&real_dir).unwrap();
    std::fs::write(
        real_dir.join("f.c,v"),
        minimal_rcs("1.1", "2024.01.01.00.00.00"),
    )
    .unwrap();
    let link = root.join("junction_dir");
    assert!(link_created(
        "a directory junction",
        junction(&real_dir, &link)
    ));
    // What `std` says about it, so the review can quote it.
    let ft = std::fs::symlink_metadata(&link).unwrap().file_type();
    eprintln!(
        "junction: is_symlink={} is_dir={} is_file={}",
        ft.is_symlink(),
        ft.is_dir(),
        ft.is_file()
    );

    let result = scan(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert_symlink_refusal(result);
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

/// A `,v` file name that is not valid Unicode: raw bytes on Unix, an unpaired surrogate on Windows.
#[cfg(unix)]
fn non_unicode_name() -> std::ffi::OsString {
    use std::os::unix::ffi::OsStrExt as _;
    std::ffi::OsStr::from_bytes(b"bad_\xff_name,v").to_owned()
}

#[cfg(windows)]
fn non_unicode_name() -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt as _;
    let mut wide: Vec<u16> = "bad_".encode_utf16().collect();
    wide.push(0xD800); // an unpaired high surrogate
    wide.extend("_name,v".encode_utf16());
    std::ffi::OsString::from_wide(&wide)
}

/// The escaped form of the offending byte(s) that the refusal must show: `0xFF` on Unix; on Windows the
/// unpaired surrogate U+D800 is WTF-8 `ED A0 80`.
#[cfg(unix)]
const NON_UNICODE_ESCAPED: &str = "\\xFF";
#[cfg(windows)]
const NON_UNICODE_ESCAPED: &str = "\\xED\\xA0\\x80";

#[test]
fn a_non_utf8_path_component_is_refused_and_shown_as_escaped_bytes() {
    let root = fresh_root("nonutf8");
    std::fs::write(
        root.join(non_unicode_name()),
        minimal_rcs("1.1", "2024.01.01.00.00.00"),
    )
    .unwrap();

    let result = scan(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Err(crate::Error::FloorRefusal { feature, reason }) => {
            assert_eq!(feature, crate::floor::NON_UTF8_PATH);
            assert!(reason.contains(NON_UNICODE_ESCAPED), "reason: {reason}");
        }
        Ok(_) => panic!("expected FloorRefusal(non-utf8-path), got Ok"),
        Err(other) => panic!("expected FloorRefusal(non-utf8-path), got {other}"),
    }
}

/// Platform-independent: the escape of the WTF-8 bytes of an unpaired surrogate (what
/// `as_encoded_bytes` yields on Windows) is lossless and names every byte.
#[test]
fn an_unpaired_surrogate_in_wtf8_is_escaped_byte_by_byte() {
    assert_eq!(
        escape_invalid_utf8(b"bad_\xED\xA0\x80_name,v"),
        "bad_\\xED\\xA0\\x80_name,v"
    );
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

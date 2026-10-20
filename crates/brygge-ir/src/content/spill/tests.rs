#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::{Read, Seek, SeekFrom, Write};

use super::create;

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "brygge-ir-spill-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_missing_directory_is_a_typed_error() {
    let dir = scratch_dir("missing");
    let missing = dir.join("does-not-exist");
    assert!(create(&missing).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_given_instead_of_a_directory_is_a_typed_error() {
    let dir = scratch_dir("notadir");
    let file_path = dir.join("a-file");
    std::fs::write(&file_path, b"x").unwrap();
    assert!(create(&file_path).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_created_file_is_readable_and_writable_through_the_returned_handle() {
    let dir = scratch_dir("rw");
    let mut f = create(&dir).unwrap();
    f.write_all(b"hello").unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"hello");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_calls_in_the_same_process_never_collide() {
    let dir = scratch_dir("twice");
    let mut a = create(&dir).unwrap();
    let mut b = create(&dir).unwrap();
    a.write_all(b"a").unwrap();
    b.write_all(b"b").unwrap();
    a.seek(SeekFrom::Start(0)).unwrap();
    b.seek(SeekFrom::Start(0)).unwrap();
    let mut buf_a = Vec::new();
    let mut buf_b = Vec::new();
    a.read_to_end(&mut buf_a).unwrap();
    b.read_to_end(&mut buf_b).unwrap();
    assert_eq!(buf_a, b"a");
    assert_eq!(buf_b, b"b");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn on_unix_the_directory_holds_no_spill_file_once_created() {
    let dir = scratch_dir("unlinked");
    let _f = create(&dir).unwrap();
    let remaining: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert!(
        remaining.is_empty(),
        "the directory entry must be gone the moment the file is created: {remaining:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn on_unix_the_file_is_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("mode");
    let f = create(&dir).unwrap();
    let mode = f.metadata().unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "got {mode:o}");
    let _ = std::fs::remove_dir_all(&dir);
}

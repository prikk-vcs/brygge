//! Create a private spill file (RFC 015 D-3a): a unique name in the given directory, `create_new` so a
//! pre-existing file or symlink there is never followed or truncated, mode `0600` on Unix — and never
//! left behind, however the process ends:
//! - **Unix:** the directory entry is removed right after opening; the open handle keeps the data
//!   alive (and readable, by seek) until it is dropped, whatever happens to the process meanwhile.
//! - **Windows:** opened with `FILE_FLAG_DELETE_ON_CLOSE`, so the OS removes it when the handle closes
//!   — including on an abrupt process exit.
//!
//! No `unsafe`: both are plain, safe `OpenOptions` flags (`forbid(unsafe_code)` holds crate-wide).

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Error, Result};

/// Bounded so a persistently hostile or exhausted directory fails loudly instead of looping forever.
const MAX_ATTEMPTS: u32 = 64;

pub(super) fn create(dir: &Path) -> Result<File> {
    if !dir.is_dir() {
        return Err(Error::io_msg(format!(
            "spill directory {} does not exist or is not a directory",
            dir.display()
        )));
    }
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::io_msg(e.to_string()))?
        .as_nanos();
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        let path = dir.join(format!(".brygge-spill-{pid}-{nanos}-{attempt}"));
        match open_new(&path) {
            Ok(file) => {
                remove_entry(&path)?;
                return Ok(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
            }
            Err(e) => return Err(Error::io(e)),
        }
    }
    #[allow(clippy::expect_used)]
    Err(Error::io(last_err.expect(
        "the loop above ran at least once, so a collision on every attempt means it set this",
    )))
}

#[cfg(unix)]
fn open_new(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(windows)]
fn open_new(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_new(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
}

/// Unix: the directory entry is the one thing left to remove — the open handle already keeps the
/// data alive. Windows (and any other target): `open_new` already arranged removal on close, or there
/// is no such mechanism to arrange; nothing more to do here.
#[cfg(unix)]
fn remove_entry(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(Error::io)
}

#[cfg(not(unix))]
fn remove_entry(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;

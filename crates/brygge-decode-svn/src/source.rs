//! Resolving the SVN source into dumpstream bytes (RFC 006 D-1, Tier D).
//!
//! Two forms: a **dumpfile** the operator supplies (no subprocess), or a **local repository** brygge
//! dumps with a read-only `svnadmin dump` (a fixed argument vector, no shell, no network). A **remote /
//! URL source is refused** (INV-3): brygge never dumps over the network, and `svnrdump` is out of scope.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Error;

/// The maximum dumpstream size brygge will hold in memory (RFC 006 security review, T-8). Correctness
/// first; streaming a larger dump is OQ-F (deferred). Refused, not truncated, above this.
const MAX_DUMP_BYTES: usize = 8 * 1024 * 1024 * 1024;

/// Where to read the SVN history from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A path to a fulltext SVN dumpstream file (`svnadmin dump` output). No subprocess is run.
    DumpFile(PathBuf),
    /// A path to a **local** SVN repository; brygge runs a read-only `svnadmin dump` on it.
    LocalRepo(PathBuf),
}

impl Source {
    /// Load the dumpstream bytes for this source.
    ///
    /// # Errors
    /// [`Error::Open`] if the file is unreadable, the repo path is a URL, or `svnadmin` fails;
    /// [`Error::ResourceLimit`] if the dumpstream exceeds [`MAX_DUMP_BYTES`].
    pub fn load(&self) -> Result<Vec<u8>, Error> {
        match self {
            Self::DumpFile(path) => {
                let bytes = std::fs::read(path).map_err(|e| {
                    Error::Open(format!("cannot read dumpfile {}: {e}", path.display()))
                })?;
                check_size(bytes.len())?;
                Ok(bytes)
            }
            Self::LocalRepo(path) => dump_local_repo(path),
        }
    }
}

fn check_size(len: usize) -> Result<(), Error> {
    if len > MAX_DUMP_BYTES {
        return Err(Error::ResourceLimit {
            limit: format!("dumpstream over {MAX_DUMP_BYTES} bytes"),
        });
    }
    Ok(())
}

/// Run a read-only `svnadmin dump` on a local repository, capturing the fulltext dumpstream from stdout.
fn dump_local_repo(path: &Path) -> Result<Vec<u8>, Error> {
    // A URL is not a local repository; brygge does not dump over the network (INV-3).
    let shown = path.to_string_lossy();
    if shown.contains("://") {
        return Err(Error::FloorRefusal {
            feature: "remote-source".to_string(),
            reason: "brygge dumps only a local repository; a URL/remote source is refused, and \
                     `svnrdump` (network) is out of scope (INV-3, RFC 006 §4)"
                .to_string(),
        });
    }
    // `svnadmin dump <repo> --quiet` writes a fulltext dumpstream to stdout (no `--deltas`), progress to
    // stderr (suppressed). Fixed argv, no shell. It performs no network I/O and runs no repository hooks.
    let output = Command::new("svnadmin")
        .arg("dump")
        .arg(path)
        .arg("--quiet")
        .output()
        .map_err(|e| Error::Open(format!("failed to run `svnadmin dump`: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Open(format!(
            "`svnadmin dump {}` failed: {}",
            path.display(),
            stderr.trim()
        )));
    }
    check_size(output.stdout.len())?;
    Ok(output.stdout)
}

//! Walk a CVS repository tree and read its RCS `,v` files (RFC 007 §1). Maps store paths to repo-relative
//! paths: `dir/foo.c,v` → `dir/foo.c`, and `dir/Attic/foo.c,v` → `dir/foo.c` (a mainline-deleted file).
//! Skips the `CVSROOT` administrative directory. Bounds the file count (T-8). No path is ever converted
//! lossily (CR-03): a non-UTF-8 path component is refused, shown as `\xNN`; a symlink anywhere under the
//! root, and the same repo-relative path existing in both `Attic/` and live, are refused too (CR-08.3/4).

use std::collections::BTreeSet;
use std::io::Read;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use crate::rcs::{self, MAX_RCS_BYTES, RcsFile};
use crate::{Error, floor};

/// The maximum number of `,v` files read from one repository (a malformed/hostile tree must not exhaust
/// the host); OQ-F streaming is deferred.
const MAX_FILES: usize = 5_000_000;

/// Resource ceilings for scanning a CVS repository tree (RFC 010 D-4). Tests construct a small instance
/// instead of needing gigabytes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The maximum size of one `,v` file (RFC 010 CR-10).
    pub(crate) max_rcs_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rcs_bytes: MAX_RCS_BYTES as u64,
        }
    }
}

/// One versioned file: its repo-relative path and parsed RCS history.
pub struct CvsFile {
    /// Repo-relative path (no `,v`, no `Attic/` component).
    pub path: String,
    /// The parsed RCS file.
    pub rcs: RcsFile,
}

/// Read every `,v` file under `root`.
///
/// # Errors
/// [`Error::Read`] on a malformed `,v`; [`Error::ResourceLimit`] past [`MAX_FILES`] or the RCS file size
/// ceiling; [`Error::FloorRefusal`] on a symlink, a non-UTF-8 path, or a path present in both `Attic/`
/// and live.
pub fn scan(root: &Path) -> Result<Vec<CvsFile>, Error> {
    scan_with(root, &Limits::default())
}

pub(crate) fn scan_with(root: &Path, limits: &Limits) -> Result<Vec<CvsFile>, Error> {
    let mut out = Vec::new();
    let mut seen_paths: BTreeSet<String> = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| {
            Error::Read(format!("cannot read directory {}: {e}", display_path(&dir)))
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::Read(format!("directory entry error: {e}")))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|e| Error::Read(format!("file type error: {e}")))?;
            if file_type.is_symlink() {
                return Err(Error::FloorRefusal {
                    feature: floor::SYMLINK_IN_REPOSITORY.to_string(),
                    reason: format!(
                        "{} is a symlink; brygge reads only the repository it is given",
                        display_path(&path)
                    ),
                });
            }
            if file_type.is_dir() {
                // Skip the CVSROOT admin directory (config, not history).
                if path.file_name().is_some_and(|n| n == "CVSROOT") && dir == root {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && ends_with_comma_v(&path) {
                if out.len() >= MAX_FILES {
                    return Err(Error::ResourceLimit {
                        what: "the `,v` file count".to_string(),
                        ceiling: format!("{MAX_FILES} files"),
                    });
                }
                let rel = repo_path(root, &path)?;
                if !seen_paths.insert(rel.clone()) {
                    return Err(Error::FloorRefusal {
                        feature: floor::PATH_IN_ATTIC_AND_LIVE.to_string(),
                        reason: format!(
                            "'{rel}' exists as both a live and an Attic ,v file; the repository is \
                             inconsistent — repair it with `cvs admin` or by hand before importing"
                        ),
                    });
                }
                let bytes = read_bounded(&path, limits.max_rcs_bytes)?;
                // Review 008 F-4: a parse error names the file it came from (losslessly, `\xNN` for
                // non-UTF-8 bytes) — a bare "unparseable RCS date for revision 1.2" is not actionable.
                let rcs = rcs::parse_rcs(&bytes).map_err(|e| match e {
                    Error::Read(m) => Error::Read(format!("{}: {m}", display_path(&path))),
                    other => other,
                })?;
                out.push(CvsFile { path: rel, rcs });
            }
        }
    }
    // Deterministic order regardless of directory iteration order.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// True when `path`'s final component ends in `,v`, checked on raw bytes (never a lossy conversion).
fn ends_with_comma_v(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.as_bytes().ends_with(b",v"))
}

/// Read a `,v` file into memory, refusing before allocation if its size already exceeds
/// `max_rcs_bytes` (CR-10) — checked from filesystem metadata, not after a full read. Reading itself is
/// still bounded with `take(max_rcs_bytes + 1)` in case the file grows after the metadata check.
fn read_bounded(path: &Path, max_rcs_bytes: u64) -> Result<Vec<u8>, Error> {
    let meta = std::fs::metadata(path)
        .map_err(|e| Error::Read(format!("cannot stat {}: {e}", display_path(path))))?;
    if meta.len() > max_rcs_bytes {
        return Err(Error::ResourceLimit {
            what: "an RCS file".to_string(),
            ceiling: format!("{max_rcs_bytes} bytes"),
        });
    }
    let file = std::fs::File::open(path)
        .map_err(|e| Error::Read(format!("cannot read {}: {e}", display_path(path))))?;
    let mut bytes = Vec::new();
    file.take(max_rcs_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Read(format!("cannot read {}: {e}", display_path(path))))?;
    if bytes.len() as u64 > max_rcs_bytes {
        return Err(Error::ResourceLimit {
            what: "an RCS file".to_string(),
            ceiling: format!("{max_rcs_bytes} bytes"),
        });
    }
    Ok(bytes)
}

/// Render `bytes` as valid UTF-8 kept verbatim and each invalid byte escaped as `\xNN` — never a lossy
/// substitution, which would silently alter the bytes a refusal message names (CR-03). Shared with
/// `rcs.rs` for symbol-name refusals (review 008 R-4): no lossy conversion anywhere in this crate.
pub(crate) fn escape_invalid_utf8(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                let valid_prefix = rest.get(..valid_up_to).unwrap_or(&[]);
                out.push_str(std::str::from_utf8(valid_prefix).unwrap_or_default());
                let bad_len = e.error_len().unwrap_or(rest.len() - valid_up_to).max(1);
                let bad_end = (valid_up_to + bad_len).min(rest.len());
                for &b in rest.get(valid_up_to..bad_end).unwrap_or(&[]) {
                    let _ = write!(out, "\\x{b:02X}");
                }
                rest = rest.get(bad_end..).unwrap_or(&[]);
                if rest.is_empty() {
                    break;
                }
            }
        }
    }
    out
}

/// A path rendered for a refusal/error message: valid UTF-8 verbatim, invalid bytes escaped (never a
/// lossy conversion — CR-03).
fn display_path(path: &Path) -> String {
    escape_invalid_utf8(path.as_os_str().as_bytes())
}

#[cfg(test)]
mod tests;

/// Map a `,v` store path under `root` to its repo-relative path, refusing a non-UTF-8 component.
///
/// # Errors
/// [`Error::FloorRefusal`] if any path component is not valid UTF-8.
fn repo_path(root: &Path, file: &Path) -> Result<String, Error> {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let mut parts: Vec<String> = Vec::new();
    for c in rel.components() {
        let bytes = c.as_os_str().as_bytes();
        match std::str::from_utf8(bytes) {
            Ok(s) => parts.push(s.to_string()),
            Err(_) => {
                return Err(Error::FloorRefusal {
                    feature: floor::NON_UTF8_PATH.to_string(),
                    reason: format!(
                        "path component '{}' is not valid UTF-8 (invalid bytes shown as \\xNN)",
                        escape_invalid_utf8(bytes)
                    ),
                });
            }
        }
    }
    // Strip the trailing ",v".
    if let Some(last) = parts.last_mut() {
        if let Some(stripped) = last.strip_suffix(",v") {
            *last = stripped.to_string();
        }
    }
    // Remove an `Attic` directory component (a mainline-deleted file lives in Attic/).
    if parts.len() >= 2 {
        let attic_idx = parts.len() - 2;
        if parts.get(attic_idx).is_some_and(|p| p == "Attic") {
            parts.remove(attic_idx);
        }
    }
    Ok(parts.join("/"))
}

//! Walk a CVS repository tree and read its RCS `,v` files (RFC 007 §1). Maps store paths to repo-relative
//! paths: `dir/foo.c,v` → `dir/foo.c`, and `dir/Attic/foo.c,v` → `dir/foo.c` (a mainline-deleted file).
//! Skips the `CVSROOT` administrative directory. Bounds the file count (T-8).

use std::path::Path;

use crate::Error;
use crate::rcs::{self, RcsFile};

/// The maximum number of `,v` files read from one repository (a malformed/hostile tree must not exhaust
/// the host); OQ-F streaming is deferred.
const MAX_FILES: usize = 5_000_000;

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
/// [`Error::Read`] on a malformed `,v`; [`Error::ResourceLimit`] past [`MAX_FILES`].
pub fn scan(root: &Path) -> Result<Vec<CvsFile>, Error> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| Error::Read(format!("cannot read directory {}: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::Read(format!("directory entry error: {e}")))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|e| Error::Read(format!("file type error: {e}")))?;
            if file_type.is_dir() {
                // Skip the CVSROOT admin directory (config, not history).
                if path.file_name().is_some_and(|n| n == "CVSROOT") && dir == root {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && path.to_string_lossy().ends_with(",v") {
                if out.len() >= MAX_FILES {
                    return Err(Error::ResourceLimit {
                        limit: format!("more than {MAX_FILES} ,v files"),
                    });
                }
                let rel = repo_path(root, &path);
                let bytes = std::fs::read(&path)
                    .map_err(|e| Error::Read(format!("cannot read {}: {e}", path.display())))?;
                let rcs = rcs::parse_rcs(&bytes)?;
                out.push(CvsFile { path: rel, rcs });
            }
        }
    }
    // Deterministic order regardless of directory iteration order.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Map a `,v` store path under `root` to its repo-relative path.
fn repo_path(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
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
    parts.join("/")
}

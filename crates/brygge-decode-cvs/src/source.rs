//! Resolving the CVS source (RFC 007 D-1, Tier R): a **local** CVS repository — a directory of RCS `,v`
//! files (deleted files under `Attic/`). A **remote / `:pserver:` / URL source is refused** (INV-3):
//! brygge reads local files only, runs no `cvs` client, and contacts no network.

use std::path::PathBuf;

use crate::Error;

/// Where to read the CVS history from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A path to a **local** CVS repository (module) directory of RCS `,v` files.
    LocalRepo(PathBuf),
}

impl Source {
    /// Validate the source and return its repository root.
    ///
    /// # Errors
    /// [`Error::FloorRefusal`] for a remote/`:pserver:` source; [`Error::Open`] if the path is not a
    /// directory.
    pub fn resolve(&self) -> Result<&PathBuf, Error> {
        match self {
            Self::LocalRepo(path) => {
                let shown = path.to_string_lossy();
                // A CVSROOT of the form `:pserver:…`/`:ext:…`, or a URL, is remote (INV-3).
                if shown.starts_with(':') || shown.contains("://") {
                    return Err(Error::FloorRefusal {
                        feature: "remote-source".to_string(),
                        reason: "brygge reads a local CVS repository directly; a :pserver:/:ext:/URL \
                                 source is refused, and no `cvs` client is run (INV-3, RFC 007 §D-1)"
                            .to_string(),
                    });
                }
                if !path.is_dir() {
                    return Err(Error::Open(format!(
                        "{} is not a directory (expected a local CVS repository of ,v files)",
                        path.display()
                    )));
                }
                Ok(path)
            }
        }
    }
}

//! Open a Git repository under the **locked-down configuration** (RFC 004 D-6 / RFC 009 D-4) and apply
//! the repository-level floor checks (RFC 004 D-4). One configuration serves both determinism and the
//! untrusted-input guarantee: no ambient config, no credentials, no network, and no source-provided code.

use std::path::Path;

use crate::Error;

/// Open `path` in isolation: gix's `isolated()` options read **no** global/system/environment config and
/// use **no** credentials, so the decode depends only on the repository's own objects (determinism, VF-1)
/// and cannot be steered by ambient state (INV-2/T-2). brygge enables no gix network feature (INV-3) and
/// never checks out (so no filter/smudge runs); blob bytes are read raw from the object database.
///
/// # Errors
/// [`Error::Open`] if the path is not a readable Git repository.
pub fn open(path: &Path) -> Result<gix::Repository, Error> {
    gix::open_opts(path, gix::open::Options::isolated()).map_err(|e| {
        Error::Open(format!(
            "cannot open Git repository at {}: {e}",
            path.display()
        ))
    })
}

/// Refuse the repository-level floor features (RFC 004 D-4, owner-ratified): grafts and shallow clones
/// rewrite or truncate the history a reader would otherwise see; importing them silently is exactly the
/// laundering the project forbids. (Submodules are caught per-entry during tree walk; replace refs are
/// caught while scanning refs.)
///
/// # Errors
/// [`Error::FloorRefusal`] naming the refused feature (`FA-3`).
pub fn check_repo_floor(repo: &gix::Repository) -> Result<(), Error> {
    let git_dir = repo.git_dir();
    if git_dir.join("shallow").exists() {
        return Err(Error::FloorRefusal {
            feature: "shallow clone".to_string(),
            reason: "a shallow clone is a truncated history that would look whole; refused rather \
                     than imported as if complete (FA-1)"
                .to_string(),
        });
    }
    if git_dir.join("info").join("grafts").exists() {
        return Err(Error::FloorRefusal {
            feature: "grafts".to_string(),
            reason:
                "grafts rewrite the ancestry a reader would see; refused rather than importing \
                     the rewritten view silently"
                    .to_string(),
        });
    }
    Ok(())
}

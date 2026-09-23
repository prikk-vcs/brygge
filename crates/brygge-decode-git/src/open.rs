//! Open a Git repository under the **locked-down configuration** (RFC 004 D-6 / RFC 009 D-4) and apply
//! the repository-level floor checks (RFC 004 D-4). One configuration serves both determinism and the
//! untrusted-input guarantee: no ambient config, no credentials, no network, and no source-provided code.
//!
//! **CR-11 finding.** gix's `open::Options::isolated()` denies environment-sourced *configuration*
//! (`Permissions::isolated()`, `gix-0.87.1/src/open/permissions.rs:195-201`, denying `Config`,
//! `Attributes` and `Environment` — none of which has an "alternates" or "object database" field). It
//! does **not** gate repository-*shape* resolution: `ThreadSafeRepository::open_opts` (which
//! `gix::open_opts` calls, `gix-0.87.1/src/open/repository.rs:70-107`) unconditionally resolves a
//! `.git` **file** (a `gitdir:` redirect) via `gix_discover::is_git`/`from_dot_git_dir` at line 99, and
//! `open_from_paths` (called by `open_opts`) unconditionally reads a `commondir` file at line 188 —
//! both outside the `Permissions` destructure and unaffected by `isolated()`. Separately, `gix-odb`'s
//! store init (`gix-odb-0.84.0/src/store_impls/dynamic/init.rs:111`, `crate::alternate::resolve(...)`,
//! reached via the default `Slots::AsNeededByDiskState`) always follows `objects/info/alternates`; the
//! `gix-odb` crate has no concept of `gix`'s open permissions at all. So **alternates, `gitdir:`
//! redirects and `commondir` are all followed regardless of `isolated()`** — hence the explicit,
//! pre-open filesystem checks below (owner ruling D-3(ii)), which run unconditionally (RFC 004 §6
//! prohibits making them depend on this finding).
//!
//! `GIT_DIR` is read only by `EnvironmentOverrides::from_env` (`gix-0.87.1/src/open/repository.rs:38-49`),
//! which is reached only from `ThreadSafeRepository::open_with_environment_overrides` — a distinct entry
//! point this decoder never calls (it calls `open_opts`, whose own `EnvironmentOverrides` never enters
//! the picture). `GIT_OBJECT_DIRECTORY` and `GIT_ALTERNATE_OBJECT_DIRECTORIES` appear nowhere in `gix`
//! or `gix-odb`; their only occurrence in the dependency tree is
//! `gix-path-0.12.6/src/env/git/mod.rs:171-172`, inside a helper that shells out to the `git` binary to
//! enumerate config file locations for the `Config::git_binary` permission — a permission that defaults
//! to `false` in both `Config::all()` and `Config::isolated()` (`gix-0.87.1/src/open/permissions.rs:39,51`)
//! and that this decoder never enables, so that helper is unreachable here. None of these three
//! environment variables affect this decoder's behaviour; the environment does not need to be cleared.

use std::path::{Path, PathBuf};

use crate::{Error, floor};

fn redirected(reason: String) -> Error {
    Error::FloorRefusal {
        feature: floor::REDIRECTED_GIT_DIRECTORY.to_string(),
        reason,
    }
}

fn alternates(reason: String) -> Error {
    Error::FloorRefusal {
        feature: floor::OBJECT_ALTERNATES.to_string(),
        reason,
    }
}

/// Refuse `p` (`redirected git directory`) if it is itself a symlink, checked with `symlink_metadata`
/// (never following it first) — a symlink here makes an object read leave the given repository exactly
/// as a `gitdir:` redirect or `commondir` does (D-3(ii), 2026-09-23 review R-1 of this handoff). `p` is
/// rendered with `{:?}` (never `Display`) so any unusual byte in the path is escaped, not printed raw.
fn refuse_if_symlink(p: &Path) -> Result<(), Error> {
    if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(redirected(format!(
            "{p:?} is a symlink; brygge reads only the repository it is given; replace the symlink \
             with the real directory, or point brygge at the directory it links to"
        )));
    }
    Ok(())
}

/// The git directory `path` resolves to, without following a `gitdir:` redirect: `path/.git` if that is
/// a directory, else `path` itself (which may already be a git directory).
fn git_dir_of(path: &Path) -> PathBuf {
    let dot_git = path.join(".git");
    if dot_git.is_dir() {
        dot_git
    } else {
        path.to_path_buf()
    }
}

/// Repository-shape refusals (CR-11, D-3(ii)): checked from the filesystem, before `gix` ever opens the
/// repository, because `gix` itself would silently follow every one of these (see the module doc's
/// CR-11 finding).
fn check_repository_shape(path: &Path) -> Result<(), Error> {
    let dot_git = path.join(".git");
    // R-1: `.git` itself may be a symlink (to another repository's git directory, or anywhere else).
    // Checked with `symlink_metadata`, before `is_file`/`is_dir` below would otherwise follow it. The
    // given `path` itself may be a symlink; that is the operator's own choice.
    refuse_if_symlink(&dot_git)?;

    if dot_git.is_file() {
        return Err(redirected(format!(
            "{} is a `gitdir:` redirect (a linked worktree or a submodule checkout); point brygge \
             at the repository's main git directory (`git rev-parse --git-common-dir`)",
            path.display()
        )));
    }

    let git_dir = git_dir_of(path);
    if git_dir.join("commondir").is_file() {
        return Err(redirected(format!(
            "{} is a linked worktree's git directory (it contains `commondir`); point brygge at the \
             repository's main git directory (`git rev-parse --git-common-dir`)",
            git_dir.display()
        )));
    }

    // R-1: a symlinked `objects`, `objects/info` or `objects/pack` directory — or a symlinked entry
    // directly inside `objects/pack/` — makes reads leave the given repository exactly as an
    // alternates file or a `gitdir:` redirect does. A symlinked *loose* object is a recorded residual
    // (`RR-git-loose-object-symlink`), not a refusal here: scanning every loose object costs O(objects).
    let objects = git_dir.join("objects");
    refuse_if_symlink(&objects)?;
    refuse_if_symlink(&objects.join("info"))?;
    let pack = objects.join("pack");
    refuse_if_symlink(&pack)?;
    if let Ok(entries) = std::fs::read_dir(&pack) {
        for entry in entries.flatten() {
            refuse_if_symlink(&entry.path())?;
        }
    }

    let info = objects.join("info");
    if std::fs::metadata(info.join("alternates")).is_ok_and(|m| m.len() > 0) {
        return Err(alternates(
            "the repository borrows objects from another store; brygge reads only the repository \
             it is given; run `git repack -a -d` in it (which copies borrowed objects in) and then \
             remove `objects/info/alternates`"
                .to_string(),
        ));
    }
    if info.join("http-alternates").exists() {
        return Err(alternates(
            "the repository borrows objects from another store; brygge reads only the repository \
             it is given; run `git repack -a -d` in it (which copies borrowed objects in) and then \
             remove `objects/info/http-alternates`"
                .to_string(),
        ));
    }
    Ok(())
}

/// Open `path` in isolation: gix's `isolated()` options read **no** global/system/environment
/// *configuration* and use **no** credentials, so the decode depends only on the repository's own
/// objects (determinism, VF-1) and cannot be steered by ambient config (INV-2/T-2). brygge enables no
/// gix network feature (INV-3) and never checks out (so no filter/smudge runs); blob bytes are read raw
/// from the object database. Repository *shape* (alternates, `gitdir:` redirects, `commondir`) is
/// refused explicitly, before opening, per the module doc's CR-11 finding: `isolated()` does not cover it.
///
/// # Errors
/// [`Error::FloorRefusal`] on a refused repository shape (CR-11); [`Error::Open`] if the path is not a
/// readable Git repository.
pub fn open(path: &Path) -> Result<gix::Repository, Error> {
    check_repository_shape(path)?;
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
            feature: floor::SHALLOW_CLONE.to_string(),
            reason: "a shallow clone is a truncated history that would look whole; refused rather \
                     than imported as if complete (FA-1)"
                .to_string(),
        });
    }
    if git_dir.join("info").join("grafts").exists() {
        return Err(Error::FloorRefusal {
            feature: floor::GRAFTS.to_string(),
            reason:
                "grafts rewrite the ancestry a reader would see; refused rather than importing \
                     the rewritten view silently"
                    .to_string(),
        });
    }
    Ok(())
}

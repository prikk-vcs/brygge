//! Compute the **published view** (RFC 005 corrections handoff §1, owner ruling D-4 revised
//! 2026-09-23): the set of changesets `hg clone` would transfer. Secret/archived/internal changesets
//! (phase `>= 2`) and hidden (obsolete, unreachable) changesets are excluded; only served changesets
//! become atoms, and named-branch heads are computed over the served set alone.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use crate::revlog::{NULL_REV, Revlog};
use crate::util::parse_hex20;
use crate::{Error, floor, obsstore, phases};

/// The ceiling for a small, whole-file untrusted store read — `phaseroots`, `bookmarks`, `localtags`
/// (RFC 010 D-4 style; review 009 R-6). Documented in the crate README's Ceilings section.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) max_small_file_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_small_file_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Read `path` whole, bounded by `limits.max_small_file_bytes`, returning `None` when the file does not
/// exist. The bound is enforced *while reading* (`take(max + 1)`, refuse if the extra byte arrives), never
/// as a size check followed by an unbounded read (review 009 F-2; handoff 3 §6).
///
/// # Errors
/// [`Error::Open`] if the file exists but cannot be opened/read; [`Error::ResourceLimit`] if it exceeds
/// the ceiling.
pub(crate) fn read_bounded(
    path: &Path,
    what: &str,
    limits: &Limits,
) -> Result<Option<Vec<u8>>, Error> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Open(format!("cannot open {}: {e}", path.display()))),
    };
    let mut data = Vec::new();
    file.take(limits.max_small_file_bytes.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| Error::Open(format!("cannot read {}: {e}", path.display())))?;
    if data.len() as u64 > limits.max_small_file_bytes {
        return Err(Error::ResourceLimit {
            what: what.to_string(),
            ceiling: format!("{} bytes", limits.max_small_file_bytes),
        });
    }
    Ok(Some(data))
}

/// The computed published view: which revisions are served, and the two counts the loss boundary
/// records (each omitted when zero).
pub(crate) struct PublishedView {
    /// Indexed by revision number.
    pub(crate) served: Vec<bool>,
    /// Changesets with phase `>= 2` (secret, archived, internal) — RFC 005 corrections handoff §1.1.
    pub(crate) not_published: usize,
    /// Obsolete changesets not reachable from anything non-obsolete or pinned — §1.3.
    pub(crate) hidden: usize,
}

/// Compute the published view for `changelog`.
///
/// # Errors
/// [`Error::FloorRefusal`] if the repository has an unresolved merge in progress
/// (`.hg/merge/state2`); propagates any [`Error`] from reading `phaseroots`, `obsstore`, `bookmarks`,
/// `dirstate`, or `localtags`; [`Error::Read`] if the computed served set is not closed under ancestry
/// (a bug in this computation, or a store inconsistency — either way, refuse rather than import a
/// broken view).
pub(crate) fn compute(
    root: &Path,
    store: &Path,
    changelog: &Revlog,
) -> Result<PublishedView, Error> {
    refuse_unfinished_merge(root)?;

    let limits = Limits::default();
    let n = changelog.len();
    let phase = phases::compute(store, changelog, &limits)?;

    let mut node_to_rev: HashMap<[u8; 20], usize> = HashMap::with_capacity(n);
    for rev in 0..n {
        if let Some(e) = changelog.entry(rev) {
            node_to_rev.insert(e.node, rev);
        }
    }

    // Obsolete: a precursor present in the changelog and not public (phase != 0) — §1.2.
    let precursors = obsstore::precursor_nodes(store, &obsstore::Limits::default())?;
    let mut obsolete = vec![false; n];
    for node in &precursors {
        if let Some(&rev) = node_to_rev.get(node) {
            if phase.get(rev).copied().unwrap_or(0) != 0 {
                if let Some(slot) = obsolete.get_mut(rev) {
                    *slot = true;
                }
            }
        }
    }

    // Pinned (review 009 R-4): bookmarks, working-directory parents, and local tags only — `.hgtags`
    // does NOT pin (Mercurial's `repoview.pinnedrevs` never names it; the original handoff was wrong).
    let mut pinned = vec![false; n];
    mark_pinned_bookmarks(root, &node_to_rev, &limits, &mut pinned)?;
    mark_pinned_dirstate(root, &node_to_rev, &mut pinned)?;
    mark_pinned_localtags(root, &node_to_rev, &limits, &mut pinned)?;

    // visible = ancestors-or-self of every changeset that is non-obsolete or pinned (Mercurial's
    // `repoview.computehidden`). A single backward pass over descending rev order computes the
    // ancestor closure in O(n): parents always have a lower rev than their children.
    let mut visible = vec![false; n];
    for rev in 0..n {
        if !obsolete.get(rev).copied().unwrap_or(false) || pinned.get(rev).copied().unwrap_or(false)
        {
            if let Some(slot) = visible.get_mut(rev) {
                *slot = true;
            }
        }
    }
    for rev in (0..n).rev() {
        if !visible.get(rev).copied().unwrap_or(false) {
            continue;
        }
        let Some(entry) = changelog.entry(rev) else {
            continue;
        };
        for p in [entry.p1, entry.p2] {
            if p != NULL_REV {
                if let Ok(pi) = usize::try_from(p) {
                    if let Some(slot) = visible.get_mut(pi) {
                        *slot = true;
                    }
                }
            }
        }
    }

    let mut served = vec![false; n];
    let mut not_published = 0usize;
    let mut hidden = 0usize;
    for rev in 0..n {
        let is_unpublished = phase.get(rev).copied().unwrap_or(0) >= phases::NOT_PUBLISHED_FLOOR;
        let is_hidden = !is_unpublished
            && obsolete.get(rev).copied().unwrap_or(false)
            && !visible.get(rev).copied().unwrap_or(false);
        // The two records partition the exclusions (review 009 R-5): a secret-and-obsolete changeset
        // counts once, as not-published, never also as hidden.
        if is_hidden {
            hidden += 1;
        }
        if is_unpublished {
            not_published += 1;
        }
        if let Some(slot) = served.get_mut(rev) {
            *slot = !is_hidden && !is_unpublished;
        }
    }

    // A served changeset cannot have an unserved parent, by construction.
    for rev in 0..n {
        if !served.get(rev).copied().unwrap_or(false) {
            continue;
        }
        let Some(entry) = changelog.entry(rev) else {
            continue;
        };
        for p in [entry.p1, entry.p2] {
            if p != NULL_REV {
                if let Ok(pi) = usize::try_from(p) {
                    if !served.get(pi).copied().unwrap_or(false) {
                        return Err(Error::Read(
                            "published view is not closed under ancestry".to_string(),
                        ));
                    }
                }
            }
        }
    }

    Ok(PublishedView {
        served,
        not_published,
        hidden,
    })
}

/// A repository with `.hg/merge/state2` present has an unresolved merge in progress. Mercurial pins its
/// local and other nodes; brygge instead refuses (review 009 R-4) — simpler and more honest than
/// parsing mergestate for a transient, mid-operation state that is not a stable thing to migrate.
fn refuse_unfinished_merge(root: &Path) -> Result<(), Error> {
    let path = root.join(".hg").join("merge").join("state2");
    if path.exists() {
        return Err(Error::FloorRefusal {
            feature: floor::UNFINISHED_MERGE.to_string(),
            reason: "finish or abort the merge in the source first".to_string(),
        });
    }
    Ok(())
}

fn mark_pinned_bookmarks(
    root: &Path,
    node_to_rev: &HashMap<[u8; 20], usize>,
    limits: &Limits,
    pinned: &mut [bool],
) -> Result<(), Error> {
    let path = root.join(".hg").join("bookmarks");
    let Some(data) = read_bounded(&path, "the bookmarks file", limits)? else {
        return Ok(());
    };
    let body = std::str::from_utf8(&data)
        .map_err(|_| Error::Read(format!("{} is not valid UTF-8", path.display())))?;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (node_hex, _name) = line
            .split_once(' ')
            .ok_or_else(|| Error::Read(format!("malformed bookmarks line {line:?}")))?;
        let node = parse_hex20(node_hex)?;
        pin(node_to_rev, pinned, &node);
    }
    Ok(())
}

/// The working-directory parents: `.hg/dirstate`'s first 40 bytes are two 20-byte node ids (p1 then
/// p2), ignoring the null node. Only those 40 bytes are read (review 009 R-6) — the rest of dirstate
/// (per-file tracking state) is never needed here.
fn mark_pinned_dirstate(
    root: &Path,
    node_to_rev: &HashMap<[u8; 20], usize>,
    pinned: &mut [bool],
) -> Result<(), Error> {
    let path = root.join(".hg").join("dirstate");
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::Open(format!("cannot open {}: {e}", path.display()))),
    };
    let mut buf = Vec::new();
    file.take(40)
        .read_to_end(&mut buf)
        .map_err(|e| Error::Open(format!("cannot read {}: {e}", path.display())))?;
    // An empty file means "no parents"; anything between 1 and 39 bytes is a truncated header, and a
    // store that cannot say what it pins must not be read as if it pinned nothing (review 009 F-3).
    if !buf.is_empty() && buf.len() < 40 {
        return Err(Error::Read(format!(
            "{} is truncated ({} bytes; the parents header needs 40)",
            path.display(),
            buf.len()
        )));
    }
    for bytes in [buf.get(0..20), buf.get(20..40)].into_iter().flatten() {
        if let Ok(node) = <[u8; 20]>::try_from(bytes) {
            if node != [0u8; 20] {
                pin(node_to_rev, pinned, &node);
            }
        }
    }
    Ok(())
}

fn mark_pinned_localtags(
    root: &Path,
    node_to_rev: &HashMap<[u8; 20], usize>,
    limits: &Limits,
    pinned: &mut [bool],
) -> Result<(), Error> {
    let path = root.join(".hg").join("localtags");
    let Some(data) = read_bounded(&path, "the localtags file", limits)? else {
        return Ok(());
    };
    let body = std::str::from_utf8(&data)
        .map_err(|_| Error::Read(format!("{} is not valid UTF-8", path.display())))?;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (node_hex, _rest) = line
            .split_once(' ')
            .ok_or_else(|| Error::Read(format!("malformed localtags line {line:?}")))?;
        let node = parse_hex20(node_hex)?;
        pin(node_to_rev, pinned, &node);
    }
    Ok(())
}

fn pin(node_to_rev: &HashMap<[u8; 20], usize>, pinned: &mut [bool], node: &[u8; 20]) {
    if let Some(&rev) = node_to_rev.get(node) {
        if let Some(slot) = pinned.get_mut(rev) {
            *slot = true;
        }
    }
}

#[cfg(test)]
mod tests;

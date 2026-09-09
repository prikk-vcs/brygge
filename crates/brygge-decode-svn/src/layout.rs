//! The branch/tag layout convention (RFC 006 D-4/OQ-C), used only when `reconstruct_refs` is on.
//!
//! SVN has no first-class branches or tags — they are directory copies by *convention* (`/trunk`,
//! `/branches/x`, `/tags/y`). Interpreting a path as belonging to a branch/tag is therefore a **`Derived`**
//! judgment, never `Stated`; this module only *classifies* a path into the root it conventionally belongs
//! to, and the decode layer marks the resulting ref `Derived` with the convention recorded as a parameter
//! (`PR-5`).

use brygge_ir::RefKind;

use crate::options::LayoutPolicy;

/// A conventional branch/tag root a path belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// The ref name (e.g. `"trunk"`, a branch or tag directory name).
    pub name: String,
    /// Branch or tag.
    pub kind: RefKind,
    /// The repo-relative prefix that is this root (e.g. `"branches/foo"`), used as its identity.
    pub prefix: String,
}

/// Classify a repo-relative path into the conventional branch/tag root it belongs to, if any.
#[must_use]
pub fn classify(path: &str, layout: &LayoutPolicy) -> Option<Root> {
    if path == layout.trunk || starts_with_dir(path, &layout.trunk) {
        return Some(Root {
            name: "trunk".to_string(),
            kind: RefKind::Branch,
            prefix: layout.trunk.clone(),
        });
    }
    if let Some(name) = child_under(path, &layout.branches) {
        return Some(Root {
            prefix: format!("{}/{name}", layout.branches),
            name,
            kind: RefKind::Branch,
        });
    }
    if let Some(name) = child_under(path, &layout.tags) {
        return Some(Root {
            prefix: format!("{}/{name}", layout.tags),
            name,
            kind: RefKind::Tag,
        });
    }
    None
}

/// Whether `path` is inside the directory `dir` (i.e. `dir/...`).
fn starts_with_dir(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some()
}

/// If `path` is under container `dir` (`dir/<name>/...` or exactly `dir/<name>`), the first segment
/// `<name>`.
fn child_under(path: &str, dir: &str) -> Option<String> {
    let rest = path.strip_prefix(dir)?.strip_prefix('/')?;
    let name = rest.split('/').next().unwrap_or(rest);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests;

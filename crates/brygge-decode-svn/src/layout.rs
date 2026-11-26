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
///
/// RFC 020 D-3: under `--layout-trunk .` (the whole repository is the trunk), `layout.branches` and
/// `layout.tags` are both `None`, and every path — with no exception — classifies into the one trunk
/// root, prefix `""` (never `"."`; see [`LayoutPolicy::trunk_prefix`]).
#[must_use]
pub fn classify(path: &str, layout: &LayoutPolicy) -> Option<Root> {
    let trunk_prefix = layout.trunk_prefix();
    if trunk_prefix.is_empty() {
        return Some(Root {
            name: "trunk".to_string(),
            kind: RefKind::Branch,
            prefix: String::new(),
        });
    }
    if path == trunk_prefix || starts_with_dir(path, trunk_prefix) {
        return Some(Root {
            name: "trunk".to_string(),
            kind: RefKind::Branch,
            prefix: trunk_prefix.to_string(),
        });
    }
    if let Some(branches) = &layout.branches {
        if let Some(name) = child_under(path, branches) {
            return Some(Root {
                prefix: format!("{branches}/{name}"),
                name,
                kind: RefKind::Branch,
            });
        }
    }
    if let Some(tags) = &layout.tags {
        if let Some(name) = child_under(path, tags) {
            return Some(Root {
                prefix: format!("{tags}/{name}"),
                name,
                kind: RefKind::Tag,
            });
        }
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

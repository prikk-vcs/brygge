//! The running repository-tree model (RFC 006 D-1/§5b) — the SVN-specific core.
//!
//! SVN dumps a directory copy *implicitly* (it does not re-list the copied subtree), so the decoder keeps
//! a live `path → file` snapshot and evolves it revision by revision, expanding directory copies from the
//! historical snapshot they name. The path operations for a revision come from **diffing** the tree before
//! and after — which yields one op per path and so sidesteps the `replace`-as-two-same-path-ops ordering
//! trap the handoff warned of (RFC 006 D-3). Copies (`copyfrom`) are carried as `Stated` `RenameHint`s
//! beside the diff (SRC-S3).
//!
//! Only files are stored; a directory exists implicitly when files sit under it, and an empty directory
//! has no IR representation (dropped-with-record) — the IR has no empty-dir entity, like Git/prikk.

use std::collections::{BTreeMap, HashMap};

use brygge_ir::BlobId;
use brygge_ir::builder::IrBuilder;
use brygge_ir::model::{PathOp, RenameHint};
use brygge_ir::status::EpistemicStatus;

use crate::dumpstream::{NodeAction, NodeKind, NodeRecord};
use crate::{Error, props};

/// A file in the tree: its content address and IR mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Content address in the IR store.
    pub blob: BlobId,
    /// IR file mode (regular / executable / symlink).
    pub mode: u32,
}

/// The repository tree at a point: repo-relative file path → file. `BTreeMap` for deterministic order.
pub type Tree = BTreeMap<String, FileEntry>;

/// The outcome of applying one revision's node records.
pub struct Applied {
    /// The tree after the revision.
    pub tree: Tree,
    /// The literal path operations (diff of before → after), all `Stated`.
    pub ops: Vec<PathOp>,
    /// Source-recorded copies as `Stated` rename hints.
    pub hints: Vec<RenameHint>,
    /// Which loss categories the revision's node properties touched.
    pub prop_loss: props::PropLoss,
    /// How many empty directories (no files) the revision added.
    pub empty_dirs: usize,
}

/// Apply a revision's node records to the previous tree, producing the new tree, the diff ops, and the
/// stated copy hints.
///
/// # Errors
/// [`Error::FloorRefusal`] for a refused property; [`Error::Read`] for a copyfrom that names a missing
/// revision/path or a malformed change.
pub fn apply_revision(
    prev: &Tree,
    nodes: Vec<NodeRecord>,
    kept: &HashMap<u64, Tree>,
    builder: &mut IrBuilder,
) -> Result<Applied, Error> {
    let mut tree = prev.clone();
    let mut hints = Vec::new();
    let mut prop_loss = props::PropLoss::default();
    let mut empty_dirs = 0usize;

    for node in nodes {
        if let Some(p) = &node.props {
            props::check_externals(&node.path, p)?;
            let l = props::classify_loss(p);
            prop_loss.mergeinfo |= l.mergeinfo;
            prop_loss.workflow |= l.workflow;
            prop_loss.custom |= l.custom;
        }
        match node.action {
            NodeAction::Delete => remove_subtree(&mut tree, &node.path),
            NodeAction::Add | NodeAction::Change | NodeAction::Replace => {
                if node.action == NodeAction::Replace {
                    remove_subtree(&mut tree, &node.path);
                }
                match node.kind {
                    Some(NodeKind::Dir) => {
                        apply_dir(&mut tree, &mut hints, &mut empty_dirs, kept, node)?;
                    }
                    Some(NodeKind::File) => {
                        apply_file(&mut tree, &mut hints, kept, builder, node)?;
                    }
                    None => {
                        // A kindless change on an existing file is a property-only change; on a
                        // directory (a tracked-only-by-children path) there is nothing to do.
                        if tree.contains_key(&node.path) {
                            apply_file(&mut tree, &mut hints, kept, builder, node)?;
                        }
                    }
                }
            }
        }
    }

    let ops = diff(prev, &tree);
    Ok(Applied {
        tree,
        ops,
        hints,
        prop_loss,
        empty_dirs,
    })
}

/// Apply a file add/change/replace: compute its blob and mode, record a stated copy hint, set the path.
fn apply_file(
    tree: &mut Tree,
    hints: &mut Vec<RenameHint>,
    kept: &HashMap<u64, Tree>,
    builder: &mut IrBuilder,
    node: NodeRecord,
) -> Result<(), Error> {
    let np = node.props.as_deref().unwrap_or_default();
    let has_props = node.props.is_some();

    let source = match &node.copyfrom {
        Some((crev, cpath)) => Some(lookup_file(kept, *crev, cpath)?),
        None => None,
    };

    let blob = if let Some(text) = node.text {
        let content = if has_props && props::has(np, props::SVN_SPECIAL) {
            props::symlink_target(&text)
        } else {
            text
        };
        builder.add_blob(content)
    } else if let Some(src) = &source {
        src.blob
    } else if let Some(existing) = tree.get(&node.path) {
        existing.blob
    } else {
        return Err(Error::Read(format!(
            "file '{}' has no content and no copy source",
            node.path
        )));
    };

    let mode = if has_props {
        props::file_mode(np)
    } else if let Some(src) = &source {
        src.mode
    } else if let Some(existing) = tree.get(&node.path) {
        existing.mode
    } else {
        props::MODE_REGULAR
    };

    if let Some((_, cpath)) = &node.copyfrom {
        hints.push(RenameHint {
            from: cpath.clone(),
            to: node.path.clone(),
            status: EpistemicStatus::Stated,
        });
    }
    tree.insert(node.path, FileEntry { blob, mode });
    Ok(())
}

/// Apply a directory add/change/replace: expand a copy from its historical snapshot, or record an empty
/// directory. A directory property-only change touches no files.
fn apply_dir(
    tree: &mut Tree,
    hints: &mut Vec<RenameHint>,
    empty_dirs: &mut usize,
    kept: &HashMap<u64, Tree>,
    node: NodeRecord,
) -> Result<(), Error> {
    if let Some((crev, cpath)) = &node.copyfrom {
        let src = snapshot_for(kept, *crev)?;
        let prefix = format!("{cpath}/");
        let mut copied = 0usize;
        // Collect first (avoid borrowing `src` while mutating `tree`; they are different maps anyway).
        let mut additions: Vec<(String, String, FileEntry)> = Vec::new();
        for (spath, sentry) in src {
            let new_path = if spath == cpath {
                node.path.clone()
            } else if let Some(rest) = spath.strip_prefix(&prefix) {
                format!("{}/{}", node.path, rest)
            } else {
                continue;
            };
            additions.push((spath.clone(), new_path, sentry.clone()));
        }
        for (from, to, entry) in additions {
            hints.push(RenameHint {
                from,
                to: to.clone(),
                status: EpistemicStatus::Stated,
            });
            tree.insert(to, entry);
            copied = copied.saturating_add(1);
        }
        if copied == 0 {
            // A copy of an empty (or file-less) directory carries nothing into the file tree.
            *empty_dirs = empty_dirs.saturating_add(1);
        }
    } else if node.action == NodeAction::Add {
        // An added directory with no copy source and no files is an empty directory.
        *empty_dirs = empty_dirs.saturating_add(1);
    }
    // A directory `change` with no copyfrom is a property-only change (e.g. svn:mergeinfo): no file change.
    Ok(())
}

/// Diff `prev` → `new` into `Stated` path operations (one op per changed path).
fn diff(prev: &Tree, new: &Tree) -> Vec<PathOp> {
    let mut ops = Vec::new();
    for (path, e) in new {
        match prev.get(path) {
            None => ops.push(PathOp::Add {
                path: path.clone(),
                blob: e.blob,
                mode: e.mode,
                status: EpistemicStatus::Stated,
            }),
            Some(p) if p.blob != e.blob || p.mode != e.mode => ops.push(PathOp::Modify {
                path: path.clone(),
                blob: e.blob,
                mode: e.mode,
                status: EpistemicStatus::Stated,
            }),
            Some(_) => {}
        }
    }
    for path in prev.keys() {
        if !new.contains_key(path) {
            ops.push(PathOp::Delete {
                path: path.clone(),
                status: EpistemicStatus::Stated,
            });
        }
    }
    ops
}

/// Remove the exact path and its whole subtree (`path/...`).
fn remove_subtree(tree: &mut Tree, path: &str) {
    let prefix = format!("{path}/");
    tree.retain(|k, _| k != path && !k.starts_with(&prefix));
}

/// The retained snapshot for a revision. Only revisions a `copyfrom` names are retained (RFC 010
/// increment 1); a reference to a revision not present in this dump is a typed refusal.
fn snapshot_for(kept: &HashMap<u64, Tree>, rev: u64) -> Result<&Tree, Error> {
    kept.get(&rev).ok_or_else(|| {
        Error::Read(format!(
            "copyfrom references revision {rev}, which is not present in this dumpstream"
        ))
    })
}

fn lookup_file(kept: &HashMap<u64, Tree>, rev: u64, path: &str) -> Result<FileEntry, Error> {
    let t = snapshot_for(kept, rev)?;
    t.get(path).cloned().ok_or_else(|| {
        Error::Read(format!(
            "copyfrom source file '{path}' at revision {rev} not found"
        ))
    })
}

#[cfg(test)]
mod tests;

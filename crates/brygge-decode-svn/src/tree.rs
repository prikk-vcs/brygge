//! The running repository-tree model (RFC 006 D-1/§5b) — the SVN-specific core.
//!
//! SVN dumps a directory copy *implicitly* (it does not re-list the copied subtree), so the decoder keeps
//! a live `path → file` snapshot and evolves it revision by revision, expanding directory copies from the
//! historical snapshot they name. The path operations for a revision come from **diffing** the tree before
//! and after — which yields one op per path and so sidesteps the `replace`-as-two-same-path-ops ordering
//! trap the handoff warned of (RFC 006 D-3). Copies (`copyfrom`) are carried as `Stated` [`CopyRecord`]s
//! beside the diff (SRC-S3), with the **correct** `from_atom` (RFC 011 §5): each copy names the
//! already-assigned atom for its `copyfrom-rev`, via `revnum_to_atom`.
//!
//! Only files are stored; a directory exists implicitly when files sit under it, and an empty directory
//! has no IR representation (dropped-with-record) — the IR has no empty-dir entity, like Git/prikk.
//!
//! A path SVN states as **replaced** (`Node-action: replace`) gets `PathOp::Replace`, even when its
//! content is unchanged, because the source stated a new node at that path (CR-07.2, RFC 011 D-7) — never
//! folded into `Modify`, and never silently dropped when identical.

use std::collections::{BTreeMap, HashMap, HashSet};

use brygge_ir::BlobId;
use brygge_ir::builder::IrBuilder;
use brygge_ir::model::{AtomId, CopyRecord, PathOp};
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
    /// Source-recorded copies as `Stated` copy records, each naming the correct source atom.
    pub copies: Vec<CopyRecord>,
    /// Which loss categories the revision's node properties touched.
    pub prop_loss: props::PropLoss,
    /// How many directories added or replaced this revision (by `add`, `replace` or copy) ended up with
    /// no file under them after the whole revision was applied (CR-07.4; review 010 R-4 for `replace`).
    pub empty_dirs: usize,
}

/// Apply a revision's node records to the previous tree, producing the new tree, the diff ops, and the
/// stated copy records. `revnum_to_atom` maps every already-processed revision to its atom id, so a
/// `copyfrom-rev` resolves to the correct `from_atom` (RFC 011 §5) regardless of how far back it points.
///
/// # Errors
/// [`Error::FloorRefusal`] for a refused property; [`Error::Read`] for a copyfrom that names a missing
/// revision/path or a malformed change, or a symlink whose content is not `link <target>`.
pub fn apply_revision(
    prev: &Tree,
    nodes: Vec<NodeRecord>,
    kept: &HashMap<u64, Tree>,
    revnum_to_atom: &HashMap<u64, AtomId>,
    builder: &mut IrBuilder,
) -> Result<Applied, Error> {
    // Pass 1 (before any mutation): which paths this revision replaced (CR-07.2), and which directories
    // it added (CR-07.4) — both need the tree's *before* state, and the replaced set also needs the
    // *after* state, computed below once the revision has been fully applied.
    let mut replaced_paths: HashSet<String> = HashSet::new();
    let mut replaced_dirs: Vec<String> = Vec::new();
    let mut added_dirs: Vec<String> = Vec::new();
    for node in &nodes {
        match (node.kind, node.action) {
            (Some(NodeKind::File), NodeAction::Replace) => {
                replaced_paths.insert(node.path.clone());
            }
            (Some(NodeKind::Dir), NodeAction::Replace) => {
                collect_subtree_paths(prev, &node.path, &mut replaced_paths);
                replaced_dirs.push(node.path.clone());
                // Review 010 R-4: a replaced directory is a stated directory too — leaving it out of
                // empty-dir accounting would drop it with no record if it ends up empty.
                added_dirs.push(node.path.clone());
            }
            (Some(NodeKind::Dir), NodeAction::Add) => {
                added_dirs.push(node.path.clone());
            }
            _ => {}
        }
    }

    let mut tree = prev.clone();
    let mut copies = Vec::new();
    let mut prop_loss = props::PropLoss::default();

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
                        apply_dir(&mut tree, &mut copies, kept, revnum_to_atom, node)?;
                    }
                    Some(NodeKind::File) => {
                        apply_file(&mut tree, &mut copies, kept, revnum_to_atom, builder, node)?;
                    }
                    None => {
                        // A kindless change on an existing file is a property-only change; on a
                        // directory (a tracked-only-by-children path) there is nothing to do.
                        if tree.contains_key(&node.path) {
                            apply_file(
                                &mut tree,
                                &mut copies,
                                kept,
                                revnum_to_atom,
                                builder,
                                node,
                            )?;
                        }
                    }
                }
            }
        }
    }

    // Pass 2: the *after* half of each replaced directory's file set (CR-07.2), and whether each added
    // directory ended up with any file under it at all (CR-07.4) — both need the final tree.
    for node_path in &replaced_dirs {
        collect_subtree_paths(&tree, node_path, &mut replaced_paths);
    }
    let mut empty_dirs = 0usize;
    for dir in &added_dirs {
        if !is_live_under(&tree, dir) {
            empty_dirs = empty_dirs.saturating_add(1);
        }
    }

    let ops = diff(prev, &tree, &replaced_paths);
    Ok(Applied {
        tree,
        ops,
        copies,
        prop_loss,
        empty_dirs,
    })
}

/// Insert `dir` itself (if it is a file path — directories are not stored) and every file under it into
/// `out`.
fn collect_subtree_paths(tree: &Tree, dir: &str, out: &mut HashSet<String>) {
    if tree.contains_key(dir) {
        out.insert(dir.to_string());
    }
    let prefix = format!("{dir}/");
    for path in tree.keys() {
        if path.starts_with(&prefix) {
            out.insert(path.clone());
        }
    }
}

/// Apply a file add/change/replace: resolve its mode (own property block → copy source → existing entry,
/// CR-07.1), then its content — stripping the `link ` prefix whenever the *resolved* mode is a symlink,
/// regardless of whether this node carried its own property block (a retarget with no property block
/// inherits `svn:special` implicitly, and must still be unwrapped).
fn apply_file(
    tree: &mut Tree,
    copies: &mut Vec<CopyRecord>,
    kept: &HashMap<u64, Tree>,
    revnum_to_atom: &HashMap<u64, AtomId>,
    builder: &mut IrBuilder,
    node: NodeRecord,
) -> Result<(), Error> {
    let np = node.props.as_deref().unwrap_or_default();
    let has_props = node.props.is_some();

    let source = match &node.copyfrom {
        Some((crev, cpath)) => Some(lookup_file(kept, *crev, cpath)?),
        None => None,
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

    let blob = if let Some(text) = node.text {
        let content = if mode == props::MODE_SYMLINK {
            props::symlink_target(&text).ok_or_else(|| {
                Error::Read(format!(
                    "svn:special file without a link target: {}",
                    node.path
                ))
            })?
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

    if let Some((crev, cpath)) = &node.copyfrom {
        copies.push(CopyRecord {
            from: cpath.clone(),
            from_atom: atom_for(revnum_to_atom, *crev)?,
            to: node.path.clone(),
            status: EpistemicStatus::Stated,
        });
    }
    tree.insert(node.path, FileEntry { blob, mode });
    Ok(())
}

/// Apply a directory add/change/replace: expand a copy from its historical snapshot. A directory
/// property-only change, or a plain `add`/`replace` with no copy source, touches no files — CR-07.4's
/// empty-directory accounting happens once, after the whole revision, from the final tree.
fn apply_dir(
    tree: &mut Tree,
    copies: &mut Vec<CopyRecord>,
    kept: &HashMap<u64, Tree>,
    revnum_to_atom: &HashMap<u64, AtomId>,
    node: NodeRecord,
) -> Result<(), Error> {
    let Some((crev, cpath)) = &node.copyfrom else {
        return Ok(());
    };
    let src = snapshot_for(kept, *crev)?;
    let from_atom = atom_for(revnum_to_atom, *crev)?;
    let prefix = format!("{cpath}/");
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
        copies.push(CopyRecord {
            from,
            from_atom,
            to: to.clone(),
            status: EpistemicStatus::Stated,
        });
        tree.insert(to, entry);
    }
    Ok(())
}

/// Resolve `rev`'s already-assigned atom id (RFC 011 §5). A `copyfrom-rev` always names an earlier,
/// already-processed revision, so every lookup here should succeed; a miss means a malformed dump
/// (a forward reference) rather than a decoder bug.
fn atom_for(revnum_to_atom: &HashMap<u64, AtomId>, rev: u64) -> Result<AtomId, Error> {
    revnum_to_atom.get(&rev).copied().ok_or_else(|| {
        Error::Read(format!(
            "copyfrom references revision {rev}, whose atom is not yet known (a forward reference)"
        ))
    })
}

/// Diff `prev` → `new` into `Stated` path operations (one op per changed path). A path named in
/// `replaced` gets `PathOp::Replace` whenever it is present on both sides, even with identical content
/// (CR-07.2, RFC 011 D-7) — the source stated a new node, not a modification.
fn diff(prev: &Tree, new: &Tree, replaced: &HashSet<String>) -> Vec<PathOp> {
    let mut ops = Vec::new();
    for (path, e) in new {
        match prev.get(path) {
            None => ops.push(PathOp::Add {
                path: path.clone(),
                blob: e.blob,
                mode: e.mode,
                status: EpistemicStatus::Stated,
            }),
            Some(_) if replaced.contains(path) => ops.push(PathOp::Replace {
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

/// Whether at least one file in `tree` lives at or under `prefix` (used by the decode layer for CR-07.3's
/// live-ref check and CR-07.5's layout-honesty count).
#[must_use]
pub fn is_live_under(tree: &Tree, prefix: &str) -> bool {
    let dir_prefix = format!("{prefix}/");
    tree.keys()
        .any(|k| k == prefix || k.starts_with(&dir_prefix))
}

#[cfg(test)]
mod tests;

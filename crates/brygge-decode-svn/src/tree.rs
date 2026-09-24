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

use crate::dumpstream::{
    Checksums, NodeAction, NodeKind, NodeRecord, PropBlock, PropChange, TextBody,
};
use crate::source::Limits;
use crate::{Error, checksum, props, svndiff};

/// A file in the tree: its content address, and its two mode properties.
///
/// The two mode properties are kept **separately**, not only as the derived `mode`: a file can carry both
/// `svn:executable` and `svn:special`, and a property delta that removes `svn:special` must leave it
/// executable. `mode` is the derived IR mode ([`props::mode_of`]) and is what the diff compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Content address in the IR store. For an `svn:special` file this is the bare link target, not SVN's
    /// text (`link <target>`); [`svn_text`] gives SVN's text.
    pub blob: BlobId,
    /// IR file mode (regular / executable / symlink), derived from the two flags.
    pub mode: u32,
    /// `svn:executable` is set.
    pub exec: bool,
    /// `svn:special` is set (the file is a symlink).
    pub special: bool,
}

/// The resource accounting for one decode: the ceilings, and how much the deltas have reconstructed so far
/// (RFC 013 D-2, T-8). A delta dump never expands past what brygge would accept as a fulltext dump.
pub struct Budget<'a> {
    /// The ceilings.
    pub limits: &'a Limits,
    /// The bytes every delta's reconstructed target has taken so far.
    pub reconstructed: usize,
}

impl<'a> Budget<'a> {
    /// A budget with nothing spent.
    #[must_use]
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            reconstructed: 0,
        }
    }
}

/// SVN's text for an entry: what the dump's checksums and deltas are about. The IR blob of an `svn:special`
/// file is the bare target, and SVN's text is `link ` followed by it (`props::symlink_target` strips exactly
/// that prefix), so this is exactly reversible. **The one place** an entry's SVN text is made: every delta
/// base and every checksum uses it.
///
/// # Errors
/// [`Error::Read`] if the blob is not in the builder (a decoder bug, not bad input).
pub fn svn_text(entry: &FileEntry, builder: &IrBuilder) -> Result<Vec<u8>, Error> {
    let blob = builder
        .blob(&entry.blob)
        .ok_or_else(|| Error::Read("a tracked file's content is missing (internal)".to_string()))?;
    if entry.special {
        let mut text = Vec::with_capacity(blob.len().saturating_add(5));
        text.extend_from_slice(b"link ");
        text.extend_from_slice(blob);
        Ok(text)
    } else {
        Ok(blob.to_vec())
    }
}

/// The IR blob for SVN text under a flag: an `svn:special` file's blob is the text without `link `, and a text
/// without it is refused, never guessed (CR-07.1).
fn blob_for(
    svn_text: Vec<u8>,
    special: bool,
    path: &str,
    builder: &mut IrBuilder,
) -> Result<BlobId, Error> {
    let content = if special {
        props::symlink_target(&svn_text)
            .ok_or_else(|| Error::Read(format!("svn:special file without a link target: {path}")))?
    } else {
        svn_text
    };
    Ok(builder.add_blob(content))
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
    budget: &mut Budget<'_>,
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
        if let Some(block) = &node.props {
            // The entries the block sets (the whole set for a full block): what is refused and classified.
            let set: props::Owned = match block {
                PropBlock::Full(p) => p.clone(),
                PropBlock::Delta(changes) => changes
                    .iter()
                    .filter_map(|c| match c {
                        PropChange::Set(k, v) => Some((k.clone(), v.clone())),
                        PropChange::Delete(_) => None,
                    })
                    .collect(),
            };
            props::check_externals(&node.path, &set)?;
            let l = props::classify_loss(&set);
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
                        apply_file(
                            &mut tree,
                            &mut copies,
                            kept,
                            revnum_to_atom,
                            builder,
                            budget,
                            node,
                        )?;
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
                                budget,
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

/// Verify one stated checksum against the text it names. A consistency check, not authenticity (the dump is
/// untrusted and states any checksum it likes); it is also the end-to-end check of our own delta application.
fn verify_sum(
    path: &str,
    header: &str,
    stated: Option<&str>,
    text: &[u8],
    hash: fn(&[u8]) -> Result<String, Error>,
) -> Result<(), Error> {
    let Some(want) = stated else {
        return Ok(());
    };
    if !hash(text)?.eq_ignore_ascii_case(want.trim()) {
        return Err(Error::Read(format!(
            "{path}: {header} mismatch; the dump or its base is not what it states"
        )));
    }
    Ok(())
}

/// Apply a file add/change/replace.
///
/// The **base** of a text or property delta (RFC 013 D-2): the copy source at its revision for a node with
/// `Node-copyfrom-*`, the path's current entry for a `change`, and empty text with no properties for an `add`
/// or `replace` without a copy source. The two mode flags come from the node's own property block (a full
/// block: its set; a delta: the base's flags changed by its entries), else the copy source's, else the
/// existing entry's (CR-07.1). The content is SVN's text under the resolved flag: the fulltext, or the
/// reconstruction of a delta against the base's SVN text, then checked against every checksum the node
/// states; a node with no text keeps its base's content, **recomputed** when a property change flips
/// `svn:special` (`RR-svn-special-toggle`): becoming special takes the link target out of `link <target>`,
/// ceasing to be special makes the target the whole text.
fn apply_file(
    tree: &mut Tree,
    copies: &mut Vec<CopyRecord>,
    kept: &HashMap<u64, Tree>,
    revnum_to_atom: &HashMap<u64, AtomId>,
    builder: &mut IrBuilder,
    budget: &mut Budget<'_>,
    node: NodeRecord,
) -> Result<(), Error> {
    let source = match &node.copyfrom {
        Some((crev, cpath)) => Some(lookup_file(kept, *crev, cpath)?),
        None => None,
    };
    let existing = tree.get(&node.path).cloned();
    // What content and flags a node with no text and no block of its own inherits (CR-07.1).
    let inherited = source.as_ref().or(existing.as_ref());
    // The base a delta applies to.
    let delta_base = match (&source, node.action) {
        (Some(src), _) => Some(src),
        (None, NodeAction::Change) => existing.as_ref(),
        (None, _) => None,
    };

    let base_flags = delta_base.map_or((false, false), |b| (b.exec, b.special));
    let (exec, special) = match &node.props {
        Some(PropBlock::Full(np)) => (
            props::has(np, props::SVN_EXECUTABLE),
            props::has(np, props::SVN_SPECIAL),
        ),
        Some(PropBlock::Delta(changes)) => {
            let (mut exec, mut special) = base_flags;
            for change in changes {
                match change {
                    PropChange::Set(k, _) if k == props::SVN_EXECUTABLE => exec = true,
                    PropChange::Set(k, _) if k == props::SVN_SPECIAL => special = true,
                    PropChange::Delete(k) if k == props::SVN_EXECUTABLE => exec = false,
                    PropChange::Delete(k) if k == props::SVN_SPECIAL => special = false,
                    _ => {}
                }
            }
            (exec, special)
        }
        None => inherited.map_or((false, false), |e| (e.exec, e.special)),
    };
    let mode = props::mode_of(exec, special);

    // The node's SVN text, if it carries any; each stated checksum checked on the way.
    let svn_text_of_node: Option<Vec<u8>> = match node.text {
        Some(TextBody::Full(text)) => Some(text),
        Some(TextBody::Delta(delta)) => {
            let base_text = match delta_base {
                Some(b) => svn_text(b, builder)?,
                None if node.action == NodeAction::Change => {
                    return Err(Error::Read(format!(
                        "{}: delta against a base not present in this dump; an incremental or partial \
                         delta dump cannot be decoded alone",
                        node.path
                    )));
                }
                None => Vec::new(),
            };
            verify_base(&node.path, &node.checksums, &base_text)?;
            let remaining = budget
                .limits
                .max_dump_bytes
                .saturating_sub(budget.reconstructed);
            let allowed = budget.limits.max_node_text_bytes.min(remaining);
            let target = svndiff::apply(&delta, &base_text, allowed).map_err(|e| match e {
                svndiff::DiffError::UnsupportedVersion(v) => Error::UnsupportedFormat {
                    what: format!("svndiff version {v}"),
                    reason: format!("{}: this build reads svndiff version 0", node.path),
                },
                svndiff::DiffError::Malformed(m) => {
                    Error::Read(format!("{}: text delta: {m}", node.path))
                }
                svndiff::DiffError::TooLarge { .. } => {
                    if budget.limits.max_node_text_bytes <= remaining {
                        Error::ResourceLimit {
                            what: format!("the text of '{}'", node.path),
                            ceiling: format!("{} bytes", budget.limits.max_node_text_bytes),
                        }
                    } else {
                        Error::ResourceLimit {
                            what: "the text every delta reconstructs, in total".to_string(),
                            ceiling: format!("{} bytes", budget.limits.max_dump_bytes),
                        }
                    }
                }
            })?;
            budget.reconstructed = budget.reconstructed.saturating_add(target.len());
            Some(target)
        }
        None => None,
    };
    if let Some(text) = &svn_text_of_node {
        verify_content(&node.path, &node.checksums, text)?;
    }
    if let (Some(src), Some(_)) = (&source, &svn_text_of_node) {
        // A copy with text states its source's checksum too (`svnadmin`; `svnrdump` writes none).
        let src_text = svn_text(src, builder)?;
        verify_sum(
            &node.path,
            "Text-copy-source-md5",
            node.checksums.copy_md5.as_deref(),
            &src_text,
            checksum::md5_hex,
        )?;
        verify_sum(
            &node.path,
            "Text-copy-source-sha1",
            node.checksums.copy_sha1.as_deref(),
            &src_text,
            checksum::sha1_hex,
        )?;
    }

    let blob = if let Some(text) = svn_text_of_node {
        blob_for(text, special, &node.path, builder)?
    } else if let Some(base) = inherited {
        if base.special == special {
            base.blob
        } else {
            // A property change flipped `svn:special` on a node with no text: SVN's text is unchanged and
            // the IR blob follows the new flag.
            let text = svn_text(base, builder)?;
            blob_for(text, special, &node.path, builder)?
        }
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
    tree.insert(
        node.path,
        FileEntry {
            blob,
            mode,
            exec,
            special,
        },
    );
    Ok(())
}

/// `Text-delta-base-md5` / `-sha1` against the base's SVN text.
fn verify_base(path: &str, sums: &Checksums, base: &[u8]) -> Result<(), Error> {
    verify_sum(
        path,
        "Text-delta-base-md5",
        sums.base_md5.as_deref(),
        base,
        checksum::md5_hex,
    )?;
    verify_sum(
        path,
        "Text-delta-base-sha1",
        sums.base_sha1.as_deref(),
        base,
        checksum::sha1_hex,
    )
}

/// `Text-content-md5` / `-sha1` against the resulting SVN text (fulltext nodes included).
fn verify_content(path: &str, sums: &Checksums, text: &[u8]) -> Result<(), Error> {
    verify_sum(
        path,
        "Text-content-md5",
        sums.content_md5.as_deref(),
        text,
        checksum::md5_hex,
    )?;
    verify_sum(
        path,
        "Text-content-sha1",
        sums.content_sha1.as_deref(),
        text,
        checksum::sha1_hex,
    )
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

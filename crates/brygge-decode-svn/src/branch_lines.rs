//! RFC 019 Amendment A, D-9: `--branch-lines` — one line of history per branch/tag root, instead of the
//! single whole-repository spine [`crate::decode`] produces. Each root's own atoms carry only that
//! root's files at the root of its tree, so the result reads as a real per-branch history once
//! `encode git` writes it, rather than the whole repository's tree repeated in every commit.
//!
//! **The contract is unchanged** (the architect's refinement at handoff): a line atom is an ordinary
//! `Derived(ReconstructedBranch)` atom, with `params = {layout, root, line: "true"}` — no new
//! `DerivationKind` variant, because an enum variant an older reader does not know fails its read
//! outright (the critical-bit rule covers fields, not variants).
//!
//! The whole-tree bookkeeping (checksums, svndiff reconstruction, blob dedup, symlink/mode handling) is
//! entirely [`crate::tree`]'s, reused unchanged via [`tree::apply_revision`] once per revision. This
//! module's own job is to **partition** that whole tree by root and **re-derive** each root's own parent
//! link, starting state and ops from it.

use std::collections::{BTreeMap, HashMap, HashSet};

use brygge_ir::Storage;
use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, CopyRecord, DropRecord, Flag, FlagKind, Identity, ImportProvenance, Ir, LossBoundary,
    LossClass, MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity, SourceKind, Text,
};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};

use crate::layout::{self, Root};
use crate::source::{Limits, svnadmin_version};
use crate::tree::{self, Budget, FileEntry, Tree};
use crate::{DECODER, Error, Options, Source, decoder_version, dumpstream, floor, props};

/// A root's own relative tree: repo-relative paths under its prefix, with the prefix stripped.
type RelTree = BTreeMap<String, FileEntry>;

/// Decode with `--branch-lines` (a decoder change with no new process or platform code).
///
/// # Errors
/// As [`crate::decode::decode`].
pub(crate) fn decode_with(
    source: &Source,
    opts: &Options,
    limits: &Limits,
    storage: &Storage,
) -> Result<Ir, Error> {
    let bytes = source.load_with(limits)?;
    let dump = dumpstream::parse_dump_with(&bytes, limits)?;
    drop(bytes);
    let mut budget = Budget::new(limits);

    let repo_id = dump
        .uuid
        .as_ref()
        .map(|u| u.as_bytes().to_vec())
        .unwrap_or_default();

    let provenance = ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Svn,
            repo_id: repo_id.clone(),
            atom_id: repo_id.clone(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        brygge_version: decoder_version().to_string(),
        decoder: DECODER.to_string(),
        decoder_version: decoder_version().to_string(),
        params: {
            let mut params = opts.as_params();
            params.insert("floor".to_string(), floor::joined());
            match source {
                Source::DumpFile(_) => {
                    params.insert("source_form".to_string(), "dumpfile".to_string());
                }
                Source::LocalRepo(_) => {
                    params.insert("source_form".to_string(), "svnadmin-dump".to_string());
                    params.insert("svnadmin_version".to_string(), svnadmin_version()?);
                }
            }
            params
        },
    };
    let mut builder = IrBuilder::with_storage(provenance, storage).map_err(Error::Ir)?;
    let mut loss = Loss::default();

    if dump.revisions.is_empty() {
        builder.set_loss(loss.into_boundary());
        return builder.finish().map_err(Error::Ir);
    }

    // Snapshot retention (RFC 010 increment 1, unchanged): a copyfrom — including the one that creates a
    // line by copying a whole root — always names an earlier revision, collected here exactly as the
    // single-spine path does.
    let mut needed: HashSet<u64> = HashSet::new();
    for rev in &dump.revisions {
        for node in &rev.nodes {
            if let Some((crev, _)) = &node.copyfrom {
                needed.insert(*crev);
            }
        }
    }

    let mut kept: HashMap<u64, Tree> = HashMap::new();
    // `tree::apply_revision` still wants a flat revision -> atom map, so its own copy-tracking can
    // resolve a `from_atom` without erroring on an unknown revision. Branch-lines mode has no single
    // atom per revision to give it, so every processed revision gets this placeholder instead — its
    // value is never read: every copy it would have resolved is re-resolved below, per root line,
    // using `history` and `Applied::copy_source_revs`.
    let placeholder_atom = AtomId([0u8; 32]);
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    let mut prev_tree = Tree::new();

    let mut live: BTreeMap<String, Line> = BTreeMap::new();
    // Every root's (revision -> atom) history, across every life it has had — the lookup D-9 calls "the
    // newest atom on <src>'s line at or before <rev>".
    let mut history: HashMap<String, BTreeMap<u64, AtomId>> = HashMap::new();
    let mut dead_lives: Vec<DeadLife> = Vec::new();
    let mut outside_drops: Vec<DropRecord> = Vec::new();
    let mut revisions_with_no_branch_change: u64 = 0u64;
    // D-9 §2 / review 147 F-3: a root's first atom, created by copying from a path outside every
    // recognized root. The behaviour (a fresh line, no parent) is unchanged; this only counts it, into
    // the existing convention-violation flag's own detail (`layout_honesty`).
    let mut roots_created_from_outside_the_layout: u64 = 0;

    for rev in dump.revisions {
        let metadata = metadata_from_props(&rev.props, &mut loss.unparseable_dates);
        let number = rev.number;

        // Which roots this revision names at all, in canonical order (trunk, then branches, then tags,
        // each by bytes) — D-9's "one per (revision, root it touches)".
        let mut touched: Vec<Root> = rev
            .nodes
            .iter()
            .filter_map(|n| layout::classify(&n.path, &opts.layout))
            .collect();
        dedup_roots(&mut touched);
        sort_roots_canonically(&mut touched, &opts.layout.trunk);
        if touched.is_empty() {
            revisions_with_no_branch_change = revisions_with_no_branch_change.saturating_add(1);
        }

        // The node whose own path creates a root (the root-level `svn cp <src>@<rev> <root>`), if any —
        // keyed by the root's prefix, looked up below only for a *new* line.
        let mut root_creating_copy: HashMap<String, (u64, String)> = HashMap::new();
        for node in &rev.nodes {
            if let Some(root) = layout::classify(&node.path, &opts.layout) {
                if root.prefix == node.path {
                    if let Some((crev, cpath)) = &node.copyfrom {
                        root_creating_copy.insert(root.prefix.clone(), (*crev, cpath.clone()));
                    }
                }
            }
        }

        let applied = tree::apply_revision(
            &prev_tree,
            rev.nodes,
            &kept,
            &revnum_to_atom,
            &mut builder,
            &mut budget,
        )?;

        loss.mergeinfo |= applied.prop_loss.mergeinfo;
        loss.workflow |= applied.prop_loss.workflow;
        loss.custom |= applied.prop_loss.custom;
        loss.empty_dirs = loss.empty_dirs.saturating_add(applied.empty_dirs);

        // Partition the new whole-tree state by root, one pass.
        let mut by_root: HashMap<String, (Root, RelTree)> = HashMap::new();
        for (path, entry) in &applied.tree {
            if let Some(root) = layout::classify(path, &opts.layout) {
                let rel = path
                    .strip_prefix(&root.prefix)
                    .and_then(|r| r.strip_prefix('/'))
                    .unwrap_or("")
                    .to_string();
                by_root
                    .entry(root.prefix.clone())
                    .or_insert_with(|| (root, RelTree::new()))
                    .1
                    .insert(rel, entry.clone());
            }
        }

        // Outside every root: each changed path there is its own drop (D-9 §2), on top of the existing
        // aggregate convention-violation flag computed once at the end.
        for op in &applied.ops {
            let path = op_path(op);
            if layout::classify(path, &opts.layout).is_none() {
                outside_drops.push(DropRecord {
                    class: LossClass::Other,
                    what: format!("r{number} {path}"),
                    reason: "outside the trunk/branches/tags layout; on no branch-lines line"
                        .to_string(),
                    step: None,
                });
            }
        }

        // A root previously live but absent from the new partition just died — whether an explicit node
        // deleted it directly, or a containing directory was removed with no node of its own.
        let now_dead: Vec<String> = live
            .keys()
            .filter(|prefix| !by_root.contains_key(prefix.as_str()))
            .cloned()
            .collect();
        for prefix in now_dead {
            if let Some(line) = live.remove(&prefix) {
                dead_lives.push(DeadLife {
                    root: line.root,
                    last_atom: line.last_atom,
                    last_rev: line.last_rev,
                });
            }
        }

        for root in touched {
            let Some((_, new_rel_tree)) = by_root.get(&root.prefix) else {
                // Touched but ended up with nothing under it this revision: just died, handled above.
                continue;
            };
            let new_rel_tree = new_rel_tree.clone();

            let continuing = live.contains_key(&root.prefix);
            let (parent, starting_tree) = if let Some(line) = live.get(&root.prefix) {
                (Some(line.last_atom), line.tree.clone())
            } else if let Some((crev, cpath)) = root_creating_copy.get(&root.prefix) {
                match layout::classify(cpath, &opts.layout) {
                    Some(enclosing) => {
                        let parent = history
                            .get(&enclosing.prefix)
                            .and_then(|h| h.range(..=*crev).next_back())
                            .map(|(_, atom)| *atom);
                        let start = kept
                            .get(crev)
                            .map(|snap| relative_subtree(snap, &enclosing.prefix))
                            .unwrap_or_default();
                        (parent, start)
                    }
                    // The copy source is not under any recognized root: no line to inherit from,
                    // so a fresh line starts with no parent, and its ops (below) become exactly the
                    // copied content (diffed against the empty starting tree) — review 147 F-3.
                    None => {
                        roots_created_from_outside_the_layout += 1;
                        (None, RelTree::new())
                    }
                }
            } else {
                (None, RelTree::new())
            };

            // A continuing line's ops come from the already-correct whole-tree diff, filtered and
            // relativized — preserving `Replace` (a node SVN stated as replaced, even with identical
            // content, CR-07.2) which a synthetic before/after diff of two tree snapshots cannot tell
            // apart from a no-op. A line's first atom has no such global diff to filter (its starting
            // state is the copy source's tree, not `prev_tree`), so it still uses the synthetic diff.
            let ops = if continuing {
                filter_ops_for_root(&applied.ops, &root)
            } else {
                diff_relative(&starting_tree, &new_rel_tree)
            };

            // Copies carried into this atom: only for a *continuing* line (a root just created by a
            // whole-directory copy carries nothing explicit — its content is inherited through the
            // parent pointer and the ops diff above already accounts for anything on top of that).
            let copies = if parent.is_some() && !root_creating_copy.contains_key(&root.prefix) {
                resolve_copies(
                    &applied.copies,
                    &applied.copy_source_revs,
                    number,
                    &root,
                    &opts.layout,
                    &history,
                )?
            } else {
                Vec::new()
            };

            let mut params = BTreeMap::new();
            params.insert("layout".to_string(), opts.layout.label());
            params.insert("root".to_string(), root.prefix.clone());
            params.insert("line".to_string(), "true".to_string());
            let mut source_atom_id = number.to_be_bytes().to_vec();
            source_atom_id.extend_from_slice(root.prefix.as_bytes());

            let atom_id = builder.add_atom(AtomDraft {
                parents: parent.into_iter().collect(),
                ops,
                copies,
                metadata: metadata.clone(),
                source: SourceIdentity {
                    kind: SourceKind::Svn,
                    repo_id: repo_id.clone(),
                    atom_id: source_atom_id,
                    signatures: Vec::new(),
                    extras: Vec::new(),
                },
                status: EpistemicStatus::Derived(Derivation {
                    kind: DerivationKind::ReconstructedBranch,
                    by: DECODER.to_string(),
                    decoder_version: decoder_version().to_string(),
                    params,
                    confidence: None,
                }),
            })?;

            history
                .entry(root.prefix.clone())
                .or_default()
                .insert(number, atom_id);
            live.insert(
                root.prefix.clone(),
                Line {
                    root,
                    tree: new_rel_tree,
                    last_atom: atom_id,
                    last_rev: number,
                },
            );
        }

        if needed.contains(&number) {
            kept.insert(number, applied.tree.clone());
        }
        prev_tree = applied.tree;
        revnum_to_atom.insert(number, placeholder_atom);
    }

    if live.is_empty() && dead_lives.is_empty() {
        // RFC 019 Amendment A §1: every revision touched no root, so this decode would yield no atoms.
        // Turning a whole history into nothing is never a success — refuse instead of returning an
        // empty artifact with only a flag to say why.
        return Err(Error::BranchLinesNoLayout);
    } else {
        for (_, line) in live {
            let mut params = BTreeMap::new();
            params.insert("layout".to_string(), opts.layout.label());
            if line.root.kind == RefKind::Tag {
                params.insert(
                    "immutability".to_string(),
                    "not-guaranteed: an SVN tag is an ordinary directory and may carry \
                     post-creation commits"
                        .to_string(),
                );
            }
            builder.add_ref(RefRecord {
                name: line.root.name.clone(),
                kind: line.root.kind.clone(),
                target: line.last_atom,
                status: EpistemicStatus::Derived(Derivation {
                    kind: DerivationKind::ReconstructedBranch,
                    by: DECODER.to_string(),
                    decoder_version: decoder_version().to_string(),
                    params,
                    confidence: None,
                }),
                source: None,
                annotation: None,
            })?;
        }
        for dead in dead_lives {
            let mut params = BTreeMap::new();
            params.insert("layout".to_string(), opts.layout.label());
            params.insert("root".to_string(), dead.root.prefix.clone());
            params.insert("state".to_string(), "deleted".to_string());
            let other_label = match dead.root.kind {
                RefKind::Tag => "deleted-tag",
                _ => "deleted-branch",
            };
            builder.add_ref(RefRecord {
                name: format!("{}@r{}", dead.root.prefix, dead.last_rev),
                kind: RefKind::Other(other_label.to_string()),
                target: dead.last_atom,
                status: EpistemicStatus::Derived(Derivation {
                    kind: DerivationKind::ReconstructedBranch,
                    by: DECODER.to_string(),
                    decoder_version: decoder_version().to_string(),
                    params,
                    confidence: None,
                }),
                source: None,
                annotation: None,
            })?;
        }
        layout_honesty(
            &prev_tree,
            opts,
            roots_created_from_outside_the_layout,
            &mut builder,
        );
    }

    loss.outside_root_ops = outside_drops;
    loss.revisions_with_no_branch_change = revisions_with_no_branch_change;
    builder.set_loss(loss.into_boundary());
    builder.finish().map_err(Error::Ir)
}

/// The currently-live state of one root's line.
struct Line {
    root: Root,
    tree: RelTree,
    last_atom: AtomId,
    last_rev: u64,
}

/// A root whose line has ended (deleted, or moved to a prefix a later life does not reuse).
struct DeadLife {
    root: Root,
    last_atom: AtomId,
    last_rev: u64,
}

/// `snapshot`'s content under `root_prefix`, keyed with the prefix removed — the "starting tree" D-9
/// gives a new line created by copy, whether the copy named the root itself or a subdirectory of it (in
/// which case the ops diff against this full tree is what "makes up the difference", D-9 §2).
fn relative_subtree(snapshot: &Tree, root_prefix: &str) -> RelTree {
    let dir_prefix = format!("{root_prefix}/");
    let mut out = RelTree::new();
    for (path, entry) in snapshot {
        if path == root_prefix {
            out.insert(String::new(), entry.clone());
        } else if let Some(rel) = path.strip_prefix(&dir_prefix) {
            out.insert(rel.to_string(), entry.clone());
        }
    }
    out
}

/// A plain before/after diff of two relative trees: `Add` for a new key, `Modify` for a changed one,
/// `Delete` for one no longer present. Unlike [`tree::apply_revision`]'s own diff, this never produces
/// `Replace`: that distinction is about a literal SVN `Node-action: replace` on one path, which has no
/// meaning once two different trees (a copy's source, and a new root's result) are being compared
/// synthetically. A continuing line's ops come from the real diff instead (via [`resolve_copies`]'s
/// sibling, the direct `applied.ops` filter in the caller), so this only runs for a line's first atom.
fn diff_relative(before: &RelTree, after: &RelTree) -> Vec<PathOp> {
    let mut ops = Vec::new();
    for (path, entry) in after {
        match before.get(path) {
            None => ops.push(PathOp::Add {
                path: path.clone(),
                blob: entry.blob,
                mode: entry.mode,
                status: EpistemicStatus::Stated,
            }),
            Some(b) if b.blob != entry.blob || b.mode != entry.mode => ops.push(PathOp::Modify {
                path: path.clone(),
                blob: entry.blob,
                mode: entry.mode,
                status: EpistemicStatus::Stated,
            }),
            Some(_) => {}
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            ops.push(PathOp::Delete {
                path: path.clone(),
                status: EpistemicStatus::Stated,
            });
        }
    }
    ops
}

/// `applied.ops`'s entries under `root`, relativized, keeping each op's original variant (so `Replace`,
/// CR-07.2, survives) — used for a continuing line, whose starting state really is `prev_tree`'s content
/// under `root`, so the existing whole-tree diff already says exactly what changed.
fn filter_ops_for_root(ops: &[PathOp], root: &Root) -> Vec<PathOp> {
    let dir_prefix = format!("{}/", root.prefix);
    ops.iter()
        .filter_map(|op| {
            let path = op_path(op);
            let rel = path
                .strip_prefix(&dir_prefix)
                .or_else(|| (path == root.prefix).then_some(""))?;
            Some(with_path(op, rel.to_string()))
        })
        .collect()
}

/// `op`, with its path replaced by `path`, preserving every other field.
fn with_path(op: &PathOp, path: String) -> PathOp {
    match op {
        PathOp::Add {
            blob, mode, status, ..
        } => PathOp::Add {
            path,
            blob: *blob,
            mode: *mode,
            status: status.clone(),
        },
        PathOp::Modify {
            blob, mode, status, ..
        } => PathOp::Modify {
            path,
            blob: *blob,
            mode: *mode,
            status: status.clone(),
        },
        PathOp::Replace {
            blob, mode, status, ..
        } => PathOp::Replace {
            path,
            blob: *blob,
            mode: *mode,
            status: status.clone(),
        },
        PathOp::Delete { status, .. } => PathOp::Delete {
            path,
            status: status.clone(),
        },
    }
}

/// The path a [`PathOp`] names, whichever variant it is.
fn op_path(op: &PathOp) -> &str {
    match op {
        PathOp::Add { path, .. }
        | PathOp::Modify { path, .. }
        | PathOp::Replace { path, .. }
        | PathOp::Delete { path, .. } => path,
    }
}

/// Re-resolve `applied.copies` for one continuing root's atom: each copy's `from_atom` was computed
/// against the single-spine `revnum_to_atom`, which branch-lines mode leaves empty, so it is recomputed
/// here from `from`'s own enclosing root's line history at `copy_source_revs`. A copy whose `to` is not
/// under `root` belongs to a different atom and is simply not this atom's; one whose `from` is under no
/// recognized root is the documented outside-the-layout case (D-9 §2, review 147 F-3) and is left out,
/// not an error.
///
/// # Errors
/// [`Error::Ir`] (wrapping [`brygge_ir::Error::Invariant`]) if `from`'s enclosing root is found, but that
/// root has no recorded atom at or before the copy's own source revision (review 147 F-2) — this
/// "should not occur" for any dump D-9 describes: a copy's source content can only be read from a
/// retained snapshot (`tree::apply_file`/`apply_dir`'s own `kept`), which requires that content to have
/// already been tracked by some earlier, already-processed revision — i.e. for `history` to already hold
/// an entry. No hand-built dumpstream in this module's tests reaches it, for the same reason; it fails
/// closed rather than being silently dropped, per the honesty rule copies are already held to.
fn resolve_copies(
    copies: &[CopyRecord],
    source_revs: &[u64],
    revision: u64,
    root: &Root,
    policy: &crate::options::LayoutPolicy,
    history: &HashMap<String, BTreeMap<u64, AtomId>>,
) -> Result<Vec<CopyRecord>, Error> {
    let dir_prefix = format!("{}/", root.prefix);
    let mut out = Vec::new();
    for (copy, crev) in copies.iter().zip(source_revs.iter()) {
        let Some(to_rel) = copy
            .to
            .strip_prefix(&dir_prefix)
            .or_else(|| (copy.to == root.prefix).then_some(""))
        else {
            continue;
        };
        let Some(from_root) = layout::classify(&copy.from, policy) else {
            continue;
        };
        let Some(from_atom) = history
            .get(&from_root.prefix)
            .and_then(|h| h.range(..=*crev).next_back())
            .map(|(_, a)| *a)
        else {
            return Err(Error::Ir(brygge_ir::Error::Invariant(format!(
                "r{revision}: a copy into '{}' names source '{}' (at r{crev}), whose root \
                 '{}' has no recorded line atom at or before that revision",
                copy.to, copy.from, from_root.prefix
            ))));
        };
        let from_dir_prefix = format!("{}/", from_root.prefix);
        let from_rel = copy
            .from
            .strip_prefix(&from_dir_prefix)
            .unwrap_or(copy.from.as_str());
        out.push(CopyRecord {
            from: from_rel.to_string(),
            from_atom,
            to: to_rel.to_string(),
            status: EpistemicStatus::Stated,
        });
    }
    Ok(out)
}

/// Remove duplicate roots by prefix (several nodes under one root in the same revision), keeping the
/// first occurrence.
fn dedup_roots(roots: &mut Vec<Root>) {
    let mut seen = HashSet::new();
    roots.retain(|r| seen.insert(r.prefix.clone()));
}

/// Canonical root order within a revision (D-9 §2): trunk, then branches, then tags, each by bytes.
fn sort_roots_canonically(roots: &mut [Root], trunk_prefix: &str) {
    fn rank<'a>(r: &'a Root, trunk_prefix: &str) -> (u8, &'a [u8]) {
        if r.prefix == trunk_prefix {
            (0, r.name.as_bytes())
        } else if r.kind == RefKind::Tag {
            (2, r.name.as_bytes())
        } else {
            (1, r.name.as_bytes())
        }
    }
    roots.sort_by(|a, b| rank(a, trunk_prefix).cmp(&rank(b, trunk_prefix)));
}

/// Count files in the final tree outside every root the layout recognizes, same as the single-spine
/// path's own check — the aggregate flag `--branch-lines` keeps alongside the per-change drops above.
fn layout_honesty(
    final_tree: &Tree,
    opts: &Options,
    roots_created_from_outside_the_layout: u64,
    builder: &mut IrBuilder,
) {
    let outside = final_tree
        .keys()
        .filter(|path| layout::classify(path, &opts.layout).is_none())
        .count() as u64;
    if outside > 0 || roots_created_from_outside_the_layout > 0 {
        let what = if roots_created_from_outside_the_layout > 0 {
            format!(
                "paths outside the trunk/branches/tags layout ({outside}); a root created from \
                 outside the layout ({roots_created_from_outside_the_layout})"
            )
        } else {
            format!("paths outside the trunk/branches/tags layout ({outside})")
        };
        builder.add_flag(Flag {
            kind: FlagKind::ConventionViolation,
            what,
            count: outside + roots_created_from_outside_the_layout,
            reason: "the repository partly follows the layout; these paths belong to no \
                     reconstructed branch or tag"
                .to_string(),
        });
    }
}

/// Extract a revision's metadata claims from its properties — identical to the single-spine path's own
/// function; not shared directly because `crate::decode`'s is private to its module, and duplicating one
/// small, stable function is simpler than making it crate-public for one caller.
fn metadata_from_props(
    rev_props: &[(String, Vec<u8>)],
    unparseable_dates: &mut usize,
) -> MetadataClaims {
    let author = props::get(rev_props, props::SVN_AUTHOR)
        .filter(|b| !b.is_empty())
        .map(|name| Identity {
            name: Text {
                bytes: name.to_vec(),
                encoding: None,
            },
            email: None,
        });
    let message = props::get(rev_props, props::SVN_LOG).map(|m| Text {
        bytes: m.to_vec(),
        encoding: None,
    });
    let raw_date = props::get_str(rev_props, props::SVN_DATE);
    let time = raw_date
        .as_deref()
        .and_then(props::parse_svn_date)
        .map(|seconds| brygge_ir::Time {
            seconds,
            offset_minutes: Some(0),
        });
    if raw_date.is_some() && time.is_none() {
        *unparseable_dates = unparseable_dates.saturating_add(1);
    }
    MetadataClaims {
        author,
        author_time: time,
        committer: None,
        commit_time: None,
        message,
    }
}

/// Accumulated loss categories across a branch-lines import.
#[derive(Debug, Default)]
struct Loss {
    mergeinfo: bool,
    workflow: bool,
    custom: bool,
    empty_dirs: usize,
    unparseable_dates: usize,
    outside_root_ops: Vec<DropRecord>,
    revisions_with_no_branch_change: u64,
}

impl Loss {
    fn into_boundary(self) -> LossBoundary {
        let mut dropped = vec![DropRecord {
            class: LossClass::Representation,
            what: "SVN physical storage and dumpstream framing".to_string(),
            reason: "representation not assertion; the logical revisions are preserved".to_string(),
            step: None,
        }];
        if self.mergeinfo {
            dropped.push(DropRecord {
                class: LossClass::AdvisoryUnreliable,
                what: "svn:mergeinfo".to_string(),
                reason:
                    "advisory merge tracking, frequently incomplete; never promoted to a merge \
                         parent"
                        .to_string(),
                step: None,
            });
        }
        if self.workflow {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: "svn:eol-style / svn:keywords / svn:ignore (working-copy hints)".to_string(),
                reason:
                    "the stored normal-form bytes are carried verbatim; keyword/EOL expansion is \
                         a working-copy transform, not history"
                        .to_string(),
                step: None,
            });
        }
        if self.custom {
            dropped.push(DropRecord {
                class: LossClass::Other,
                what: "custom (user-defined) properties".to_string(),
                reason:
                    "not carried in this version (nothing consumes them, and the IR contract is \
                         not grown speculatively)"
                        .to_string(),
                step: None,
            });
        }
        if self.empty_dirs > 0 {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: format!("empty directories ({})", self.empty_dirs),
                reason:
                    "the IR has no empty-directory entity (as Git/prikk); the directory carried \
                         no file"
                        .to_string(),
                step: None,
            });
        }
        if self.unparseable_dates > 0 {
            dropped.push(DropRecord {
                class: LossClass::Other,
                what: format!("unparseable svn:date ({})", self.unparseable_dates),
                reason: "svn:date did not parse as an SVN-format timestamp; no time is carried \
                         rather than a wrong one"
                    .to_string(),
                step: None,
            });
        }
        if self.revisions_with_no_branch_change > 0 {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: format!(
                    "revisions with no branch change ({})",
                    self.revisions_with_no_branch_change
                ),
                reason:
                    "the revision named no path under the trunk/branches/tags layout, so no line \
                         gained an atom"
                        .to_string(),
                step: None,
            });
        }
        dropped.extend(self.outside_root_ops);
        LossBoundary { dropped }
    }
}

#[cfg(test)]
mod tests;

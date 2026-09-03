//! The decode orchestration (RFC 004 D-2/D-3/D-5): read a Git object database and build a
//! [`brygge_ir::Ir`], entirely *Stated* except opt-in marked-*Derived* rename hints.
//!
//! History is walked **parent-first** (an atom's `AtomId` is computed from its parents' `AtomId`s), and
//! per-commit path operations are computed by diffing full tree **snapshots** (child against first
//! parent) — auditable, deterministic, and explicit about modes and submodules.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use gix::ObjectId;
use gix::bstr::ByteSlice;

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, DropRecord, Identity, ImportProvenance, Ir, LossBoundary, LossClass, MetadataClaims,
    PathOp, RefKind, RefRecord, RenameHint, SourceIdentity, SourceKind,
};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};

use crate::{Error, Options, decoder_version, open};

/// The full path → (blob/link/gitlink object id, mode) contents of a tree.
type Snapshot = BTreeMap<String, (ObjectId, u32)>;

const DECODER: &str = "brygge-decode-git";

fn read_err(e: impl std::fmt::Display) -> Error {
    Error::Read(e.to_string())
}

/// Decode the Git repository at `path` into an [`Ir`] under `opts`.
///
/// # Errors
/// [`Error::Open`] if the path is not a readable repository; [`Error::FloorRefusal`] on a refused source
/// feature (submodule, grafts, shallow, replace ref — RFC 004 D-4); [`Error::Read`] on a malformed
/// object; [`Error::Ir`] if the assembled IR violates an invariant.
pub fn decode(path: &Path, opts: &Options) -> Result<Ir, Error> {
    let repo = open::open(path)?;
    open::check_repo_floor(&repo)?;

    let refs = scan_refs(&repo)?;
    let tips: Vec<ObjectId> = refs.iter().filter_map(|r| r.commit).collect();

    // Reachable commit set from every ref tip.
    let mut set: BTreeSet<ObjectId> = BTreeSet::new();
    if !tips.is_empty() {
        for info in repo.rev_walk(tips).all().map_err(read_err)? {
            set.insert(info.map_err(read_err)?.id);
        }
    }

    let parents = commit_parents(&repo, &set)?;
    let order = parent_first_order(&set, &parents)?;

    // Repo fingerprint: the smallest root-commit id (content-stable → deterministic, RFC 004 D-2).
    let repo_id = order
        .iter()
        .filter(|id| parents.get(*id).is_none_or(Vec::is_empty))
        .min()
        .map(|id| id.as_bytes().to_vec())
        .unwrap_or_default();

    let provenance = ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Git,
            repo_id: repo_id.clone(),
            atom_id: repo_id.clone(),
            signatures: Vec::new(),
        },
        brygge_version: decoder_version().to_string(),
        decoder: DECODER.to_string(),
        decoder_version: decoder_version().to_string(),
        params: opts.as_params(),
        import_time: None,
    };

    let mut builder = IrBuilder::new(provenance);
    let mut sha_to_atom: HashMap<ObjectId, AtomId> = HashMap::new();
    let mut snap_cache: HashMap<ObjectId, Snapshot> = HashMap::new();

    for id in &order {
        let commit = repo
            .find_object(*id)
            .map_err(read_err)?
            .try_into_commit()
            .map_err(read_err)?;

        let child_tree = commit.tree_id().map_err(read_err)?.detach();
        let child_snap = snapshot(&repo, child_tree, &mut snap_cache)?;

        let ps = parents.get(id).cloned().unwrap_or_default();
        let base_snap = match ps.first() {
            Some(p0) => {
                let p_commit = repo
                    .find_object(*p0)
                    .map_err(read_err)?
                    .try_into_commit()
                    .map_err(read_err)?;
                let p_tree = p_commit.tree_id().map_err(read_err)?.detach();
                snapshot(&repo, p_tree, &mut snap_cache)?
            }
            None => Snapshot::new(),
        };

        let (ops, rename_hints) = diff_to_ops(&repo, &base_snap, &child_snap, opts, &mut builder)?;
        let metadata = build_metadata(&commit)?;
        let signatures = extract_signatures(&commit)?;
        let parent_atoms: Vec<AtomId> = ps
            .iter()
            .filter_map(|p| sha_to_atom.get(p).copied())
            .collect();

        let atom_id = builder.add_atom(AtomDraft {
            parents: parent_atoms,
            ops,
            rename_hints,
            metadata,
            source: SourceIdentity {
                kind: SourceKind::Git,
                repo_id: repo_id.clone(),
                atom_id: id.as_bytes().to_vec(),
                signatures,
            },
            status: EpistemicStatus::Stated,
        });
        sha_to_atom.insert(*id, atom_id);
    }

    // Refs and the dropped-namespace loss records.
    let mut dropped_namespaces: BTreeSet<&'static str> = BTreeSet::new();
    for r in &refs {
        match &r.kind {
            ScannedKind::Branch(name) | ScannedKind::Tag(name) => {
                if let Some(target) = r.commit.and_then(|c| sha_to_atom.get(&c).copied()) {
                    let kind = if matches!(r.kind, ScannedKind::Branch(_)) {
                        RefKind::Branch
                    } else {
                        RefKind::Tag
                    };
                    builder.add_ref(RefRecord {
                        name: name.clone(),
                        kind,
                        target,
                        status: EpistemicStatus::Stated,
                        source: None,
                    })?;
                }
            }
            ScannedKind::Dropped(ns) => {
                dropped_namespaces.insert(ns);
            }
        }
    }

    builder.set_loss(loss_boundary(&dropped_namespaces));
    builder.finish().map_err(Error::Ir)
}

/// A ref as scanned, categorised, and peeled.
struct ScannedRef {
    kind: ScannedKind,
    /// The commit it (ultimately) points at, if any.
    commit: Option<ObjectId>,
}

enum ScannedKind {
    Branch(String),
    Tag(String),
    Dropped(&'static str),
}

/// Scan refs: refuse replace refs (RFC 004 D-4), categorise the rest, and peel each to a commit id.
fn scan_refs(repo: &gix::Repository) -> Result<Vec<ScannedRef>, Error> {
    let mut out = Vec::new();
    let platform = repo.references().map_err(read_err)?;
    for r in platform.all().map_err(read_err)? {
        let mut r = r.map_err(read_err)?;
        let name = r.name().as_bstr().to_str_lossy().into_owned();

        if name.starts_with("refs/replace/") {
            return Err(Error::FloorRefusal {
                feature: "replace ref".to_string(),
                reason: format!(
                    "{name} rewrites the object graph a reader would see; refused rather than \
                     importing the rewritten view silently"
                ),
            });
        }

        let commit = r.peel_to_id().ok().map(|id| id.detach()).filter(|id| {
            repo.find_object(*id)
                .is_ok_and(|o| matches!(o.kind, gix::objs::Kind::Commit))
        });

        let kind = if let Some(b) = name.strip_prefix("refs/heads/") {
            ScannedKind::Branch(b.to_string())
        } else if let Some(t) = name.strip_prefix("refs/tags/") {
            ScannedKind::Tag(t.to_string())
        } else if name.starts_with("refs/remotes/") {
            ScannedKind::Dropped("remote-tracking")
        } else if name.starts_with("refs/notes/") {
            ScannedKind::Dropped("notes")
        } else if name == "refs/stash" {
            ScannedKind::Dropped("stash")
        } else {
            // HEAD and any other odd ref namespace: not authored history.
            ScannedKind::Dropped("other")
        };
        out.push(ScannedRef { kind, commit });
    }
    Ok(out)
}

/// Map each commit to its parents that are within `set` (external parents become roots).
fn commit_parents(
    repo: &gix::Repository,
    set: &BTreeSet<ObjectId>,
) -> Result<BTreeMap<ObjectId, Vec<ObjectId>>, Error> {
    let mut parents = BTreeMap::new();
    for id in set {
        let commit = repo
            .find_object(*id)
            .map_err(read_err)?
            .try_into_commit()
            .map_err(read_err)?;
        let ps: Vec<ObjectId> = commit
            .parent_ids()
            .map(|p| p.detach())
            .filter(|p| set.contains(p))
            .collect();
        parents.insert(*id, ps);
    }
    Ok(parents)
}

/// Deterministic parent-first order: Kahn's algorithm with the ready set ordered by object id.
fn parent_first_order(
    set: &BTreeSet<ObjectId>,
    parents: &BTreeMap<ObjectId, Vec<ObjectId>>,
) -> Result<Vec<ObjectId>, Error> {
    let mut indeg: BTreeMap<ObjectId, usize> = set.iter().map(|id| (*id, 0usize)).collect();
    let mut children: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
    for (id, ps) in parents {
        for p in ps {
            children.entry(*p).or_default().push(*id);
            if let Some(d) = indeg.get_mut(id) {
                *d += 1;
            }
        }
    }
    let mut ready: BTreeSet<ObjectId> = indeg
        .iter()
        .filter_map(|(id, d)| (*d == 0).then_some(*id))
        .collect();
    let mut order = Vec::with_capacity(set.len());
    while let Some(id) = ready.iter().next().copied() {
        ready.remove(&id);
        order.push(id);
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                if let Some(d) = indeg.get_mut(kid) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        ready.insert(*kid);
                    }
                }
            }
        }
    }
    if order.len() != set.len() {
        return Err(Error::Read(
            "commit graph contains a cycle (a Git history must be acyclic)".to_string(),
        ));
    }
    Ok(order)
}

/// Build the full path→(oid,mode) snapshot of a tree, caching by tree id. A gitlink (submodule) entry
/// is a floor refusal (RFC 004 D-4).
fn snapshot(
    repo: &gix::Repository,
    tree_id: ObjectId,
    cache: &mut HashMap<ObjectId, Snapshot>,
) -> Result<Snapshot, Error> {
    if let Some(s) = cache.get(&tree_id) {
        return Ok(s.clone());
    }
    let mut out = Snapshot::new();
    walk_tree(repo, tree_id, "", &mut out)?;
    cache.insert(tree_id, out.clone());
    Ok(out)
}

fn walk_tree(
    repo: &gix::Repository,
    tree_id: ObjectId,
    prefix: &str,
    out: &mut Snapshot,
) -> Result<(), Error> {
    let tree = repo
        .find_object(tree_id)
        .map_err(read_err)?
        .try_into_tree()
        .map_err(read_err)?;
    for entry in tree.iter() {
        let entry = entry.map_err(read_err)?;
        let name = entry.filename().to_str_lossy();
        let path = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let mode = entry.mode();
        let oid = entry.oid().to_owned();
        match mode.kind() {
            gix::objs::tree::EntryKind::Tree => {
                walk_tree(repo, oid, &path, out)?;
            }
            gix::objs::tree::EntryKind::Commit => {
                return Err(Error::FloorRefusal {
                    feature: "submodule".to_string(),
                    reason: format!(
                        "submodule (gitlink) at '{path}' points outside this repository; refused \
                         rather than approximated"
                    ),
                });
            }
            _ => {
                out.insert(path, (oid, u32::from(mode.value())));
            }
        }
    }
    Ok(())
}

/// Diff `base` → `child` into literal path operations (all *Stated*), plus — only if
/// `opts.detect_renames` — marked *Derived* rename hints for exact-content moves. The literal
/// delete+add always remain; a hint sits beside them, never in place of them (RFC 004 D-3).
fn diff_to_ops(
    repo: &gix::Repository,
    base: &Snapshot,
    child: &Snapshot,
    opts: &Options,
    builder: &mut IrBuilder,
) -> Result<(Vec<PathOp>, Vec<RenameHint>), Error> {
    let mut ops = Vec::new();
    let mut added_by_oid: BTreeMap<ObjectId, Vec<String>> = BTreeMap::new();
    let mut deleted_by_oid: BTreeMap<ObjectId, Vec<String>> = BTreeMap::new();

    for (path, (oid, mode)) in child {
        match base.get(path) {
            None => {
                let blob = builder.add_blob(blob_bytes(repo, *oid)?);
                ops.push(PathOp::Add {
                    path: path.clone(),
                    blob,
                    mode: *mode,
                    status: EpistemicStatus::Stated,
                });
                added_by_oid.entry(*oid).or_default().push(path.clone());
            }
            Some((base_oid, base_mode)) => {
                if base_oid != oid || base_mode != mode {
                    let blob = builder.add_blob(blob_bytes(repo, *oid)?);
                    ops.push(PathOp::Modify {
                        path: path.clone(),
                        blob,
                        mode: *mode,
                        status: EpistemicStatus::Stated,
                    });
                }
            }
        }
    }
    for (path, (oid, _)) in base {
        if !child.contains_key(path) {
            ops.push(PathOp::Delete {
                path: path.clone(),
                status: EpistemicStatus::Stated,
            });
            deleted_by_oid.entry(*oid).or_default().push(path.clone());
        }
    }

    let mut hints = Vec::new();
    if opts.detect_renames {
        for (oid, froms) in &deleted_by_oid {
            if let Some(tos) = added_by_oid.get(oid) {
                for from in froms {
                    for to in tos {
                        hints.push(RenameHint {
                            from: from.clone(),
                            to: to.clone(),
                            status: EpistemicStatus::Derived(Derivation {
                                kind: DerivationKind::InferredRename,
                                by: DECODER.to_string(),
                                decoder_version: decoder_version().to_string(),
                                params: opts.as_params(),
                                confidence: Some(100),
                            }),
                        });
                    }
                }
            }
        }
    }
    Ok((ops, hints))
}

fn blob_bytes(repo: &gix::Repository, oid: ObjectId) -> Result<Vec<u8>, Error> {
    Ok(repo
        .find_object(oid)
        .map_err(read_err)?
        .try_into_blob()
        .map_err(read_err)?
        .data
        .clone())
}

fn build_metadata(commit: &gix::Commit<'_>) -> Result<MetadataClaims, Error> {
    let author = commit.author().map_err(read_err)?;
    let committer = commit.committer().map_err(read_err)?;
    let message = commit.message_raw().map_err(read_err)?;
    Ok(MetadataClaims {
        author: Some(Identity {
            name: author.name.to_str_lossy().into_owned(),
            email: author.email.to_str_lossy().into_owned(),
        }),
        committer: Some(Identity {
            name: committer.name.to_str_lossy().into_owned(),
            email: committer.email.to_str_lossy().into_owned(),
        }),
        message: Some(message.to_str_lossy().into_owned()),
        author_time: Some(author.seconds()),
        commit_time: Some(committer.seconds()),
    })
}

/// The commit's GPG signature, preserved opaquely (RFC 004 D-2 / SRC-G3). It verifies nothing in any
/// target; it is carried, never interpreted.
fn extract_signatures(commit: &gix::Commit<'_>) -> Result<Vec<Vec<u8>>, Error> {
    let decoded = commit.decode().map_err(read_err)?;
    Ok(match decoded.extra_headers().pgp_signature() {
        Some(sig) => vec![sig.to_vec()],
        None => Vec::new(),
    })
}

/// The Git loss boundary (RFC 004 D-5): the representation-class drops, every one class-stated.
fn loss_boundary(dropped_namespaces: &BTreeSet<&'static str>) -> LossBoundary {
    let mut dropped = vec![
        DropRecord {
            class: LossClass::Representation,
            what: "packfile and delta layout, physical object store".to_string(),
            reason: "representation not assertion; reconstructible from the objects (PR-7)"
                .to_string(),
        },
        DropRecord {
            class: LossClass::Representation,
            what: "index and working tree".to_string(),
            reason: "local state, not history (PR-7)".to_string(),
        },
        DropRecord {
            class: LossClass::Representation,
            what: "reflogs".to_string(),
            reason: "local operation log, not history (PR-7)".to_string(),
        },
    ];
    for ns in dropped_namespaces {
        dropped.push(DropRecord {
            class: LossClass::Representation,
            what: format!("{ns} refs"),
            reason: "workflow/representation refs, not authored history (RFC 004 D-5, OQ-B)"
                .to_string(),
        });
    }
    LossBoundary { dropped }
}

#[cfg(test)]
mod tests;

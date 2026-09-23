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

/// An annotated tag's opaque object id and any signatures, preserved in a ref's `source` (`PR-4`).
type TagIdentity = (ObjectId, Vec<Vec<u8>>);

const DECODER: &str = "brygge-decode-git";

fn read_err(e: impl std::fmt::Display) -> Error {
    Error::Read(e.to_string())
}

/// Render `bytes` as valid UTF-8 kept verbatim and each invalid byte escaped as `\xNN` — never
/// `to_str_lossy`'s `U+FFFD` substitution, which would silently alter the bytes a refusal message
/// names (D-3(i), CR-03).
fn escape_invalid_utf8(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                let valid_prefix = rest.get(..valid_up_to).unwrap_or(&[]);
                out.push_str(std::str::from_utf8(valid_prefix).unwrap_or_default());
                let bad_len = e.error_len().unwrap_or(rest.len() - valid_up_to).max(1);
                let bad_end = (valid_up_to + bad_len).min(rest.len());
                for &b in rest.get(valid_up_to..bad_end).unwrap_or(&[]) {
                    let _ = write!(out, "\\x{b:02X}");
                }
                rest = rest.get(bad_end..).unwrap_or(&[]);
                if rest.is_empty() {
                    break;
                }
            }
        }
    }
    out
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

    let (refs, symbolic_ref_count) = scan_refs(&repo)?;

    // CR-05: walk tips come only from carried refs (non-symbolic refs/heads/* and refs/tags/*,
    // a tag peeled to its commit). Compute this set first — before anything is read for atoms —
    // so a commit reachable only from a dropped namespace (refs/remotes/*, refs/notes/*,
    // refs/stash, ...) is never read as history.
    let carried_tips: Vec<ObjectId> = refs
        .iter()
        .filter(|r| matches!(r.kind, ScannedKind::Branch(_) | ScannedKind::Tag(_)))
        .filter_map(|r| r.commit)
        .collect();
    let mut set: BTreeSet<ObjectId> = BTreeSet::new();
    if !carried_tips.is_empty() {
        for info in repo
            .rev_walk(carried_tips.clone())
            .all()
            .map_err(read_err)?
        {
            set.insert(info.map_err(read_err)?.id);
        }
    }

    // The count for the loss record only: commits reachable only from a dropped-namespace ref.
    // `.with_hidden(carried_tips)` paints every carried-reachable commit as unwanted, so the walk
    // visits dropped-only commits without re-walking the shared history. This is a *count*, not a
    // requirement (RFC 004 R-2, 2026-09-23 review of this handoff): it reads commit headers only —
    // never a tree or blob, and nothing it finds becomes an atom — and it never fails the decode. A
    // broken commit in dropped-only history (e.g. a corrupt stash) makes the count unavailable, not
    // the decode.
    let dropped_tips: Vec<ObjectId> = refs
        .iter()
        .filter(|r| matches!(r.kind, ScannedKind::Dropped(_)))
        .filter_map(|r| r.commit)
        .collect();
    let dropped_only_commits = count_dropped_only_commits(&repo, dropped_tips, carried_tips);

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
    let mut unparseable_times = 0u64;

    for id in &order {
        let commit = repo
            .find_object(*id)
            .map_err(read_err)?
            .try_into_commit()
            .map_err(read_err)?;

        let child_tree = commit.tree_id().map_err(read_err)?.detach();
        let child_snap = snapshot(&repo, child_tree, *id, &mut snap_cache)?;

        let ps = parents.get(id).cloned().unwrap_or_default();
        let base_snap = match ps.first() {
            Some(p0) => {
                let p_commit = repo
                    .find_object(*p0)
                    .map_err(read_err)?
                    .try_into_commit()
                    .map_err(read_err)?;
                let p_tree = p_commit.tree_id().map_err(read_err)?.detach();
                snapshot(&repo, p_tree, *p0, &mut snap_cache)?
            }
            None => Snapshot::new(),
        };

        let (ops, rename_hints) = diff_to_ops(&repo, &base_snap, &child_snap, opts, &mut builder)?;
        let (metadata, unparseable) = build_metadata(&commit)?;
        unparseable_times += unparseable;
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

    // Refs, the dropped-namespace loss records, and annotated-tag identity preservation (OQ-B).
    let mut dropped_namespace_counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut annotated_tags = 0u64;
    let mut non_commit_refs = 0u64;
    for r in &refs {
        match &r.kind {
            ScannedKind::Branch(name) => {
                match r.commit.and_then(|c| sha_to_atom.get(&c).copied()) {
                    Some(target) => {
                        builder.add_ref(RefRecord {
                            name: name.clone(),
                            kind: RefKind::Branch,
                            target,
                            status: EpistemicStatus::Stated,
                            source: None,
                        })?;
                    }
                    // CR-06: peeled to a tree or blob, not a commit — recorded, not silently skipped.
                    None => non_commit_refs += 1,
                }
            }
            ScannedKind::Tag(name) => match r.commit.and_then(|c| sha_to_atom.get(&c).copied()) {
                Some(target) => {
                    // An annotated tag preserves its own opaque object id + signature (PR-4/SRC-G3);
                    // its tagger and message have no slot in the RefRecord and are recorded as loss.
                    let source = r.tag_identity.as_ref().map(|(tag_id, signatures)| {
                        annotated_tags += 1;
                        SourceIdentity {
                            kind: SourceKind::Git,
                            repo_id: repo_id.clone(),
                            atom_id: tag_id.as_bytes().to_vec(),
                            signatures: signatures.clone(),
                        }
                    });
                    builder.add_ref(RefRecord {
                        name: name.clone(),
                        kind: RefKind::Tag,
                        target,
                        status: EpistemicStatus::Stated,
                        source,
                    })?;
                }
                // CR-06: peeled to a tree or blob, not a commit — recorded, not silently skipped.
                None => non_commit_refs += 1,
            },
            ScannedKind::Dropped(ns) => {
                *dropped_namespace_counts.entry(ns).or_insert(0) += 1;
            }
        }
    }

    builder.set_loss(loss_boundary(
        &dropped_namespace_counts,
        dropped_only_commits,
        non_commit_refs,
        annotated_tags,
        symbolic_ref_count,
        unparseable_times,
    ));
    builder.finish().map_err(Error::Ir)
}

/// The result of sizing the "commits reachable only from dropped refs" loss record (CR-05).
enum DroppedOnlyCommits {
    /// The exact count.
    Counted(u64),
    /// The walk itself failed (e.g. a corrupt or missing commit in dropped-only history); the count is
    /// unknown rather than guessed. `reason` is the underlying error's display text.
    Unavailable(String),
}

/// Count commits reachable only from `dropped_tips`, never `carried_tips` (CR-05). This never fails the
/// decode (2026-09-23 review R-2 of this handoff): any error during the walk — setup or iteration —
/// becomes [`DroppedOnlyCommits::Unavailable`] instead of propagating, since this is a loss-record count,
/// not a requirement for the decode to succeed.
fn count_dropped_only_commits(
    repo: &gix::Repository,
    dropped_tips: Vec<ObjectId>,
    carried_tips: Vec<ObjectId>,
) -> DroppedOnlyCommits {
    if dropped_tips.is_empty() {
        return DroppedOnlyCommits::Counted(0);
    }
    let walk = match repo.rev_walk(dropped_tips).with_hidden(carried_tips).all() {
        Ok(walk) => walk,
        Err(e) => return DroppedOnlyCommits::Unavailable(e.to_string()),
    };
    let mut count = 0u64;
    for info in walk {
        match info {
            Ok(_) => count += 1,
            Err(e) => return DroppedOnlyCommits::Unavailable(e.to_string()),
        }
    }
    DroppedOnlyCommits::Counted(count)
}

/// A ref as scanned, categorised, and peeled.
struct ScannedRef {
    kind: ScannedKind,
    /// The commit it (ultimately) points at, if any.
    commit: Option<ObjectId>,
    /// For an **annotated** tag: the tag object's own id and any signature (`PR-4`/`SRC-G3`), preserved
    /// opaquely in the ref's `source`. `None` for branches and lightweight tags.
    tag_identity: Option<TagIdentity>,
}

enum ScannedKind {
    Branch(String),
    Tag(String),
    Dropped(&'static str),
}

const HEADS_PREFIX: &[u8] = b"refs/heads/";
const TAGS_PREFIX: &[u8] = b"refs/tags/";

/// Scan refs: refuse replace refs (RFC 004 D-4), categorise the rest, and peel each to a commit id.
/// Returns the carried refs plus a count of symbolic refs skipped (CR-16).
fn scan_refs(repo: &gix::Repository) -> Result<(Vec<ScannedRef>, u64), Error> {
    let mut out = Vec::new();
    let mut symbolic_refs = 0u64;
    let platform = repo.references().map_err(read_err)?;
    for r in platform.all().map_err(read_err)? {
        let mut r = r.map_err(read_err)?;
        let raw_name: Vec<u8> = r.name().as_bstr().to_vec();
        let name = raw_name.to_str_lossy(); // CR-03: lossy until RFC 011 (text as bytes); only used
        // for the `refs/replace/` prefix check and dropped-namespace matching below, both ASCII.

        if raw_name.starts_with(b"refs/replace/") {
            return Err(Error::FloorRefusal {
                feature: "replace ref".to_string(),
                reason: format!(
                    "{name} rewrites the object graph a reader would see; refused rather than \
                     importing the rewritten view silently"
                ),
            });
        }

        // A symbolic ref (e.g. `refs/remotes/origin/HEAD`) is an alias to another ref, in any
        // namespace; the IR has no alias concept (RFC 004 OQ-B, CR-16). `try_id()` is `None` exactly
        // when the ref's own stored form is symbolic — including a *dangling* symbolic ref, whose
        // target does not exist, since this reflects the ref's own shape, not whether it resolves.
        // gix's `id()` panics on this case; `try_id()` is the fallible form. Skip it: it contributes no
        // walk tip and is not carried as a ref. Its target, if any, is a distinct ref that `platform.all()`
        // yields on its own and this same loop carries or drops on its own merits.
        let Some(direct) = r.try_id().map(|id| id.detach()) else {
            symbolic_refs += 1;
            continue;
        };

        // CR-03/D-3(i): a carried ref's name (after its namespace prefix) must be valid UTF-8, never
        // converted lossily. Namespace classification itself only compares the ASCII prefix bytes, so
        // it is unaffected by what follows.
        let carried_suffix: Option<(&[u8], bool)> = if raw_name.starts_with(HEADS_PREFIX) {
            Some((raw_name.get(HEADS_PREFIX.len()..).unwrap_or(&[]), true))
        } else if raw_name.starts_with(TAGS_PREFIX) {
            Some((raw_name.get(TAGS_PREFIX.len()..).unwrap_or(&[]), false))
        } else {
            None
        };
        if let Some((suffix, _)) = carried_suffix {
            if std::str::from_utf8(suffix).is_err() {
                return Err(Error::FloorRefusal {
                    feature: "non-UTF-8 ref name".to_string(),
                    reason: format!(
                        "ref name '{}' is not valid UTF-8 (invalid bytes shown as \\xNN)",
                        escape_invalid_utf8(&raw_name)
                    ),
                });
            }
        }

        let mut tag_identity = None;
        let (kind, commit) = if let Some((suffix, is_branch)) = carried_suffix {
            // Checked valid UTF-8 just above.
            let suffix_str = std::str::from_utf8(suffix).unwrap_or_default().to_string();
            if is_branch {
                (ScannedKind::Branch(suffix_str), peel_carried(repo, &mut r)?)
            } else {
                tag_identity = annotated_tag_identity(repo, direct)?;
                (ScannedKind::Tag(suffix_str), peel_carried(repo, &mut r)?)
            }
        } else {
            // Dropped namespaces are peeled leniently: they contribute no atom and no hard error, only
            // (optionally) a count of commits reachable only from them (CR-05).
            let dropped_kind = if name.starts_with("refs/remotes/") {
                ScannedKind::Dropped("remote-tracking")
            } else if name.starts_with("refs/notes/") {
                ScannedKind::Dropped("notes")
            } else if name == "refs/stash" {
                ScannedKind::Dropped("stash")
            } else {
                // HEAD and any other odd ref namespace: not authored history.
                ScannedKind::Dropped("other")
            };
            let lenient_commit = r.peel_to_id().ok().map(|id| id.detach()).filter(|id| {
                repo.find_object(*id)
                    .is_ok_and(|o| matches!(o.kind, gix::objs::Kind::Commit))
            });
            (dropped_kind, lenient_commit)
        };
        out.push(ScannedRef {
            kind,
            commit,
            tag_identity,
        });
    }
    Ok((out, symbolic_refs))
}

/// Peel a `refs/heads/*` or `refs/tags/*` ref to its commit (CR-06). A ref whose target is missing
/// entirely is a broken repository (`Error::Read`), not a drop. A ref that peels cleanly to a tree or
/// blob is `None` — the caller counts it as a non-commit-target ref rather than silently skipping it.
fn peel_carried(
    repo: &gix::Repository,
    r: &mut gix::Reference<'_>,
) -> Result<Option<ObjectId>, Error> {
    let id = r.peel_to_id().map_err(read_err)?.detach();
    match repo.find_object(id) {
        Ok(o) if matches!(o.kind, gix::objs::Kind::Commit) => Ok(Some(id)),
        Ok(_) => Ok(None),
        Err(e) => Err(read_err(e)),
    }
}

/// If `direct` is an **annotated** tag object, return its id and any GPG signature, preserved opaquely
/// (`PR-4`/`SRC-G3` — the tag verifies nothing in any target, but is the cryptographic link back to the
/// source). Returns `None` for a lightweight tag (whose direct target is the commit itself).
fn annotated_tag_identity(
    repo: &gix::Repository,
    direct: ObjectId,
) -> Result<Option<TagIdentity>, Error> {
    let Ok(object) = repo.find_object(direct) else {
        return Ok(None);
    };
    if !matches!(object.kind, gix::objs::Kind::Tag) {
        return Ok(None);
    }
    let tag = object.try_into_tag().map_err(read_err)?;
    let signatures = tag
        .decode()
        .map_err(read_err)?
        .signature
        .map(|s| s.to_vec())
        .into_iter()
        .collect();
    Ok(Some((direct, signatures)))
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
    commit_id: ObjectId,
    cache: &mut HashMap<ObjectId, Snapshot>,
) -> Result<Snapshot, Error> {
    if let Some(s) = cache.get(&tree_id) {
        return Ok(s.clone());
    }
    let mut out = Snapshot::new();
    walk_tree(repo, tree_id, "", commit_id, &mut out)?;
    cache.insert(tree_id, out.clone());
    Ok(out)
}

fn walk_tree(
    repo: &gix::Repository,
    tree_id: ObjectId,
    prefix: &str,
    commit_id: ObjectId,
    out: &mut Snapshot,
) -> Result<(), Error> {
    let tree = repo
        .find_object(tree_id)
        .map_err(read_err)?
        .try_into_tree()
        .map_err(read_err)?;
    for entry in tree.iter() {
        let entry = entry.map_err(read_err)?;
        let raw_name = entry.filename(); // &BStr; CR-03/D-3(i): checked below, never `to_str_lossy`.
        let Ok(name) = std::str::from_utf8(raw_name) else {
            return Err(Error::FloorRefusal {
                feature: "non-UTF-8 path".to_string(),
                reason: format!(
                    "commit {commit_id}: path '{prefix}{}{}' is not valid UTF-8 (invalid bytes \
                     shown as \\xNN)",
                    if prefix.is_empty() { "" } else { "/" },
                    escape_invalid_utf8(raw_name)
                ),
            });
        };
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let mode = entry.mode();
        let oid = entry.oid().to_owned();
        match mode.kind() {
            gix::objs::tree::EntryKind::Tree => {
                walk_tree(repo, oid, &path, commit_id, out)?;
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
                // CR-03 §2.3.4: valid UTF-8 paths cannot collide at this point (git trees disallow
                // duplicate entry names, and entry names cannot contain '/'); this is defense in depth.
                if out.contains_key(&path) {
                    return Err(Error::Read(format!(
                        "commit {commit_id}: duplicate path '{path}' in tree snapshot"
                    )));
                }
                out.insert(path, (oid, u32::from(mode.value())));
            }
        }
    }
    Ok(())
}

/// Diff `base` → `child` into literal path operations (all *Stated*), plus — only if
/// `opts.infer_renames` — marked *Derived* rename hints for exact-content moves. The literal
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
    if opts.infer_renames {
        // Exact-content moves only (RFC 004 D-3, OQ-A). A hint is emitted **only** when a blob deleted
        // at exactly one path reappears added at exactly one path — an unambiguous 1:1 move. An
        // ambiguous many-to-many identical-content case (the same bytes deleted at several paths and
        // added at several) is left unmarked: brygge does not guess which path became which. Nothing is
        // lost by declining — the literal delete+add ops remain (D-3). Similarity-based detection above
        // exact content is OQ-A, deferred until a consumer can give a threshold a fitness signal.
        for (oid, froms) in &deleted_by_oid {
            let [from] = froms.as_slice() else {
                continue; // this blob was deleted at several paths — ambiguous, decline to guess
            };
            let Some([to]) = added_by_oid.get(oid).map(Vec::as_slice) else {
                continue; // not re-added, or re-added at several paths — decline to guess
            };
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

/// Parse the seconds field of a raw signature `time` string strictly (CR-16 R-1, handoff §3.1c amended).
/// The first whitespace-separated token must match `-?[0-9]+` exactly and fit `i64`, or the result is
/// `None`. Neither gix's `seconds()` (silently defaults to 0) nor its `time()` (via `gix_date`'s
/// `parse_header`, which salvages the leading digits of a token like `"1695456000abc"` and silently
/// defaults a malformed timezone offset) is used: both are the silent-approximation class CR-16 exists
/// to remove.
fn strict_seconds(raw: &str) -> Option<i64> {
    let token = raw.split_whitespace().next()?;
    let digits = token.strip_prefix('-').unwrap_or(token);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse::<i64>().ok()
}

/// Build message/authorship claims. Times are parsed strictly ([`strict_seconds`], CR-16): an unparseable
/// time becomes an absent claim, never a fabricated or salvaged one (NG-5). Returns the claims plus how
/// many of the two time fields failed to parse, for the caller to fold into the loss boundary.
fn build_metadata(commit: &gix::Commit<'_>) -> Result<(MetadataClaims, u64), Error> {
    let author = commit.author().map_err(read_err)?;
    let committer = commit.committer().map_err(read_err)?;
    let message = commit.message_raw().map_err(read_err)?;
    let mut unparseable = 0u64;
    let author_time = strict_seconds(author.time);
    if author_time.is_none() {
        unparseable += 1;
    }
    let commit_time = strict_seconds(committer.time);
    if commit_time.is_none() {
        unparseable += 1;
    }
    Ok((
        MetadataClaims {
            author: Some(Identity {
                name: author.name.to_str_lossy().into_owned(),
                email: author.email.to_str_lossy().into_owned(),
            }),
            committer: Some(Identity {
                name: committer.name.to_str_lossy().into_owned(),
                email: committer.email.to_str_lossy().into_owned(),
            }),
            message: Some(message.to_str_lossy().into_owned()),
            author_time,
            commit_time,
        },
        unparseable,
    ))
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

/// The Git loss boundary (RFC 004 D-5): the representation-class drops, every one class-stated, plus the
/// annotated-tag metadata that the IR `RefRecord` cannot yet hold, symbolic refs skipped (CR-16), any
/// author/committer time that failed to parse (CR-16), commits reachable only from a dropped ref
/// namespace (CR-05), and refs that peel to a tree or blob rather than a commit (CR-06) — recorded,
/// never silently omitted (`PR-9`).
fn loss_boundary(
    dropped_namespace_counts: &BTreeMap<&'static str, u64>,
    dropped_only_commits: DroppedOnlyCommits,
    non_commit_refs: u64,
    annotated_tags: u64,
    symbolic_refs: u64,
    unparseable_times: u64,
) -> LossBoundary {
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
    for (ns, count) in dropped_namespace_counts {
        dropped.push(DropRecord {
            class: LossClass::Representation,
            what: format!("{ns} refs ({count})"),
            reason: "workflow/representation refs, not authored history (RFC 004 D-5, OQ-B)"
                .to_string(),
        });
    }
    match dropped_only_commits {
        DroppedOnlyCommits::Counted(0) => {}
        DroppedOnlyCommits::Counted(n) => {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: format!("commits reachable only from dropped refs ({n})"),
                reason:
                    "workflow state (stash, notes, remote-tracking), not authored history of a \
                         carried ref (RFC 004 D-5, OQ-B)"
                        .to_string(),
            });
        }
        DroppedOnlyCommits::Unavailable(reason) => {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: format!(
                    "commits reachable only from dropped refs (count unavailable: {reason})"
                ),
                reason:
                    "workflow state (stash, notes, remote-tracking), not authored history of a \
                         carried ref (RFC 004 D-5, OQ-B); the count itself could not be computed, \
                         stated rather than guessed"
                        .to_string(),
            });
        }
    }
    if non_commit_refs > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("refs to non-commit objects ({non_commit_refs})"),
            reason: "a tag or branch naming a tree or blob; the IR's refs point at history atoms \
                     (PR-9)"
                .to_string(),
        });
    }
    if annotated_tags > 0 {
        // The annotated tag's object id and signature ARE preserved (in the ref's source, PR-4/SRC-G3);
        // its tagger and message are authored content the RefRecord has no slot for — so this is an
        // `Other`-class (not representation) drop, and it makes the import an honest "recorded loss"
        // (CL-08) rather than clean. A future brygge-ir RefRecord metadata slot would close it.
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("annotated tag tagger and message ({annotated_tags} tag(s))"),
            reason: "authored tag metadata with no RefRecord slot in this IR contract; the tag's \
                     object id and signature are preserved as source identity, the rest is recorded \
                     here rather than silently omitted (PR-9, RFC 004 OQ-B)"
                .to_string(),
        });
    }
    if symbolic_refs > 0 {
        dropped.push(DropRecord {
            class: LossClass::Representation,
            what: format!("symbolic refs ({symbolic_refs})"),
            reason:
                "an alias to another ref; the IR has no alias concept; the target ref is carried \
                     on its own (RFC 004 OQ-B, CR-16)"
                    .to_string(),
        });
    }
    if unparseable_times > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("unparseable author/committer times ({unparseable_times})"),
            reason: "the source's time field could not be parsed; the claim is absent rather than \
                     fabricated (NG-5, CR-16)"
                .to_string(),
        });
    }
    LossBoundary { dropped }
}

#[cfg(test)]
mod tests;

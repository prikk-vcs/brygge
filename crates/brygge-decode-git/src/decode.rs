//! The decode orchestration (RFC 004 D-2/D-3/D-5): read a Git object database and build a
//! [`brygge_ir::Ir`], entirely *Stated* except opt-in marked-*Derived* copy records. Every object read
//! — commit, tree, blob, annotated tag — is re-hashed against its claimed id before use (batch-2 handoff
//! §1.1, RFC 004 batch-2 corrections).
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
    Annotation, AtomId, CopyRecord, DropRecord, Extra, Identity, ImportProvenance, Ir,
    LossBoundary, LossClass, MetadataClaims, PathOp, RefKind, RefRecord, Signature, SourceIdentity,
    SourceKind, Text, Time,
};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};

use crate::{Error, Options, decoder_version, floor, open};

/// The full path → (blob/link/gitlink object id, mode) contents of a tree.
type Snapshot = BTreeMap<String, (ObjectId, u32)>;

/// An annotated tag's opaque object id, any signatures, and its tagger/time/message, preserved in a ref's
/// `source`/`annotation` (`PR-4`, batch-2 handoff §1.6).
type TagIdentity = (ObjectId, Vec<Signature>, Option<Annotation>);

const DECODER: &str = "brygge-decode-git";

/// Resource ceilings (RFC 010 D-4). One place for every Git ceiling; tests construct a small instance
/// instead of needing gigabytes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The maximum size of one blob, checked from the object **header** (no inflation) before it is
    /// read.
    pub(crate) max_blob_bytes: u64,
    /// The maximum number of commits imported from one repository, checked during the walk, before any
    /// atom is built.
    pub(crate) max_commits: u64,
    /// The maximum length of one path, checked while walking trees.
    pub(crate) max_path_bytes: usize,
    /// The maximum tree nesting depth, checked while walking trees (`walk_tree` is iterative — an
    /// explicit stack — precisely so a deep tree cannot overflow the process stack; RFC 010 CR-10).
    pub(crate) max_tree_depth: usize,
    /// The maximum number of tag objects peeled through for one ref (a tag pointing at a tag, and so
    /// on), checked while peeling (batch-2 handoff §1.1, review 011 R-2) — an attacker-chosen chain of
    /// tag objects cannot force unbounded verified reads for one ref.
    pub(crate) max_tag_chain: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_blob_bytes: 1024 * 1024 * 1024,
            max_commits: 10_000_000,
            max_path_bytes: 4096,
            max_tree_depth: 256,
            max_tag_chain: 32,
        }
    }
}

fn resource_limit(what: &str, ceiling: impl std::fmt::Display) -> Error {
    Error::ResourceLimit {
        what: what.to_string(),
        ceiling: ceiling.to_string(),
    }
}

fn read_err(e: impl std::fmt::Display) -> Error {
    Error::Read(e.to_string())
}

/// Look up `id` and verify its content actually hashes to it, before returning it (batch-2 handoff §1.1,
/// PR-4/VF-2): a crafted or corrupt repository must not be able to present content under an id it does
/// not hash to, with brygge then preserving that lie as the source's own identifier. Used at every site
/// that reads a commit, tree, blob, or annotated tag object — the entire object-read surface of this
/// decoder goes through here.
fn find_verified_object(repo: &gix::Repository, id: ObjectId) -> Result<gix::Object<'_>, Error> {
    let object = repo.find_object(id).map_err(read_err)?;
    let computed =
        gix::objs::compute_hash(repo.object_hash(), object.kind, &object.data).map_err(read_err)?;
    if computed != id {
        return Err(Error::Read(format!(
            "object {id} does not match its content (corrupt or crafted repository)"
        )));
    }
    Ok(object)
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
/// object; [`Error::ResourceLimit`] on a resource ceiling (RFC 010); [`Error::Ir`] if the assembled IR
/// violates an invariant.
pub fn decode(path: &Path, opts: &Options) -> Result<Ir, Error> {
    decode_with(path, opts, &Limits::default())
}

pub(crate) fn decode_with(path: &Path, opts: &Options, limits: &Limits) -> Result<Ir, Error> {
    let repo = open::open(path)?;
    open::check_repo_floor(&repo)?;

    let (refs, symbolic_ref_count, nested_tag_objects, tagger_time_counts) =
        scan_refs(&repo, limits)?;

    // CR-05: walk tips come only from carried refs (non-symbolic refs/heads/* and refs/tags/*,
    // a tag peeled to its commit). Compute this set first — before anything is read for atoms —
    // so a commit reachable only from a dropped namespace (refs/remotes/*, refs/notes/*,
    // refs/stash, ...) is never read as history. RFC 010: the commit count is bounded here, during
    // the walk, before any atom is built.
    let carried_tips: Vec<ObjectId> = refs
        .iter()
        .filter(|r| matches!(r.kind, ScannedKind::Branch(_) | ScannedKind::Tag(_)))
        .filter_map(|r| r.commit)
        .collect();
    let mut set: BTreeSet<ObjectId> = BTreeSet::new();
    if !carried_tips.is_empty() {
        for info in repo
            .rev_walk(carried_tips.clone())
            // batch-2 handoff §1.1 (review 011 R-1): the commit-graph is an unverified cache of parent
            // ids. Walking from it, not from the verified commit objects, would let a crafted
            // `objects/info/commit-graph` drop or fabricate a parent, silently altering history that
            // object-level verification alone could never see.
            .use_commit_graph(false)
            .all()
            .map_err(read_err)?
        {
            set.insert(info.map_err(read_err)?.id);
            if set.len() as u64 > limits.max_commits {
                return Err(resource_limit(
                    "the commit count",
                    format!("{} commits", limits.max_commits),
                ));
            }
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
            extras: Vec::new(),
        },
        brygge_version: decoder_version().to_string(),
        decoder: DECODER.to_string(),
        decoder_version: decoder_version().to_string(),
        params: {
            let mut params = opts.as_params();
            params.insert("floor".to_string(), floor::joined());
            params
        },
    };

    let mut builder = IrBuilder::new(provenance);
    let mut sha_to_atom: HashMap<ObjectId, AtomId> = HashMap::new();
    let mut snap_cache: HashMap<ObjectId, Snapshot> = HashMap::new();
    let mut time_counts = ParseCounts::default();

    for id in &order {
        let commit = find_verified_object(&repo, *id)?
            .try_into_commit()
            .map_err(read_err)?;

        let child_tree = commit.tree_id().map_err(read_err)?.detach();
        let child_snap = snapshot(&repo, child_tree, *id, limits, &mut snap_cache)?;

        let ps = parents.get(id).cloned().unwrap_or_default();
        let base_snap = match ps.first() {
            Some(p0) => {
                let p_commit = find_verified_object(&repo, *p0)?
                    .try_into_commit()
                    .map_err(read_err)?;
                let p_tree = p_commit.tree_id().map_err(read_err)?.detach();
                snapshot(&repo, p_tree, *p0, limits, &mut snap_cache)?
            }
            None => Snapshot::new(),
        };

        let parent_atoms: Vec<AtomId> = ps
            .iter()
            .filter_map(|p| sha_to_atom.get(p).copied())
            .collect();
        let (ops, copies) = diff_to_ops(
            &repo,
            &base_snap,
            &child_snap,
            opts,
            limits,
            &mut builder,
            parent_atoms.first().copied(),
        )?;
        let (metadata, counts) = build_metadata(&commit)?;
        time_counts.unparseable_times += counts.unparseable_times;
        time_counts.unparseable_offsets += counts.unparseable_offsets;
        time_counts.undecodable_encodings += counts.undecodable_encodings;
        let (signatures, extras) = signatures_and_extras(&commit, *id)?;

        let atom_id = builder.add_atom(AtomDraft {
            parents: parent_atoms,
            ops,
            copies,
            metadata,
            source: SourceIdentity {
                kind: SourceKind::Git,
                repo_id: repo_id.clone(),
                atom_id: id.as_bytes().to_vec(),
                signatures,
                extras,
            },
            status: EpistemicStatus::Stated,
        })?;
        sha_to_atom.insert(*id, atom_id);
    }

    // Refs, the dropped-namespace loss records, and annotated-tag identity preservation (OQ-B).
    let mut dropped_namespace_counts: BTreeMap<&'static str, u64> = BTreeMap::new();
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
                            annotation: None,
                        })?;
                    }
                    // CR-06: peeled to a tree or blob, not a commit — recorded, not silently skipped.
                    None => non_commit_refs += 1,
                }
            }
            ScannedKind::Tag(name) => match r.commit.and_then(|c| sha_to_atom.get(&c).copied()) {
                Some(target) => {
                    // An annotated tag preserves its own opaque object id + signature (PR-4/SRC-G3) as
                    // `source`, and its tagger/time/message as `annotation` (RFC 011 D-9, batch-2
                    // handoff §1.6) — nothing about it is lost any more.
                    let (source, annotation) = match &r.tag_identity {
                        Some((tag_id, signatures, annotation)) => (
                            Some(SourceIdentity {
                                kind: SourceKind::Git,
                                repo_id: repo_id.clone(),
                                atom_id: tag_id.as_bytes().to_vec(),
                                signatures: signatures.clone(),
                                extras: Vec::new(),
                            }),
                            annotation.clone(),
                        ),
                        None => (None, None),
                    };
                    builder.add_ref(RefRecord {
                        name: name.clone(),
                        kind: RefKind::Tag,
                        target,
                        status: EpistemicStatus::Stated,
                        source,
                        annotation,
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

    time_counts.unparseable_times += tagger_time_counts.unparseable_times;
    time_counts.unparseable_offsets += tagger_time_counts.unparseable_offsets;

    builder.set_loss(loss_boundary(
        &dropped_namespace_counts,
        dropped_only_commits,
        non_commit_refs,
        symbolic_ref_count,
        time_counts,
        nested_tag_objects,
    ));
    builder.finish().map_err(Error::Ir)
}

/// The result of sizing the "commits reachable only from dropped refs" loss record (CR-05).
enum DroppedOnlyCommits {
    /// The exact count.
    Counted(u64),
    /// The walk itself failed (e.g. a corrupt or missing commit in dropped-only history); the count is
    /// unknown rather than guessed. The record this becomes carries a **fixed** reason (batch-2 handoff
    /// §1.2, review 011 R-4): the underlying error's own text is not carried anywhere. A decoder library
    /// does not write to the process's stderr — that would bypass the CLI's neutralization
    /// (`crates/brygge/src/display.rs`) — so the fixed drop record is the whole diagnostic a caller
    /// gets; `git fsck` is how a human investigates the fault directly.
    Unavailable,
}

/// Count commits reachable only from `dropped_tips`, never `carried_tips` (CR-05). This never fails the
/// decode (2026-09-23 review R-2 of this handoff): any error during the walk — setup or iteration —
/// becomes [`DroppedOnlyCommits::Unavailable`] instead of propagating, since this is a loss-record count,
/// not a requirement for the decode to succeed. The underlying error's own text is discarded, never
/// printed (review 011 R-4): it is not deterministic (it can include object ids, allocator state), never
/// identity-bearing, and printing it would be this decoder's only unneutralized output.
fn count_dropped_only_commits(
    repo: &gix::Repository,
    dropped_tips: Vec<ObjectId>,
    carried_tips: Vec<ObjectId>,
) -> DroppedOnlyCommits {
    if dropped_tips.is_empty() {
        return DroppedOnlyCommits::Counted(0);
    }
    let Ok(walk) = repo
        .rev_walk(dropped_tips)
        .with_hidden(carried_tips)
        .use_commit_graph(false)
        .all()
    else {
        return DroppedOnlyCommits::Unavailable;
    };
    let mut count = 0u64;
    for info in walk {
        if info.is_err() {
            return DroppedOnlyCommits::Unavailable;
        }
        count += 1;
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
/// Returns the carried refs, a count of symbolic refs skipped (CR-16), a count of intermediate tag
/// objects hopped through in any tag-of-a-tag chain (batch-2 handoff §1.1, review 011 R-2), and how many
/// of any annotated tags' own tagger time/offset fields failed to parse (review 011 R-3a).
fn scan_refs(
    repo: &gix::Repository,
    limits: &Limits,
) -> Result<(Vec<ScannedRef>, u64, u64, ParseCounts), Error> {
    let mut out = Vec::new();
    let mut symbolic_refs = 0u64;
    let mut nested_tag_objects = 0u64;
    let mut tagger_time_counts = ParseCounts::default();
    let platform = repo.references().map_err(read_err)?;
    for r in platform.all().map_err(read_err)? {
        let mut r = r.map_err(read_err)?;
        let raw_name: Vec<u8> = r.name().as_bstr().to_vec();
        let name = raw_name.to_str_lossy(); // lossy conversion is safe here: `name` is only used for the
        // `refs/replace/` prefix check and dropped-namespace matching below, both ASCII.

        if raw_name.starts_with(b"refs/replace/") {
            return Err(Error::FloorRefusal {
                feature: floor::REPLACE_REF.to_string(),
                reason: format!(
                    "{} rewrites the object graph a reader would see; refused rather than \
                     importing the rewritten view silently",
                    escape_invalid_utf8(&raw_name)
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
                    feature: floor::NON_UTF8_REF_NAME.to_string(),
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
                // A branch carries no annotation, so a tag object it points at (via `update-ref`) has its
                // tagger/message/signature dropped: every hop is counted (review 011 F-2).
                let (commit, nested) = peel_carried(repo, direct, limits, false)?;
                nested_tag_objects += nested;
                (ScannedKind::Branch(suffix_str), commit)
            } else {
                let (identity, counts) = annotated_tag_identity(repo, direct)?;
                tag_identity = identity;
                tagger_time_counts.unparseable_times += counts.unparseable_times;
                tagger_time_counts.unparseable_offsets += counts.unparseable_offsets;
                let (commit, nested) = peel_carried(repo, direct, limits, true)?;
                nested_tag_objects += nested;
                (ScannedKind::Tag(suffix_str), commit)
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
    Ok((out, symbolic_refs, nested_tag_objects, tagger_time_counts))
}

/// Peel a `refs/heads/*` or `refs/tags/*` ref to its commit (CR-06), verifying **every** object on the
/// way — including any intermediate tag in a tag-of-a-tag chain (batch-2 handoff §1.1, review 011 R-2).
/// A ref whose target is missing entirely, or a chain longer than `limits.max_tag_chain`, is refused. A
/// chain that ends at a tree or blob gives `None` — the caller counts it as a non-commit-target ref
/// rather than silently skipping it. Returns the number of tag objects hopped through whose data is not
/// carried. For a `refs/tags/*` ref (`direct_tag_is_carried`), the outer (`direct`) tag itself is carried as
/// the ref's annotation by [`annotated_tag_identity`], so only *intermediate* tags count (an ordinary
/// annotated tag pointing at a commit has zero). A branch carries no annotation, so for it every tag hop
/// counts (review 011 F-2).
fn peel_carried(
    repo: &gix::Repository,
    direct: ObjectId,
    limits: &Limits,
    direct_tag_is_carried: bool,
) -> Result<(Option<ObjectId>, u64), Error> {
    let mut current = direct;
    let mut hops = 0usize;
    let mut nested_tags = 0u64;
    loop {
        let object = find_verified_object(repo, current)?;
        match object.kind {
            gix::objs::Kind::Commit => return Ok((Some(current), nested_tags)),
            gix::objs::Kind::Tag => {
                hops += 1;
                if hops > limits.max_tag_chain {
                    return Err(resource_limit(
                        "the tag chain",
                        format!("{} tag(s)", limits.max_tag_chain),
                    ));
                }
                if hops > 1 || !direct_tag_is_carried {
                    nested_tags += 1;
                }
                let tag = object.try_into_tag().map_err(read_err)?;
                let decoded = tag.decode().map_err(read_err)?;
                current = decoded.target();
            }
            _ => return Ok((None, nested_tags)),
        }
    }
}

/// If `direct` is an **annotated** tag object, return its id, any GPG signature (preserved opaquely,
/// `PR-4`/`SRC-G3` — the tag verifies nothing in any target, but is the cryptographic link back to the
/// source), and its tagger/time/message as an [`Annotation`] (batch-2 handoff §1.6), plus how many of the
/// tagger's time/offset fields failed to parse (review 011 R-3a — counted exactly like author/committer
/// times, never silently dropped). Returns `None` (with zero counts) for a lightweight tag, whose direct
/// target is the commit itself — one verified read decides which (review 011 R-5: no separate, unverified
/// pre-check).
fn annotated_tag_identity(
    repo: &gix::Repository,
    direct: ObjectId,
) -> Result<(Option<TagIdentity>, ParseCounts), Error> {
    let object = find_verified_object(repo, direct)?;
    if !matches!(object.kind, gix::objs::Kind::Tag) {
        return Ok((None, ParseCounts::default()));
    }
    let tag = object.try_into_tag().map_err(read_err)?;
    let decoded = tag.decode().map_err(read_err)?;
    let signatures = decoded
        .signature
        .map(|s| Signature {
            label: "gpgsig".to_string(),
            bytes: s.to_vec(),
        })
        .into_iter()
        .collect();
    let mut counts = ParseCounts::default();
    let tagger_sig = decoded.tagger().map_err(read_err)?;
    let tagger = tagger_sig.map(|sig| Identity {
        name: Text {
            bytes: sig.name.to_vec(),
            encoding: None,
        },
        email: Some(Text {
            bytes: sig.email.to_vec(),
            encoding: None,
        }),
    });
    let time = tagger_sig.and_then(|sig| {
        let seconds = strict_seconds(sig.time);
        if seconds.is_none() {
            counts.unparseable_times += 1;
        }
        seconds.map(|seconds| {
            let offset_minutes = strict_offset_minutes(sig.time);
            if offset_minutes.is_none() {
                counts.unparseable_offsets += 1;
            }
            Time {
                seconds,
                offset_minutes,
            }
        })
    });
    let message = Text {
        bytes: decoded.message.to_vec(),
        encoding: None,
    };
    let annotation = Annotation {
        tagger,
        time,
        message: Some(message),
    };
    Ok((Some((direct, signatures, Some(annotation))), counts))
}

/// Map each commit to its parents, all of which must be within `set` (batch-2 handoff §1.1, review 011
/// R-1). `set` was built by walking the *full* ancestry from the carried tips with the commit-graph
/// disabled, so every parent a verified commit actually names must also be in `set` — a shallow
/// repository or one using grafts is already refused before this runs (`open::check_repo_floor`). A
/// verified parent that the walk did not reach means the walk and the commit content disagree, which a
/// crafted or corrupt repository could otherwise use to hide or fabricate history; that is refused
/// outright rather than silently treating the commit as a root.
fn commit_parents(
    repo: &gix::Repository,
    set: &BTreeSet<ObjectId>,
) -> Result<BTreeMap<ObjectId, Vec<ObjectId>>, Error> {
    let mut parents = BTreeMap::new();
    for id in set {
        let commit = find_verified_object(repo, *id)?
            .try_into_commit()
            .map_err(read_err)?;
        let mut ps = Vec::new();
        for p in commit.parent_ids() {
            let p = p.detach();
            if !set.contains(&p) {
                return Err(Error::Read(
                    "history walk disagrees with commit content".to_string(),
                ));
            }
            ps.push(p);
        }
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
    limits: &Limits,
    cache: &mut HashMap<ObjectId, Snapshot>,
) -> Result<Snapshot, Error> {
    if let Some(s) = cache.get(&tree_id) {
        return Ok(s.clone());
    }
    let mut out = Snapshot::new();
    walk_tree(repo, tree_id, commit_id, limits, &mut out)?;
    cache.insert(tree_id, out.clone());
    Ok(out)
}

/// Walk a tree into a flat path→(oid,mode) snapshot, **iteratively** (RFC 010 CR-10): an explicit stack,
/// not recursion, so an attacker-chosen tree depth cannot overflow the process stack — which no
/// `catch_unwind` boundary (CR-16's `guard_decoder`) can contain. `max_tree_depth` and `max_path_bytes`
/// are enforced as the stack grows; the resulting snapshot is identical to a recursive walk's (the
/// traversal order differs, but `out` is a flat, order-independent map of full paths).
fn walk_tree(
    repo: &gix::Repository,
    root_tree_id: ObjectId,
    commit_id: ObjectId,
    limits: &Limits,
    out: &mut Snapshot,
) -> Result<(), Error> {
    // (tree id, path prefix built so far, this tree's own nesting depth; the root is depth 1).
    let mut stack: Vec<(ObjectId, String, usize)> = vec![(root_tree_id, String::new(), 1)];
    while let Some((tree_id, prefix, depth)) = stack.pop() {
        let tree = find_verified_object(repo, tree_id)?
            .try_into_tree()
            .map_err(read_err)?;
        for entry in tree.iter() {
            let entry = entry.map_err(read_err)?;
            let raw_name = entry.filename(); // &BStr; CR-03/D-3(i): checked below, never `to_str_lossy`.
            let Ok(name) = std::str::from_utf8(raw_name) else {
                return Err(Error::FloorRefusal {
                    feature: floor::NON_UTF8_PATH.to_string(),
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
            if path.len() > limits.max_path_bytes {
                return Err(resource_limit(
                    "a path",
                    format!("{} bytes", limits.max_path_bytes),
                ));
            }
            let mode = entry.mode();
            let oid = entry.oid().to_owned();
            match mode.kind() {
                gix::objs::tree::EntryKind::Tree => {
                    let child_depth = depth + 1;
                    if child_depth > limits.max_tree_depth {
                        return Err(resource_limit(
                            "the tree nesting",
                            format!("{} levels", limits.max_tree_depth),
                        ));
                    }
                    stack.push((oid, path, child_depth));
                }
                gix::objs::tree::EntryKind::Commit => {
                    return Err(Error::FloorRefusal {
                        feature: floor::SUBMODULE.to_string(),
                        reason: format!(
                            "submodule (gitlink) at '{path}' points outside this repository; refused \
                             rather than approximated"
                        ),
                    });
                }
                _ => {
                    // CR-03 §2.3.4: valid UTF-8 paths cannot collide at this point (git trees disallow
                    // duplicate entry names, and entry names cannot contain '/'); this is defense in
                    // depth.
                    if out.contains_key(&path) {
                        return Err(Error::Read(format!(
                            "commit {commit_id}: duplicate path '{path}' in tree snapshot"
                        )));
                    }
                    out.insert(path, (oid, u32::from(mode.value())));
                }
            }
        }
    }
    Ok(())
}

/// Diff `base` → `child` into literal path operations (all *Stated*), plus — only if
/// `opts.infer_renames` — marked *Derived* copy records for exact-content moves, each naming
/// `from_parent` (the atom's first parent, already added to `builder`) as `from_atom`. The literal
/// delete+add always remain; a copy record sits beside them, never in place of them (RFC 004 D-3).
fn diff_to_ops(
    repo: &gix::Repository,
    base: &Snapshot,
    child: &Snapshot,
    opts: &Options,
    limits: &Limits,
    builder: &mut IrBuilder,
    from_parent: Option<AtomId>,
) -> Result<(Vec<PathOp>, Vec<CopyRecord>), Error> {
    let mut ops = Vec::new();
    let mut added_by_oid: BTreeMap<ObjectId, Vec<String>> = BTreeMap::new();
    let mut deleted_by_oid: BTreeMap<ObjectId, Vec<String>> = BTreeMap::new();

    for (path, (oid, mode)) in child {
        match base.get(path) {
            None => {
                let blob = builder.add_blob(blob_bytes(repo, *oid, limits)?);
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
                    let blob = builder.add_blob(blob_bytes(repo, *oid, limits)?);
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

    let mut copies = Vec::new();
    if opts.infer_renames {
        // Exact-content moves only (RFC 004 D-3, OQ-A). A copy record is emitted **only** when a blob
        // deleted at exactly one path reappears added at exactly one path — an unambiguous 1:1 move,
        // and only when there is a parent to name as `from_atom` (a root commit's base snapshot is
        // always empty, so this never actually triggers for a root). An ambiguous many-to-many
        // identical-content case (the same bytes deleted at several paths and added at several) is left
        // unmarked: brygge does not guess which path became which. Nothing is lost by declining — the
        // literal delete+add ops remain (D-3). Similarity-based detection above exact content is OQ-A,
        // deferred until a consumer can give a threshold a fitness signal.
        if let Some(from_atom) = from_parent {
            for (oid, froms) in &deleted_by_oid {
                let [from] = froms.as_slice() else {
                    continue; // this blob was deleted at several paths — ambiguous, decline to guess
                };
                let Some([to]) = added_by_oid.get(oid).map(Vec::as_slice) else {
                    continue; // not re-added, or re-added at several paths — decline to guess
                };
                copies.push(CopyRecord {
                    from: from.clone(),
                    from_atom,
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
    Ok((ops, copies))
}

/// Read one blob's bytes, refusing before allocation if it exceeds `limits.max_blob_bytes` (RFC 010
/// CR-10) — checked from the object **header** (`find_header`, which gives the size without inflating
/// the blob), not after a full read.
fn blob_bytes(repo: &gix::Repository, oid: ObjectId, limits: &Limits) -> Result<Vec<u8>, Error> {
    let header = repo.find_header(oid).map_err(read_err)?;
    if header.size() > limits.max_blob_bytes {
        return Err(resource_limit(
            "a blob",
            format!("{} bytes", limits.max_blob_bytes),
        ));
    }
    Ok(find_verified_object(repo, oid)?
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

/// Parse the timezone-offset field of a raw signature `time` string strictly (RFC 011 D-5, batch-2
/// handoff §1.4). The token following the seconds must be a sign followed by exactly four digits
/// (`+HHMM`/`-HHMM`): `offset_minutes = sign × (HH × 60 + MM)`, and the result must fit `i16` with
/// `MM < 60`. Anything else is `None` — never gix's own lenient parser, which silently yields `+0000`
/// for a malformed offset.
fn strict_offset_minutes(raw: &str) -> Option<i16> {
    let mut tokens = raw.split_whitespace();
    let _seconds = tokens.next()?;
    let tz = tokens.next()?;
    let sign = match tz.as_bytes().first()? {
        b'+' => 1i32,
        b'-' => -1i32,
        _ => return None,
    };
    let digits = tz.get(1..)?;
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hh: i32 = digits.get(..2)?.parse().ok()?;
    let mm: i32 = digits.get(2..)?.parse().ok()?;
    if mm >= 60 {
        return None;
    }
    i16::try_from(sign * (hh * 60 + mm)).ok()
}

/// Build message/authorship claims. Names, emails and the message are carried as [`Text`] from the raw
/// bytes gix already read — no lossy conversion (RFC 011 §5: the Git row). The commit's `encoding`
/// header, when present and valid UTF-8, becomes `Text.encoding` **on the message only** (RFC 011 D-4,
/// batch-2 handoff §1.3) — names and emails keep `encoding: None`, meaning "not stated", never "UTF-8".
/// Times are parsed strictly ([`strict_seconds`], CR-16) and their offsets strictly
/// ([`strict_offset_minutes`], §1.4): an unparseable time becomes an absent claim, and an unparseable
/// offset on an otherwise-valid time becomes `offset_minutes: None` — never fabricated or salvaged
/// (NG-5). Returns the claims plus how many of the two time fields
/// failed to parse, for the caller to fold into the loss boundary.
fn build_metadata(commit: &gix::Commit<'_>) -> Result<(MetadataClaims, ParseCounts), Error> {
    let author = commit.author().map_err(read_err)?;
    let committer = commit.committer().map_err(read_err)?;
    let decoded = commit.decode().map_err(read_err)?;
    let message = decoded.message;

    let mut counts = ParseCounts::default();
    let message_encoding = match decoded.encoding {
        Some(e) => match std::str::from_utf8(e) {
            Ok(s) => Some(s.to_string()),
            Err(_) => {
                // Review 011 R-3b: a declared `encoding` header that is itself not valid UTF-8 is not
                // carried, but it is counted — never silently dropped.
                counts.undecodable_encodings += 1;
                None
            }
        },
        None => None,
    };
    let author_time = strict_seconds(author.time);
    if author_time.is_none() {
        counts.unparseable_times += 1;
    }
    let author_offset = strict_offset_minutes(author.time);
    if author_time.is_some() && author_offset.is_none() {
        counts.unparseable_offsets += 1;
    }
    let commit_time = strict_seconds(committer.time);
    if commit_time.is_none() {
        counts.unparseable_times += 1;
    }
    let commit_offset = strict_offset_minutes(committer.time);
    if commit_time.is_some() && commit_offset.is_none() {
        counts.unparseable_offsets += 1;
    }
    let raw_text = |bytes: &[u8]| Text {
        bytes: bytes.to_vec(),
        encoding: None,
    };
    Ok((
        MetadataClaims {
            author: Some(Identity {
                name: raw_text(author.name.as_ref()),
                email: Some(raw_text(author.email.as_ref())),
            }),
            author_time: author_time.map(|seconds| Time {
                seconds,
                offset_minutes: author_offset,
            }),
            committer: Some(Identity {
                name: raw_text(committer.name.as_ref()),
                email: Some(raw_text(committer.email.as_ref())),
            }),
            commit_time: commit_time.map(|seconds| Time {
                seconds,
                offset_minutes: commit_offset,
            }),
            message: Some(Text {
                bytes: message.to_vec(),
                encoding: message_encoding,
            }),
        },
        counts,
    ))
}

/// How many author/committer/tagger times, and how many of their timezone offsets (only counted when
/// the time itself parsed), failed strict parsing (batch-2 handoff §1.4); and how many commits'
/// `encoding` header value was not valid UTF-8 and so could not be carried (review 011 R-3b).
#[derive(Debug, Clone, Copy, Default)]
struct ParseCounts {
    unparseable_times: u64,
    unparseable_offsets: u64,
    undecodable_encodings: u64,
}

/// The commit's signatures (`gpgsig`/`gpgsig-sha256`, RFC 004 D-2/SRC-G3 — preserved opaquely, verifying
/// nothing in any target) and every other extra header as a labelled [`Extra`] (RFC 011 D-9, batch-2
/// handoff §1.5), both in header order. `mergetag` — a signed, embedded tag object — is carried as an
/// ordinary `Extra`, like any other header this decoder does not otherwise model; a multi-line header
/// value arrives already unfolded (continuation lines joined, Git's own convention) since gix's own
/// commit parser does this. A header **name** that is not valid UTF-8 is a floor refusal (review 011
/// R-3c): gix does not itself enforce that header names are ASCII, so a crafted commit can carry one,
/// and `Extra`/`Signature` labels are text.
fn signatures_and_extras(
    commit: &gix::Commit<'_>,
    id: ObjectId,
) -> Result<(Vec<Signature>, Vec<Extra>), Error> {
    let decoded = commit.decode().map_err(read_err)?;
    let mut signatures = Vec::new();
    let mut extras = Vec::new();
    for (name, value) in &decoded.extra_headers {
        let Ok(label) = std::str::from_utf8(name) else {
            return Err(Error::FloorRefusal {
                feature: floor::NON_UTF8_COMMIT_HEADER_NAME.to_string(),
                reason: format!(
                    "commit {id}: header name '{}' is not valid UTF-8 (invalid bytes shown as \\xNN)",
                    escape_invalid_utf8(name)
                ),
            });
        };
        match label {
            "gpgsig" | "gpgsig-sha256" => signatures.push(Signature {
                label: label.to_string(),
                bytes: value.to_vec(),
            }),
            _ => extras.push(Extra {
                label: label.to_string(),
                bytes: value.to_vec(),
            }),
        }
    }
    Ok((signatures, extras))
}

/// The Git loss boundary (RFC 004 D-5): the representation-class drops, every one class-stated, plus
/// symbolic refs skipped (CR-16), any author/committer/tagger time or timezone offset that failed to
/// parse (CR-16, batch-2 handoff §1.4), any undecodable `encoding` header (review 011 R-3b), commits
/// reachable only from a dropped ref namespace (CR-05), refs that peel to a tree or blob rather than a
/// commit (CR-06), and any intermediate tag object in a tag-of-a-tag chain (batch-2 handoff §1.1, review
/// 011 R-2) — recorded, never silently omitted (`PR-9`). An annotated tag's own tagger/time/message is
/// no longer dropped (batch-2 handoff §1.6): it is carried in the ref's `annotation`.
fn loss_boundary(
    dropped_namespace_counts: &BTreeMap<&'static str, u64>,
    dropped_only_commits: DroppedOnlyCommits,
    non_commit_refs: u64,
    symbolic_refs: u64,
    time_counts: ParseCounts,
    nested_tag_objects: u64,
) -> LossBoundary {
    let mut dropped = vec![
        DropRecord {
            class: LossClass::Representation,
            what: "packfile and delta layout, physical object store".to_string(),
            reason: "representation not assertion; reconstructible from the objects".to_string(),
        },
        DropRecord {
            class: LossClass::Representation,
            what: "index and working tree".to_string(),
            reason: "local state, not history".to_string(),
        },
        DropRecord {
            class: LossClass::Representation,
            what: "reflogs".to_string(),
            reason: "local operation log, not history".to_string(),
        },
    ];
    for (ns, count) in dropped_namespace_counts {
        dropped.push(DropRecord {
            class: LossClass::Representation,
            what: format!("{ns} refs ({count})"),
            reason: "workflow/representation refs, not authored history".to_string(),
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
                         carried ref"
                        .to_string(),
            });
        }
        DroppedOnlyCommits::Unavailable => {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: "commits reachable only from dropped refs (count unavailable)".to_string(),
                reason: "a commit in dropped-only history could not be read".to_string(),
            });
        }
    }
    if non_commit_refs > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("refs to non-commit objects ({non_commit_refs})"),
            reason: "a tag or branch naming a tree or blob; the IR's refs point at history atoms"
                .to_string(),
        });
    }
    if symbolic_refs > 0 {
        dropped.push(DropRecord {
            class: LossClass::Representation,
            what: format!("symbolic refs ({symbolic_refs})"),
            reason:
                "an alias to another ref; the IR has no alias concept; the target ref is carried \
                     on its own"
                    .to_string(),
        });
    }
    if time_counts.unparseable_times > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!(
                "unparseable author/committer/tagger times ({})",
                time_counts.unparseable_times
            ),
            reason: "the source's time field could not be parsed; the claim is absent rather than \
                     fabricated"
                .to_string(),
        });
    }
    if time_counts.unparseable_offsets > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!(
                "unparseable author/committer/tagger timezone offsets ({})",
                time_counts.unparseable_offsets
            ),
            reason: "the source's timezone offset could not be parsed strictly (`+HHMM`/`-HHMM`); \
                     the time is carried without an offset rather than a fabricated or salvaged one"
                .to_string(),
        });
    }
    if time_counts.undecodable_encodings > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!(
                "undecodable encoding headers ({})",
                time_counts.undecodable_encodings
            ),
            reason:
                "the commit's declared `encoding` header value was not valid UTF-8, so it is not \
                     carried on the message's `Text.encoding`"
                    .to_string(),
        });
    }
    if nested_tag_objects > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("nested tag objects not carried ({nested_tag_objects})"),
            reason: "an intermediate tag object in a tag-of-a-tag chain; only the outer tag's \
                     annotation is carried"
                .to_string(),
        });
    }
    LossBoundary { dropped }
}

#[cfg(test)]
mod tests;

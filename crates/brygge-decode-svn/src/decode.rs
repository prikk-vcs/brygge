//! The decode orchestration (RFC 006 D-2/D-3/D-4/D-9): parse an SVN dumpstream and build a
//! [`brygge_ir::Ir`] — a `Stated` linear revision spine with `Stated` copies, and an opt-in `Derived`
//! branch/tag layer. Builds against IR contract 0.2.0 (RFC 011).

use std::collections::{BTreeMap, HashMap, HashSet};

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, DropRecord, Flag, FlagKind, Identity, ImportProvenance, Ir, LossBoundary, LossClass,
    MetadataClaims, RefKind, RefRecord, SourceIdentity, SourceKind, Text,
};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};

use crate::layout::{self, Root};
use crate::source::svnadmin_version;
use crate::tree::{self, Tree};
use crate::{DECODER, Error, Options, Source, decoder_version, dumpstream, floor, props};

/// Decode the Subversion source into an [`Ir`].
///
/// # Errors
/// [`Error::Open`] if the source is unreadable or `svnadmin` fails; [`Error::UnsupportedFormat`] /
/// [`Error::FloorRefusal`] for a refused format or feature; [`Error::Read`] on a malformed dumpstream;
/// [`Error::Ir`] on an IR invariant violation.
pub fn decode(source: &Source, opts: &Options) -> Result<Ir, Error> {
    let bytes = source.load()?;
    let dump = dumpstream::parse_dump(&bytes)?;

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
            // CR-07.6: the source form, and (for a live repository) the svnadmin version that dumped it —
            // `verify --against-source` needs both to tell "cannot be compared" from "does not match".
            match source {
                Source::DumpFile(_) => {
                    params.insert("source_form".to_string(), "dumpfile".to_string());
                }
                Source::LocalRepo(_) => {
                    params.insert("source_form".to_string(), "svnadmin-dump".to_string());
                    // Review 010 R-5: never silently absent. A live decode already requires `svnadmin`
                    // to exist (for the dump itself), so a failed `--version` call is a real `Open`
                    // failure, not a reason to ship an artifact whose form promises a param it lacks.
                    params.insert("svnadmin_version".to_string(), svnadmin_version()?);
                }
            }
            params
        },
    };
    let mut builder = IrBuilder::new(provenance);
    let mut loss = Loss::default();

    if dump.revisions.is_empty() {
        builder.set_loss(loss.into_boundary());
        return builder.finish().map_err(Error::Ir);
    }

    // RFC 010 increment 1 — bound snapshot retention. A `copyfrom` may name any past revision, so a
    // first pass collects exactly which revisions are referenced; the main pass keeps a tree snapshot only
    // for those (plus the rolling previous tree, always needed as the next revision's base). This turns
    // O(revisions × tree) scratch into O(copy-targets × tree) — copy targets are branch/tag creation
    // points, typically far fewer than revisions — with no change to the atoms produced or their order.
    let mut needed: HashSet<u64> = HashSet::new();
    for rev in &dump.revisions {
        for node in &rev.nodes {
            if let Some((crev, _)) = &node.copyfrom {
                needed.insert(*crev);
            }
        }
    }

    let mut kept: HashMap<u64, Tree> = HashMap::new();
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    let mut prev_tree = Tree::new();
    let mut prev_atom: Option<AtomId> = None;
    let mut heads: BTreeMap<String, (Root, AtomId)> = BTreeMap::new();

    for rev in dump.revisions {
        let metadata = metadata_from_props(&rev.props, &mut loss.unparseable_dates);
        let number = rev.number;
        let touched: Vec<Root> = if opts.reconstruct_refs {
            rev.nodes
                .iter()
                .filter_map(|n| layout::classify(&n.path, &opts.layout))
                .collect()
        } else {
            Vec::new()
        };

        let applied =
            tree::apply_revision(&prev_tree, rev.nodes, &kept, &revnum_to_atom, &mut builder)?;

        loss.mergeinfo |= applied.prop_loss.mergeinfo;
        loss.workflow |= applied.prop_loss.workflow;
        loss.custom |= applied.prop_loss.custom;
        loss.empty_dirs = loss.empty_dirs.saturating_add(applied.empty_dirs);

        let parents: Vec<AtomId> = prev_atom.into_iter().collect();
        let atom_id = builder.add_atom(AtomDraft {
            parents,
            ops: applied.ops,
            copies: applied.copies,
            metadata,
            source: SourceIdentity {
                kind: SourceKind::Svn,
                repo_id: repo_id.clone(),
                atom_id: number.to_be_bytes().to_vec(),
                signatures: Vec::new(),
                extras: Vec::new(),
            },
            status: EpistemicStatus::Stated,
        })?;
        prev_atom = Some(atom_id);
        revnum_to_atom.insert(number, atom_id);

        for root in touched {
            heads.insert(root.prefix.clone(), (root, atom_id));
        }

        // Retain this revision's tree only if a later copyfrom names it; always carry it forward as `prev`.
        if needed.contains(&number) {
            kept.insert(number, applied.tree.clone());
        }
        prev_tree = applied.tree;
    }

    if opts.reconstruct_refs {
        reconstruct_refs(&heads, &prev_tree, opts, &mut builder, &mut loss)?;
        // Review 010 R-2: one condition, one flag. The partial-layout flag fires only when the layout
        // *was* found at all — when it was not, `reconstruct_refs` above already raised the
        // whole-layout-not-found flag, and every path in the tree is trivially "outside" a layout that
        // was never located, which would otherwise double-flag the same condition.
        if !heads.is_empty() {
            layout_honesty(&prev_tree, opts, &mut builder);
        }
    }

    builder.set_loss(loss.into_boundary());
    builder.finish().map_err(Error::Ir)
}

/// Emit the opt-in `Derived` branch/tag refs, or record a loud convention-violation if the layout was not
/// found at all (RFC 006 D-4/OQ-B; RFC 011 §5 — a typed [`Flag`], not a magic-string drop record).
/// **Live refs only** (CR-07.3): a root that was touched at some point but no longer has any file under
/// it in the final tree — deleted, or moved to a new name and so present under a *different* prefix — is
/// not emitted; it is counted as a genuine drop instead (`loss.deleted_or_moved_roots`).
fn reconstruct_refs(
    heads: &BTreeMap<String, (Root, AtomId)>,
    final_tree: &Tree,
    opts: &Options,
    builder: &mut IrBuilder,
    loss: &mut Loss,
) -> Result<(), Error> {
    if heads.is_empty() {
        builder.add_flag(Flag {
            kind: FlagKind::ConventionViolation,
            what: "trunk/branches/tags layout not found".to_string(),
            count: 1,
            reason: "ref reconstruction was requested but the repository does not follow the \
                     convention; no branch or tag ref was fabricated"
                .to_string(),
        });
        return Ok(());
    }
    let live: Vec<&(Root, AtomId)> = heads
        .values()
        .filter(|(root, _)| tree::is_live_under(final_tree, &root.prefix))
        .collect();
    loss.deleted_or_moved_roots = heads.len().saturating_sub(live.len());
    for (root, target) in live {
        let mut params = BTreeMap::new();
        params.insert("layout".to_string(), opts.layout.label());
        if root.kind == RefKind::Tag {
            params.insert(
                "immutability".to_string(),
                "not-guaranteed: an SVN tag is an ordinary directory and may carry post-creation \
                 commits"
                    .to_string(),
            );
        }
        let derivation = Derivation {
            kind: DerivationKind::ReconstructedBranch,
            by: DECODER.to_string(),
            decoder_version: decoder_version().to_string(),
            params,
            confidence: None,
        };
        builder.add_ref(RefRecord {
            name: root.name.clone(),
            kind: root.kind.clone(),
            target: *target,
            status: EpistemicStatus::Derived(derivation),
            source: None,
            annotation: None,
        })?;
    }
    Ok(())
}

/// Count files in the final tree that live outside every root the layout recognizes, and flag it
/// (CR-07.5): a repository that only partly follows the convention still leaves paths belonging to no
/// reconstructed branch or tag, which deserves the same loud attention as the whole-layout-not-found case
/// — but is a distinct condition (the layout *was* found; it just does not cover everything).
fn layout_honesty(final_tree: &Tree, opts: &Options, builder: &mut IrBuilder) {
    let outside = final_tree
        .keys()
        .filter(|path| layout::classify(path, &opts.layout).is_none())
        .count();
    if outside > 0 {
        builder.add_flag(Flag {
            kind: FlagKind::ConventionViolation,
            what: format!("paths outside the trunk/branches/tags layout ({outside})"),
            count: outside as u64,
            reason: "the repository partly follows the layout; these paths belong to no \
                     reconstructed branch or tag"
                .to_string(),
        });
    }
}

/// Extract a revision's metadata claims from its properties (`PR-3`, CR-04). `svn:author` may be absent
/// (anonymous); author and log are carried byte-exact; `svn:date` that will not parse yields no time rather than a wrong one, counted in
/// `unparseable_dates`. `svn:date` is UTC by definition, so a parsed time's offset is always `Some(0)`.
/// Committer and commit time are never stated by SVN and stay absent (the one-claim rule, RFC 011 D-5).
fn metadata_from_props(
    rev_props: &[(String, Vec<u8>)],
    unparseable_dates: &mut usize,
) -> MetadataClaims {
    // Author and log are carried as the bytes the dump held, whatever they are: Subversion validates
    // them as UTF-8, but a repository loaded with `--bypass-prop-validation` or converted by an old tool
    // can hold other bytes, and the IR's text is bytes, so nothing is lost and nothing is refused. An
    // empty author is still an absent claim (an anonymous commit), as before.
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

/// Accumulated loss categories across the import (RFC 006 D-5), rendered into the loss boundary at the end.
#[derive(Debug, Default)]
struct Loss {
    mergeinfo: bool,
    workflow: bool,
    custom: bool,
    empty_dirs: usize,
    /// Branch/tag roots that were touched at some point but no longer exist in the final tree
    /// (deleted, or moved away) — CR-07.3. A genuine drop: nothing represents that history's ref.
    deleted_or_moved_roots: usize,
    /// `svn:date` values present but not parseable (CR-04).
    unparseable_dates: usize,
}

impl Loss {
    fn into_boundary(self) -> LossBoundary {
        let mut dropped = vec![DropRecord {
            class: LossClass::Representation,
            what: "SVN physical storage and dumpstream framing".to_string(),
            reason: "representation not assertion; the logical revisions are preserved".to_string(),
        }];
        if self.mergeinfo {
            dropped.push(DropRecord {
                class: LossClass::AdvisoryUnreliable,
                what: "svn:mergeinfo".to_string(),
                reason:
                    "advisory merge tracking, frequently incomplete; never promoted to a merge \
                         parent"
                        .to_string(),
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
            });
        }
        if self.deleted_or_moved_roots > 0 {
            dropped.push(DropRecord {
                class: LossClass::Other,
                what: format!(
                    "deleted or moved branches/tags not represented ({})",
                    self.deleted_or_moved_roots
                ),
                reason:
                    "the IR's refs name live history; a deleted, moved or never-filled SVN branch \
                         or tag has no live head"
                        .to_string(),
            });
        }
        if self.unparseable_dates > 0 {
            dropped.push(DropRecord {
                class: LossClass::Other,
                what: format!("unparseable svn:date ({})", self.unparseable_dates),
                reason: "svn:date did not parse as an SVN-format timestamp; no time is carried \
                         rather than a wrong one"
                    .to_string(),
            });
        }
        LossBoundary { dropped }
    }
}

#[cfg(test)]
mod tests;

//! The decode orchestration (RFC 006 D-2/D-3/D-4/D-9): parse an SVN dumpstream and build a
//! [`brygge_ir::Ir`] — a `Stated` linear revision spine with `Stated` copies, and an opt-in `Derived`
//! branch/tag layer. Builds against the frozen IR 1.0.0 with no new field or variant (D-9).

use std::collections::{BTreeMap, HashMap, HashSet};

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, DropRecord, Identity, ImportProvenance, Ir, LossBoundary, LossClass, MetadataClaims,
    RefKind, RefRecord, SourceIdentity, SourceKind,
};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};

use crate::layout::{self, Root};
use crate::tree::{self, Tree};
use crate::{DECODER, Error, Options, Source, decoder_version, dumpstream, props};

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
        },
        brygge_version: decoder_version().to_string(),
        decoder: DECODER.to_string(),
        decoder_version: decoder_version().to_string(),
        params: opts.as_params(),
        import_time: None,
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
    let mut prev_tree = Tree::new();
    let mut prev_atom: Option<AtomId> = None;
    let mut heads: BTreeMap<String, (Root, AtomId)> = BTreeMap::new();

    for rev in dump.revisions {
        let metadata = metadata_from_props(&rev.props);
        let number = rev.number;
        let touched: Vec<Root> = if opts.reconstruct_refs {
            rev.nodes
                .iter()
                .filter_map(|n| layout::classify(&n.path, &opts.layout))
                .collect()
        } else {
            Vec::new()
        };

        let applied = tree::apply_revision(&prev_tree, rev.nodes, &kept, &mut builder)?;

        loss.mergeinfo |= applied.prop_loss.mergeinfo;
        loss.workflow |= applied.prop_loss.workflow;
        loss.custom |= applied.prop_loss.custom;
        loss.empty_dirs = loss.empty_dirs.saturating_add(applied.empty_dirs);

        let parents: Vec<AtomId> = prev_atom.into_iter().collect();
        let atom_id = builder.add_atom(AtomDraft {
            parents,
            ops: applied.ops,
            rename_hints: applied.hints,
            metadata,
            source: SourceIdentity {
                kind: SourceKind::Svn,
                repo_id: repo_id.clone(),
                atom_id: number.to_be_bytes().to_vec(),
                signatures: Vec::new(),
            },
            status: EpistemicStatus::Stated,
        });
        prev_atom = Some(atom_id);

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
        reconstruct_refs(&heads, opts, &mut builder, &mut loss)?;
    }

    builder.set_loss(loss.into_boundary());
    builder.finish().map_err(Error::Ir)
}

/// Emit the opt-in `Derived` branch/tag refs, or record a loud convention-violation if the layout was not
/// found at all (RFC 006 D-4/OQ-B).
fn reconstruct_refs(
    heads: &BTreeMap<String, (Root, AtomId)>,
    opts: &Options,
    builder: &mut IrBuilder,
    loss: &mut Loss,
) -> Result<(), Error> {
    if heads.is_empty() {
        loss.layout_unmatched = true;
        return Ok(());
    }
    for (root, target) in heads.values() {
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
        })?;
    }
    Ok(())
}

/// Extract a revision's metadata claims from its properties (`PR-3`). `svn:author` may be absent
/// (anonymous); `svn:date` that will not parse yields no time rather than a wrong one.
fn metadata_from_props(rev_props: &[(String, Vec<u8>)]) -> MetadataClaims {
    let author = props::get_str(rev_props, props::SVN_AUTHOR)
        .filter(|s| !s.is_empty())
        .map(|name| Identity {
            name,
            email: String::new(),
        });
    let message = props::get_str(rev_props, props::SVN_LOG);
    let time = props::get_str(rev_props, props::SVN_DATE)
        .as_deref()
        .and_then(props::parse_svn_date);
    MetadataClaims {
        author: author.clone(),
        committer: author,
        message,
        author_time: time,
        commit_time: time,
    }
}

/// Accumulated loss categories across the import (RFC 006 D-5), rendered into the loss boundary at the end.
#[derive(Debug, Default)]
struct Loss {
    mergeinfo: bool,
    workflow: bool,
    custom: bool,
    empty_dirs: usize,
    layout_unmatched: bool,
}

impl Loss {
    fn into_boundary(self) -> LossBoundary {
        let mut dropped = vec![DropRecord {
            class: LossClass::Representation,
            what: "SVN physical storage and dumpstream framing".to_string(),
            reason: "representation not assertion; the logical revisions are preserved (PR-7)"
                .to_string(),
        }];
        if self.mergeinfo {
            dropped.push(DropRecord {
                class: LossClass::AdvisoryUnreliable,
                what: "svn:mergeinfo".to_string(),
                reason:
                    "advisory merge tracking, frequently incomplete; never promoted to a merge \
                         parent (PR-8, SRC-S2)"
                        .to_string(),
            });
        }
        if self.workflow {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: "svn:eol-style / svn:keywords / svn:ignore (working-copy hints)".to_string(),
                reason:
                    "the stored normal-form bytes are carried verbatim; keyword/EOL expansion is \
                         a working-copy transform, not history (NG-5, PR-7)"
                        .to_string(),
            });
        }
        if self.custom {
            dropped.push(DropRecord {
                class: LossClass::Other,
                what: "custom (user-defined) properties".to_string(),
                reason: "not carried this increment (no consumer; the frozen IR is not grown \
                         speculatively — RFC 006 OQ-D)"
                    .to_string(),
            });
        }
        if self.empty_dirs > 0 {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: format!("empty directories ({})", self.empty_dirs),
                reason:
                    "the IR has no empty-directory entity (as Git/prikk); the directory carried \
                         no file (RFC 006 D-6)"
                        .to_string(),
            });
        }
        if self.layout_unmatched {
            dropped.push(DropRecord {
                class: LossClass::Other,
                what: crate::LAYOUT_UNMATCHED.to_string(),
                reason: "ref reconstruction was requested but the repository does not follow the \
                         convention; no branch/tag ref was fabricated (RFC 006 OQ-B, the loud \
                         convention-violation record)"
                    .to_string(),
            });
        }
        LossBoundary { dropped }
    }
}

#[cfg(test)]
mod tests;

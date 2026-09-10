//! The decode orchestration (RFC 007 D-2/D-3/D-4/D-9): read a CVS repository's RCS `,v` files, reconstruct
//! changesets, and build a [`brygge_ir::Ir`] whose changeset atoms are **`Derived(ReconstructedChangeset)`**
//! with a faithful per-file op spine. Builds against the frozen IR 1.0.0 with no new field or variant (D-9).

use std::collections::{BTreeMap, HashMap, HashSet};

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, DropRecord, Identity, ImportProvenance, Ir, LossBoundary, LossClass, MetadataClaims,
    PathOp, RefKind, RefRecord, SourceIdentity, SourceKind,
};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};

use crate::cluster::{self, Changeset, FileRev};
use crate::{DECODER, Error, Options, Source, UNDER_FLOOR, decoder_version, scan, symbols};

/// The IR mode for a CVS file (CVS/RCS carries no Unix exec bit; binary `-kb` is content, not mode).
const MODE_REGULAR: u32 = 0o100_644;

/// Decode the CVS repository into an [`Ir`].
///
/// # Errors
/// [`Error::Open`]/[`Error::FloorRefusal`] for a bad or remote source or a whole-import-under-floor;
/// [`Error::Read`] on a malformed `,v`; [`Error::Ir`] on an IR invariant violation.
pub fn decode(source: &Source, opts: &Options) -> Result<Ir, Error> {
    let root = source.resolve()?;
    let files = scan::scan(root)?;

    let repo_id = root.to_string_lossy().as_bytes().to_vec();
    let provenance = ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Cvs,
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

    // Gather every per-file revision, reconstructing content (except for `dead` deletions).
    let mut filerevs: Vec<FileRev> = Vec::new();
    let has_expand = files.iter().any(|f| f.rcs.expand.is_some());
    for f in &files {
        for (num, rev) in &f.rcs.revisions {
            let dead = rev.state == "dead";
            let content = if dead {
                Vec::new()
            } else {
                f.rcs.content_of(num)?
            };
            filerevs.push(FileRev {
                path: f.path.clone(),
                rev: num.clone(),
                date: rev.date,
                author: rev.author.clone(),
                log: rev.log.clone(),
                state: rev.state.clone(),
                content,
            });
        }
    }

    let changesets = cluster::reconstruct(filerevs, opts.window_secs);

    // Whole-import floor: if nothing reconstructs confidently, refuse rather than import all-uncertain (OQ-B).
    if !changesets.is_empty()
        && !changesets
            .iter()
            .any(|c| c.confidence >= opts.confidence_floor)
    {
        return Err(Error::FloorRefusal {
            feature: "whole-import-under-confidence-floor".to_string(),
            reason: format!(
                "no reconstructed changeset reached the confidence floor ({}); the history is too \
                 ambiguous to import as changesets (RFC 007 OQ-B, SRC-C3)",
                opts.confidence_floor
            ),
        });
    }

    let mut live: HashSet<String> = HashSet::new();
    let mut prev_atom: Option<AtomId> = None;
    // (path, rev-dotted) -> (atom, changeset date), for symbol resolution.
    let mut rev_to_atom: HashMap<(String, String), (AtomId, i64)> = HashMap::new();
    let mut loss = Loss {
        has_expand,
        ..Loss::default()
    };

    for cs in &changesets {
        let ops = ops_for(cs, &mut live, &mut builder);
        let atom_id = builder.add_atom(AtomDraft {
            parents: prev_atom.into_iter().collect(),
            ops,
            rename_hints: Vec::new(), // CVS has no rename (OQ-C)
            metadata: metadata_of(cs),
            source: SourceIdentity {
                kind: SourceKind::Cvs,
                repo_id: repo_id.clone(),
                atom_id: changeset_atom_id(cs),
                signatures: Vec::new(),
            },
            status: derived_status(cs, opts),
        });
        prev_atom = Some(atom_id);

        for fr in &cs.revs {
            rev_to_atom.insert((fr.path.clone(), fr.rev.to_dotted()), (atom_id, cs.date));
        }
        if cs.confidence < opts.confidence_floor {
            loss.low_confidence += 1;
        }
    }

    if opts.reconstruct_refs {
        add_refs(&files, &rev_to_atom, &mut builder)?;
    }

    builder.set_loss(loss.into_boundary());
    builder.finish().map_err(Error::Ir)
}

/// Determine a changeset's path operations against the running live-path set.
fn ops_for(cs: &Changeset, live: &mut HashSet<String>, builder: &mut IrBuilder) -> Vec<PathOp> {
    let mut ops = Vec::new();
    for fr in &cs.revs {
        if fr.is_dead() {
            if live.remove(&fr.path) {
                ops.push(PathOp::Delete {
                    path: fr.path.clone(),
                    status: EpistemicStatus::Stated,
                });
            }
            // A dead revision for a path that was never live is a no-op (created-then-deleted off-mainline).
        } else {
            let blob = builder.add_blob(fr.content.clone());
            if live.insert(fr.path.clone()) {
                ops.push(PathOp::Add {
                    path: fr.path.clone(),
                    blob,
                    mode: MODE_REGULAR,
                    status: EpistemicStatus::Stated,
                });
            } else {
                ops.push(PathOp::Modify {
                    path: fr.path.clone(),
                    blob,
                    mode: MODE_REGULAR,
                    status: EpistemicStatus::Stated,
                });
            }
        }
    }
    ops
}

fn metadata_of(cs: &Changeset) -> MetadataClaims {
    let author = if cs.author.is_empty() {
        None
    } else {
        Some(Identity {
            name: cs.author.clone(),
            email: String::new(),
        })
    };
    let message = if cs.log.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&cs.log).into_owned())
    };
    MetadataClaims {
        author: author.clone(),
        committer: author,
        message,
        author_time: Some(cs.date),
        commit_time: Some(cs.date),
    }
}

/// The `Derived(ReconstructedChangeset)` status carrying the clustering parameters and confidence.
fn derived_status(cs: &Changeset, opts: &Options) -> EpistemicStatus {
    let mut params = BTreeMap::new();
    params.insert("window_secs".to_string(), opts.window_secs.to_string());
    params.insert("cluster_keys".to_string(), "author,log".to_string());
    EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::ReconstructedChangeset,
        by: DECODER.to_string(),
        decoder_version: decoder_version().to_string(),
        params,
        confidence: Some(cs.confidence),
    })
}

/// The changeset's opaque source id: the canonical, sorted set of source-native `(path@rev)` pairs it
/// groups (PR-4/D-9/OQ-D). The only identity a reconstructed changeset has.
fn changeset_atom_id(cs: &Changeset) -> Vec<u8> {
    let mut pairs: Vec<String> = cs
        .revs
        .iter()
        .map(|r| format!("{}@{}", r.path, r.rev.to_dotted()))
        .collect();
    pairs.sort();
    pairs.join("\n").into_bytes()
}

/// Reconstruct `Derived` tag/branch refs from per-file symbolic names (opt-in, RFC 007 D-5).
fn add_refs(
    files: &[scan::CvsFile],
    rev_to_atom: &HashMap<(String, String), (AtomId, i64)>,
    builder: &mut IrBuilder,
) -> Result<(), Error> {
    for sym in symbols::collect(files) {
        // Target the changeset of the latest-dated revision the symbol names.
        let mut best: Option<(AtomId, i64)> = None;
        for (path, rev) in &sym.named {
            if let Some((atom, date)) = rev_to_atom.get(&(path.clone(), rev.to_dotted())) {
                if best.map(|(_, d)| *date > d).unwrap_or(true) {
                    best = Some((*atom, *date));
                }
            }
        }
        let Some((target, _)) = best else {
            continue; // no named revision maps to a reconstructed changeset — skip this ref
        };
        let mut params = BTreeMap::new();
        params.insert("source".to_string(), "cvs-symbol".to_string());
        let kind = if sym.is_branch {
            RefKind::Branch
        } else {
            params.insert(
                "straddle".to_string(),
                "a CVS tag is per-file and may name revisions from different reconstructed changesets"
                    .to_string(),
            );
            RefKind::Tag
        };
        builder.add_ref(RefRecord {
            name: sym.name,
            kind,
            target,
            status: EpistemicStatus::Derived(Derivation {
                kind: DerivationKind::ReconstructedBranch,
                by: DECODER.to_string(),
                decoder_version: decoder_version().to_string(),
                params,
                confidence: None,
            }),
            source: None,
        })?;
    }
    Ok(())
}

/// Accumulated loss categories (RFC 007 D-6), rendered into the loss boundary at the end.
#[derive(Debug, Default)]
struct Loss {
    has_expand: bool,
    low_confidence: usize,
}

impl Loss {
    fn into_boundary(self) -> LossBoundary {
        let mut dropped = vec![
            DropRecord {
                class: LossClass::Representation,
                what: "RCS ,v physical layout and delta encoding".to_string(),
                reason: "representation not assertion; the logical revisions are preserved (PR-7)"
                    .to_string(),
            },
            DropRecord {
                class: LossClass::Representation,
                what: "CVSROOT administrative files, locks, and working-copy state".to_string(),
                reason: "configuration and local state, not history (PR-7)".to_string(),
            },
        ];
        if self.has_expand {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: "RCS keyword expansion / -kb text translation".to_string(),
                reason: "a working-copy transform; the stored (unexpanded) bytes are carried \
                         verbatim (NG-5)"
                    .to_string(),
            });
        }
        if self.low_confidence > 0 {
            dropped.push(DropRecord {
                class: LossClass::Other,
                what: UNDER_FLOOR.to_string(),
                reason: format!(
                    "{} reconstructed changeset(s) scored below the confidence floor and are imported \
                     but flagged as low-confidence judgments (RFC 007 OQ-B, SRC-C2/C3)",
                    self.low_confidence
                ),
            });
        }
        LossBoundary { dropped }
    }
}

#[cfg(test)]
mod tests;

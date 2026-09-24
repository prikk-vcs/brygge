//! The decode orchestration (RFC 007 D-2/D-3/D-4/D-9, owner ruling D-2): read a CVS repository's RCS
//! `,v` files, keep only its **main line** (trunk, plus a vendor branch while one is set — corrections
//! handoff §2.1), reconstruct changesets from it, and build a [`brygge_ir::Ir`] whose changeset atoms are
//! **`Derived(ReconstructedChangeset)`** with a faithful per-file op spine. Builds against IR contract
//! 0.2.0 (RFC 011), with no new field or variant beyond what that contract already defines (D-9).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use sha2::{Digest, Sha256};

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, DropRecord, Flag, FlagKind, Identity, ImportProvenance, Ir, LossBoundary, LossClass,
    MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity, SourceKind, Text, Time,
};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};

use crate::cluster::{self, Changeset, FileRev};
use crate::mainline;
use crate::rcs::{RcsFile, RevNum};
use crate::{DECODER, Error, Options, Source, decoder_version, floor, scan, symbols};

/// The IR mode for a CVS file (CVS/RCS carries no Unix exec bit; binary `-kb` is content, not mode).
const MODE_REGULAR: u32 = 0o100_644;

/// Decode the CVS repository into an [`Ir`].
///
/// # Errors
/// [`Error::Open`]/[`Error::FloorRefusal`] for a bad or remote source, a repository shape violation, or
/// a whole-import-under-floor; [`Error::Read`] on a malformed `,v`; [`Error::Ir`] on an IR invariant
/// violation.
pub fn decode(source: &Source, opts: &Options) -> Result<Ir, Error> {
    let root = source.resolve()?;
    let files = scan::scan(root)?;

    let repo_id = compute_repo_id(&files)?;
    let provenance = ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Cvs,
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

    // Gather every MAIN-LINE per-file revision (owner ruling D-2), reconstructing content (except for
    // `dead` deletions). Branch revisions are counted, never imported, until branch-aware threading
    // lands (0.3.0).
    let mut filerevs: Vec<FileRev> = Vec::new();
    let has_expand = files.iter().any(|f| f.rcs.expand.is_some());
    let mut branches = BranchStats::default();
    for f in &files {
        check_no_later_trunk_after_vendor_branch(f)?;
        let vendor_branch = f.rcs.branch.clone();
        // RFC 010 increment 3: every main-line content this file needs, in one pass over its delta chains.
        let mut contents = FileContents::new(f, vendor_branch.as_ref());
        let skip_branch_point = vendor_import_branch_point(f, &contents)?;
        for (num, rev) in &f.rcs.revisions {
            if Some(num) == skip_branch_point.as_ref() {
                // cvs import's branch-point revision, identical to the vendor branch's first revision
                // and same-dated: contributes no separate op (avoids a spurious no-op Modify).
                continue;
            }
            if mainline::is_main_line(num, vendor_branch.as_ref()) {
                let dead = rev.state == "dead";
                let content = if dead {
                    Vec::new()
                } else {
                    contents.take(&f.rcs, num)?
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
            } else {
                branches.excluded_revisions += 1;
                let branch_id = mainline::branch_of(num);
                match mainline::branch_symbol_name(&f.rcs, &branch_id) {
                    Some(name) => {
                        branches.named.insert(name.to_string());
                    }
                    None => branches.unnamed_revisions += 1,
                }
            }
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
            feature: floor::WHOLE_IMPORT_UNDER_CONFIDENCE_FLOOR.to_string(),
            reason: format!(
                "no reconstructed changeset reached the confidence floor ({}); the history is too \
                 ambiguous to import as changesets",
                opts.confidence_floor
            ),
        });
    }

    let mut live: HashSet<String> = HashSet::new();
    let mut prev_atom: Option<AtomId> = None;
    // (path, rev-dotted) -> (atom, changeset date), for symbol resolution.
    let mut rev_to_atom: HashMap<(String, String), (AtomId, i64)> = HashMap::new();
    let loss = Loss { has_expand };
    // Tracked separately from `Loss`: no changeset is dropped for being low-confidence — it is imported
    // and flagged (RFC 011 D-8). One fact, one place.
    let mut low_confidence: usize = 0;

    for cs in &changesets {
        let ops = ops_for(cs, &mut live, &mut builder);
        let atom_id = builder
            .add_atom(AtomDraft {
                parents: prev_atom.into_iter().collect(),
                ops,
                copies: Vec::new(), // CVS has no rename (OQ-C)
                metadata: metadata_of(cs),
                source: SourceIdentity {
                    kind: SourceKind::Cvs,
                    repo_id: repo_id.clone(),
                    atom_id: changeset_atom_id(cs),
                    signatures: Vec::new(),
                    extras: Vec::new(),
                },
                status: derived_status(cs, opts),
            })
            .map_err(Error::Ir)?;
        prev_atom = Some(atom_id);

        for fr in &cs.revs {
            rev_to_atom.insert((fr.path.clone(), fr.rev.to_dotted()), (atom_id, cs.date));
        }
        if cs.confidence < opts.confidence_floor {
            low_confidence += 1;
        }
    }

    let mut ref_stats = RefStats::default();
    if opts.reconstruct_refs {
        ref_stats = add_refs(&files, &rev_to_atom, &mut builder)?;
    }

    const BRANCH_REASON: &str = "brygge imports the CVS main line only; branch history is \
                                  planned for a later release (0.3.0). Keep the source repository.";
    let mut boundary = loss.into_boundary();
    if branches.excluded_revisions > 0 {
        boundary.dropped.push(DropRecord {
            class: LossClass::Other,
            what: branches.what(),
            reason: BRANCH_REASON.to_string(),
        });
    }
    if ref_stats.branch_symbols_not_reconstructed > 0 {
        boundary.dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!(
                "CVS branch symbols not reconstructed ({})",
                ref_stats.branch_symbols_not_reconstructed
            ),
            reason: BRANCH_REASON.to_string(),
        });
    }
    if ref_stats.tags_not_reconstructed > 0 {
        // Review 008 R-1: a tag naming no main-line revision (a vendor release tag once the vendor
        // branch is cleared, or a tag on a release branch) is common under main-line-only import, and
        // it is never skipped silently.
        boundary.dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!(
                "CVS tags on branch revisions not reconstructed ({})",
                ref_stats.tags_not_reconstructed
            ),
            reason: BRANCH_REASON.to_string(),
        });
    }
    builder.set_loss(boundary);

    if low_confidence > 0 {
        builder.add_flag(Flag {
            kind: FlagKind::BelowConfidenceFloor,
            what: "reconstruction confidence below the floor".to_string(),
            count: low_confidence as u64,
            reason: format!(
                "{low_confidence} reconstructed changeset(s) scored below the confidence floor and are \
                 imported but flagged as low-confidence judgments"
            ),
        });
    }
    builder.finish().map_err(Error::Ir)
}

/// Tallies of excluded branch revisions, for the single drop record's wording (corrections handoff §2.1).
#[derive(Debug, Default)]
struct BranchStats {
    excluded_revisions: usize,
    /// Distinct branch *symbol names* touched by excluded revisions, across all files.
    named: BTreeSet<String>,
    /// Excluded revisions on a branch with no symbol in their file.
    unnamed_revisions: usize,
}

impl BranchStats {
    fn what(&self) -> String {
        let n = self.excluded_revisions;
        let m = self.named.len();
        if self.unnamed_revisions > 0 {
            format!(
                "CVS branch revisions not imported ({n} revision(s) on {m} named branch(es) and unnamed branch revisions)"
            )
        } else {
            format!("CVS branch revisions not imported ({n} revision(s) on {m} branch(es))")
        }
    }
}

/// The vendor-import exception (corrections handoff §2.1): if the vendor branch's first revision has
/// content identical to its branch-point revision and the same date — the shape `cvs import` produces —
/// the branch-point revision contributes no separate op, avoiding a spurious no-op `Modify`. Returns the
/// branch-point revision to skip, if the exception applies.
fn vendor_import_branch_point(
    f: &scan::CvsFile,
    contents: &FileContents,
) -> Result<Option<RevNum>, Error> {
    let Some(vb) = &f.rcs.branch else {
        return Ok(None);
    };
    let Some(first) = mainline::vendor_branch_first_revision(&f.rcs, vb) else {
        return Ok(None);
    };
    let bp = mainline::branch_point(vb);
    let (Some(bp_rev), Some(first_rev)) = (f.rcs.revisions.get(&bp), f.rcs.revisions.get(&first))
    else {
        return Ok(None);
    };
    if bp_rev.date != first_rev.date {
        return Ok(None);
    }
    if !contents.same(&f.rcs, &bp, &first)? {
        return Ok(None);
    }
    Ok(Some(bp))
}

/// The reconstructed content of one file's main-line revisions, from **one pass** over its delta chains
/// ([`RcsFile::contents_of_many`], RFC 010 increment 3) instead of one walk from `head` per revision.
///
/// Holds exactly the revisions the loop in [`decode`] will ask for (the non-dead main-line ones), plus, for a
/// `cvs import`, the branch point and the vendor branch's first revision that [`vendor_import_branch_point`]
/// compares. **If the pass fails for any reason** (a malformed `,v`: an unreachable revision, a bad delta, a
/// chain too long), nothing is held and every request falls back to per-revision reconstruction, so the
/// error a malformed file gets is exactly the one it always got, from the same revision.
struct FileContents {
    many: Option<BTreeMap<RevNum, Vec<u8>>>,
}

impl FileContents {
    fn new(f: &scan::CvsFile, vendor_branch: Option<&RevNum>) -> Self {
        let mut wanted: BTreeSet<RevNum> = f
            .rcs
            .revisions
            .iter()
            .filter(|(num, rev)| rev.state != "dead" && mainline::is_main_line(num, vendor_branch))
            .map(|(num, _)| num.clone())
            .collect();
        if let Some(vb) = vendor_branch {
            if let Some(first) = mainline::vendor_branch_first_revision(&f.rcs, vb) {
                let bp = mainline::branch_point(vb);
                if let (Some(bp_rev), Some(first_rev)) =
                    (f.rcs.revisions.get(&bp), f.rcs.revisions.get(&first))
                {
                    if bp_rev.date == first_rev.date {
                        wanted.insert(bp);
                        wanted.insert(first);
                    }
                }
            }
        }
        Self {
            many: f.rcs.contents_of_many(&wanted).ok(),
        }
    }

    /// Whether two revisions have the same content (a `cvs import`'s branch point and first vendor revision).
    fn same(&self, rcs: &RcsFile, a: &RevNum, b: &RevNum) -> Result<bool, Error> {
        if let Some(m) = &self.many {
            if let (Some(x), Some(y)) = (m.get(a), m.get(b)) {
                return Ok(x == y);
            }
        }
        Ok(rcs.content_of(a)? == rcs.content_of(b)?)
    }

    /// The content of `num`, handed over (each is asked for once).
    fn take(&mut self, rcs: &RcsFile, num: &RevNum) -> Result<Vec<u8>, Error> {
        if let Some(c) = self.many.as_mut().and_then(|m| m.remove(num)) {
            return Ok(c);
        }
        rcs.content_of(num)
    }
}

/// Refuse (never guess) a file whose default branch is set but which also has trunk revisions after
/// that branch's branch point — `cvs admin -b` can produce this shape, and there is then no single
/// honest main line: a checkout gives the branch tip, while the trunk moved on (review 008 R-5).
fn check_no_later_trunk_after_vendor_branch(f: &scan::CvsFile) -> Result<(), Error> {
    let Some(vb) = &f.rcs.branch else {
        return Ok(());
    };
    let bp = mainline::branch_point(vb);
    let ambiguous = f
        .rcs
        .revisions
        .keys()
        .any(|num| num.is_trunk() && *num > bp);
    if ambiguous {
        return Err(Error::FloorRefusal {
            feature: floor::DEFAULT_BRANCH_WITH_LATER_TRUNK.to_string(),
            reason: format!(
                "'{}': the default branch is set, but trunk revisions exist after its branch point \
                 ({}); the main line is ambiguous",
                f.path,
                bp.to_dotted()
            ),
        });
    }
    Ok(())
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
    // The one-claim rule (RFC 011 D-5): CVS states only a committer login and a commit time — there is
    // no separate "author" the source distinguishes, so that single claim is carried as `author`, never
    // duplicated onto `committer`/`commit_time` (corrections handoff §2.3/CR-04).
    let author = if cs.author.is_empty() {
        None
    } else {
        Some(Identity {
            // No lossy conversion (review 008 R-4): the committer login is carried as raw bytes.
            name: Text {
                bytes: cs.author.clone(),
                encoding: None,
            },
            email: None,
        })
    };
    let message = if cs.log.is_empty() {
        None
    } else {
        // No lossy conversion — the log message is carried as raw bytes (RFC 011 §5).
        Some(Text {
            bytes: cs.log.clone(),
            encoding: None,
        })
    };
    // RCS dates are UTC by definition (corrections handoff §2.3).
    let author_time = Some(Time {
        seconds: cs.date,
        offset_minutes: Some(0),
    });
    MetadataClaims {
        author,
        author_time,
        committer: None,
        commit_time: None,
        message,
    }
}

/// The `Derived(ReconstructedChangeset)` status carrying the clustering parameters and confidence (rule
/// `span-overlap-v1`, corrections handoff §2.2).
fn derived_status(cs: &Changeset, opts: &Options) -> EpistemicStatus {
    let mut params = BTreeMap::new();
    params.insert("window_secs".to_string(), opts.window_secs.to_string());
    params.insert("cluster_keys".to_string(), "author,log".to_string());
    params.insert("confidence_rule".to_string(), "span-overlap-v1".to_string());
    params.insert("date_rule".to_string(), "latest-per-file".to_string());
    params.insert(
        "order_splits".to_string(),
        u8::from(cs.order_split).to_string(),
    );
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

/// The repository's content-derived fingerprint (corrections handoff §2.3/CR-08.1, amended by review 008
/// R-6): SHA-256 over every file that has a trunk revision, in ascending path order (`scan` already sorts
/// `files` by path), of `path ‖ 0x00 ‖ rev ‖ 0x00 ‖ decimal date of rev ‖ 0x00 ‖ author of rev ‖ 0x0A`,
/// where `rev` is the file's **lowest-numbered** trunk revision (normally `1.1`, but not always — a file
/// need not start at `1.1`). Never a filesystem path — the same repository decoded from two locations
/// gives identical bytes.
///
/// # Errors
/// [`Error::Read`] if no file in the repository has any trunk revision at all: there is nothing to
/// fingerprint, and nothing to import either.
fn compute_repo_id(files: &[scan::CvsFile]) -> Result<Vec<u8>, Error> {
    let mut hasher = Sha256::new();
    let mut any = false;
    for f in files {
        let Some((num, rev)) = f.rcs.revisions.iter().find(|(n, _)| n.is_trunk()) else {
            continue;
        };
        any = true;
        hasher.update(f.path.as_bytes());
        hasher.update([0u8]);
        hasher.update(num.to_dotted().as_bytes());
        hasher.update([0u8]);
        hasher.update(rev.date.to_string().as_bytes());
        hasher.update([0u8]);
        hasher.update(&rev.author);
        hasher.update([0x0au8]);
    }
    if !any {
        return Err(Error::Read("no main-line revisions".to_string()));
    }
    Ok(hasher.finalize().to_vec())
}

/// How many symbols [`add_refs`] found that named no main-line revision at all, and so were not
/// reconstructed (corrections handoff §2.1) — counted by the caller into drop records. Split by kind
/// (review 008 R-1): a tag resolving to nothing is common under main-line-only import (a vendor release
/// tag once the vendor branch is cleared, or a tag on a release branch) and must be counted, never
/// silently skipped, same as a branch symbol.
#[derive(Debug, Default)]
struct RefStats {
    branch_symbols_not_reconstructed: usize,
    tags_not_reconstructed: usize,
}

/// Reconstruct `Derived` tag/branch refs from per-file symbolic names (opt-in, RFC 007 D-5), against
/// main-line changesets only (a branch revision was never added, so it can never resolve here).
fn add_refs(
    files: &[scan::CvsFile],
    rev_to_atom: &HashMap<(String, String), (AtomId, i64)>,
    builder: &mut IrBuilder,
) -> Result<RefStats, Error> {
    let mut stats = RefStats::default();
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
            if sym.is_branch {
                stats.branch_symbols_not_reconstructed += 1;
            } else {
                stats.tags_not_reconstructed += 1;
            }
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
            annotation: None,
        })?;
    }
    Ok(stats)
}

/// Accumulated loss categories (RFC 007 D-6), rendered into the loss boundary at the end.
#[derive(Debug, Default)]
struct Loss {
    has_expand: bool,
}

impl Loss {
    fn into_boundary(self) -> LossBoundary {
        let mut dropped = vec![
            DropRecord {
                class: LossClass::Representation,
                what: "RCS ,v physical layout and delta encoding".to_string(),
                reason: "representation not assertion; the logical revisions are preserved"
                    .to_string(),
            },
            DropRecord {
                class: LossClass::Representation,
                what: "CVSROOT administrative files, locks, and working-copy state".to_string(),
                reason: "configuration and local state, not history".to_string(),
            },
        ];
        if self.has_expand {
            dropped.push(DropRecord {
                class: LossClass::Representation,
                what: "RCS keyword expansion / -kb text translation".to_string(),
                reason: "a working-copy transform; the stored (unexpanded) bytes are carried \
                         verbatim"
                    .to_string(),
            });
        }
        LossBoundary { dropped }
    }
}

#[cfg(test)]
mod tests;

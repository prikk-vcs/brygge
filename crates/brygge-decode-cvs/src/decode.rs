//! The decode orchestration (RFC 007 D-2/D-3/D-4/D-9, owner ruling D-2): read a CVS repository's RCS
//! `,v` files, keep only its **main line** (trunk, plus a vendor branch while one is set — corrections
//! handoff §2.1), reconstruct changesets from it, and build a [`brygge_ir::Ir`] whose changeset atoms are
//! **`Derived(ReconstructedChangeset)`** with a faithful per-file op spine. Builds against IR contract
//! 0.2.0 (RFC 011), with no new field or variant beyond what that contract already defines (D-9).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use sha2::{Digest, Sha256};

use brygge_ir::BlobId;
use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, DropRecord, Flag, FlagKind, Identity, ImportProvenance, Ir, LossBoundary, LossClass,
    MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity, SourceKind, Text, Time,
};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};

use crate::branches;
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

    // With `--reconstruct-refs`, the named branches are imported (RFC 013 D-3, §6): find them by symbol name,
    // and decide which are imported, on which parent line, and in what order. Without it, the main line only,
    // exactly as before.
    let discovered = if opts.reconstruct_refs {
        branches::discover(&files)
    } else {
        BTreeMap::new()
    };
    let mut stats = BranchStats::default();
    // A file whose symbol names a branch point that does not exist leaves the branch, counted (never a refusal).
    let mut present: BTreeMap<String, Vec<branches::OnBranch>> = BTreeMap::new();
    for (name, ons) in &discovered {
        for on in ons {
            if on.state == branches::PointState::Missing {
                stats.missing_point_files += 1;
            } else {
                present.entry(name.clone()).or_default().push(on.clone());
            }
        }
    }
    let resolution = branches::resolve(&present);
    stats.blocked = resolution.blocked.clone();
    // Per file, the imported branches (by their number in that file) and their names.
    let mut importable: HashMap<usize, BTreeMap<RevNum, String>> = HashMap::new();
    let mut branch_files_on: BTreeMap<String, Vec<branches::OnBranch>> = BTreeMap::new();
    // Per file, the live branch-point revisions its imported branches start from.
    let mut points_of: HashMap<usize, BTreeSet<RevNum>> = HashMap::new();
    for name in &resolution.order {
        for on in present.get(name).into_iter().flatten() {
            importable
                .entry(on.file)
                .or_default()
                .insert(on.branch_id.clone(), name.clone());
            if on.state == branches::PointState::Live {
                points_of
                    .entry(on.file)
                    .or_default()
                    .insert(on.point.clone());
            }
            branch_files_on
                .entry(name.clone())
                .or_default()
                .push(on.clone());
        }
    }

    // Gather every MAIN-LINE per-file revision (owner ruling D-2), reconstructing content (except for
    // `dead` deletions), and, with `--reconstruct-refs`, every revision of each imported branch.
    let mut filerevs: Vec<FileRev> = Vec::new();
    let mut branch_revs: BTreeMap<String, Vec<FileRev>> = BTreeMap::new();
    // (file, branch-point revision) -> the blob of that revision's content, for a branch's tree at its cut.
    let mut point_blob: HashMap<(usize, RevNum), BlobId> = HashMap::new();
    // A `cvs import`'s skipped branch point, per file, with the vendor revision that stands in for it.
    let mut skipped_points: HashMap<usize, (RevNum, RevNum)> = HashMap::new();
    let has_expand = files.iter().any(|f| f.rcs.expand.is_some());
    for (fi, f) in files.iter().enumerate() {
        check_no_later_trunk_after_vendor_branch(f)?;
        let vendor_branch = f.rcs.branch.clone();
        let here = importable.get(&fi);
        // Non-dead revisions on this file's imported branches: their content is rebuilt in the same pass.
        let branch_wanted: BTreeSet<RevNum> = f
            .rcs
            .revisions
            .iter()
            .filter(|(num, rev)| {
                rev.state != "dead"
                    && !mainline::is_main_line(num, vendor_branch.as_ref())
                    && here.is_some_and(|m| m.contains_key(&mainline::branch_of(num)))
            })
            .map(|(num, _)| num.clone())
            .collect();
        // RFC 010 increment 3: every content this file needs, in one pass over its delta chains. That
        // includes the branch points its imported branches start from (a trunk revision, or a revision of
        // another line): the branch's tree at the cut is made of them.
        let points = points_of.get(&fi);
        let mut wanted_here = branch_wanted.clone();
        wanted_here.extend(points.into_iter().flatten().cloned());
        let mut contents = FileContents::new(f, vendor_branch.as_ref(), &wanted_here);
        for point in points.into_iter().flatten() {
            let blob = builder.add_blob(contents.peek(&f.rcs, point)?);
            point_blob.insert((fi, point.clone()), blob);
        }
        let skip_branch_point = vendor_import_branch_point(f, &contents)?;
        if let (Some(bp), Some(vb)) = (&skip_branch_point, &vendor_branch) {
            if let Some(first) = mainline::vendor_branch_first_revision(&f.rcs, vb) {
                skipped_points.insert(fi, (bp.clone(), first));
            }
        }
        for (num, rev) in &f.rcs.revisions {
            if Some(num) == skip_branch_point.as_ref() {
                // cvs import's branch-point revision, identical to the vendor branch's first revision
                // and same-dated: contributes no separate op (avoids a spurious no-op Modify).
                continue;
            }
            let on_main = mainline::is_main_line(num, vendor_branch.as_ref());
            let branch_name = if on_main {
                None
            } else {
                here.and_then(|m| m.get(&mainline::branch_of(num)))
            };
            if on_main || branch_name.is_some() {
                let dead = rev.state == "dead";
                let content = if dead {
                    Vec::new()
                } else {
                    contents.take(&f.rcs, num)?
                };
                let fr = FileRev {
                    path: f.path.clone(),
                    rev: num.clone(),
                    date: rev.date,
                    author: rev.author.clone(),
                    log: rev.log.clone(),
                    state: rev.state.clone(),
                    content,
                };
                match branch_name {
                    Some(name) => branch_revs.entry(name.clone()).or_default().push(fr),
                    None => filerevs.push(fr),
                }
            } else {
                stats.count_excluded(f, num, opts.reconstruct_refs);
            }
        }
    }

    let changesets = cluster::reconstruct(filerevs, opts.window_secs);

    // Whole-import floor: if nothing reconstructs confidently, refuse rather than import all-uncertain (OQ-B).
    // It considers the main line only, so its behaviour does not depend on the branches.
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

    // Each imported branch: its changesets (clustered within the branch only), the tree it starts from, and
    // where it is cut from its parent line (the main line, or another branch).
    let plans = plan_branches(
        &files,
        &changesets,
        &resolution,
        &branch_files_on,
        &point_blob,
        branch_revs,
        &skipped_points,
        opts,
    );
    // The changesets after which some child branch is cut: those trees are kept as the line is built.
    let mut snapshot_at: HashMap<branches::LineRef, BTreeSet<usize>> = HashMap::new();
    for p in &plans {
        if let branches::Parent::Changeset { index, .. } = p.parent {
            snapshot_at
                .entry(p.parent_line.clone())
                .or_default()
                .insert(index);
        }
    }

    let mut tree: BTreeMap<String, BlobId> = BTreeMap::new();
    // Per line: its changesets' atoms, in order, and the trees after the ones a child branch is cut from.
    let mut line_atoms: HashMap<branches::LineRef, Vec<AtomId>> = HashMap::new();
    let mut line_snapshots: HashMap<branches::LineRef, HashMap<usize, BTreeMap<String, BlobId>>> =
        HashMap::new();
    let no_snapshots = BTreeSet::new();
    let mut main_atoms: Vec<AtomId> = Vec::with_capacity(changesets.len());
    let mut main_snapshots: HashMap<usize, BTreeMap<String, BlobId>> = HashMap::new();
    let mut prev_atom: Option<AtomId> = None;
    // (path, rev-dotted) -> (atom, changeset date), for symbol resolution.
    let mut rev_to_atom: HashMap<(String, String), (AtomId, i64)> = HashMap::new();
    let loss = Loss { has_expand };
    // Tracked separately from `Loss`: no changeset is dropped for being low-confidence — it is imported
    // and flagged (RFC 011 D-8). One fact, one place.
    let mut low_confidence: usize = 0;
    let main_wanted = snapshot_at
        .get(&branches::LineRef::Main)
        .unwrap_or(&no_snapshots);

    for (k, cs) in changesets.iter().enumerate() {
        let ops = ops_for(&cs.revs, &mut tree, &mut builder);
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
                status: derived_status(cs, opts, &[]),
            })
            .map_err(Error::Ir)?;
        prev_atom = Some(atom_id);
        main_atoms.push(atom_id);
        if main_wanted.contains(&k) {
            main_snapshots.insert(k, tree.clone());
        }

        for fr in &cs.revs {
            rev_to_atom.insert((fr.path.clone(), fr.rev.to_dotted()), (atom_id, cs.date));
        }
        if cs.confidence < opts.confidence_floor {
            low_confidence += 1;
        }
    }
    line_atoms.insert(branches::LineRef::Main, main_atoms);
    line_snapshots.insert(branches::LineRef::Main, main_snapshots);

    // The branches, in dependency order: each is cut from an already-built line.
    let mut imported_branches: BTreeMap<String, AtomId> = BTreeMap::new();
    let mut approximate = 0usize;
    for plan in &plans {
        let parent_atom = match plan.parent {
            branches::Parent::Changeset { index, .. } => line_atoms
                .get(&plan.parent_line)
                .and_then(|atoms| atoms.get(index))
                .copied(),
            branches::Parent::Root => None,
        };
        let parent_tree = match plan.parent {
            branches::Parent::Changeset { index, .. } => line_snapshots
                .get(&plan.parent_line)
                .and_then(|snaps| snaps.get(&index))
                .cloned()
                .unwrap_or_default(),
            branches::Parent::Root => BTreeMap::new(),
        };
        if matches!(
            plan.parent,
            branches::Parent::Changeset { exact: false, .. }
        ) {
            approximate += 1;
        }
        let own = branches::LineRef::Named(plan.name.clone());
        let wanted = snapshot_at.get(&own).unwrap_or(&no_snapshots);
        let built = build_branch(
            plan,
            parent_atom,
            &parent_tree,
            wanted,
            &mut builder,
            &repo_id,
            opts,
            &mut rev_to_atom,
            &mut low_confidence,
        )?;
        line_atoms.insert(own.clone(), built.atoms);
        line_snapshots.insert(own, built.snapshots);
        match built.last {
            Some(atom) => {
                imported_branches.insert(plan.name.clone(), atom);
            }
            None => stats.empty_branches += 1,
        }
    }

    let mut ref_stats = RefStats::default();
    if opts.reconstruct_refs {
        ref_stats = add_refs(&files, &rev_to_atom, &imported_branches, &mut builder)?;
    }

    let mut boundary = loss.into_boundary();
    boundary
        .dropped
        .extend(stats.records(opts.reconstruct_refs));
    if ref_stats.branch_symbols_not_reconstructed > 0 {
        boundary.dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!(
                "CVS branch symbols not reconstructed ({})",
                ref_stats.branch_symbols_not_reconstructed
            ),
            reason: if opts.reconstruct_refs {
                "the symbol is a vendor branch (a literal branch number, imported as the main line while \
                 it is the default and not reconstructed as a line of its own), a branch whose parent \
                 line is not imported (counted in its own record), or names a branch with no file and no \
                 revision to import"
                    .to_string()
            } else {
                BRANCHES_NEED_REFS.to_string()
            },
        });
    }
    if ref_stats.tags_not_reconstructed > 0 {
        // Review 008 R-1: a tag naming no imported revision (a vendor release tag once the vendor branch is
        // cleared, or a tag on a line that is not imported) is never skipped silently.
        boundary.dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!(
                "CVS tags on branch revisions not reconstructed ({})",
                ref_stats.tags_not_reconstructed
            ),
            reason: if opts.reconstruct_refs {
                "the tag names revisions on a line that is not imported (an unnamed branch, a vendor \
                 branch, or a branch whose parent line is not imported)"
                    .to_string()
            } else {
                BRANCHES_NEED_REFS.to_string()
            },
        });
    }
    builder.set_loss(boundary);

    if approximate > 0 {
        builder.add_flag(Flag {
            kind: FlagKind::ConventionViolation,
            what: "CVS branch point spans reconstructed changesets".to_string(),
            count: approximate as u64,
            reason: format!(
                "{approximate} branch(es) were cut across reconstructed changesets: no changeset has every \
                 file of the branch at its branch-point revision, so the parent follows the rule \
                 earliest-covering-changeset (the changeset with the most files at their branch point, the \
                 earliest on ties). The branch's tree is still exact (a branch-point atom sets it)"
            ),
        });
    }
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

/// The drop reason without `--reconstruct-refs` (RFC 013 OQ-1): true today, and it says what to do.
const BRANCHES_NEED_REFS: &str = "CVS branches are imported only with `--reconstruct-refs`: a branch is \
                                  identified by its symbol name. Keep the source repository.";

/// Tallies of the branch revisions (and symbols) that are not imported, for the drop records (corrections
/// handoff §2.1, RFC 013 D-3): one record per kind, each with exact counts.
#[derive(Debug, Default)]
struct BranchStats {
    /// Without `--reconstruct-refs`: every branch revision (all are excluded).
    excluded_revisions: usize,
    /// Without `--reconstruct-refs`: distinct branch *symbol names* touched by excluded revisions.
    named: BTreeSet<String>,
    /// Revisions on a branch with no symbol in their file (RFC 013 OQ-2).
    unnamed_revisions: usize,
    /// Symbol names whose parent line is not imported (an unnamed line, or one itself dropped): not imported.
    blocked: BTreeSet<String>,
    /// Revisions on those symbols' branches, in every file.
    blocked_revisions: usize,
    /// Revisions on a vendor branch (a literal branch number) that are not on the main line.
    vendor_revisions: usize,
    /// Files whose branch symbol names a branch-point revision that does not exist.
    missing_point_files: usize,
    /// Revisions on a branch of such a file.
    missing_point_revisions: usize,
    /// Branches with no file at a live branch point, no revision and so nothing to import.
    empty_branches: usize,
}

impl BranchStats {
    /// Count one branch revision that is not imported.
    fn count_excluded(&mut self, f: &scan::CvsFile, num: &RevNum, reconstruct_refs: bool) {
        self.excluded_revisions += 1;
        let branch_id = mainline::branch_of(num);
        let symbol = mainline::branch_symbol(&f.rcs, &branch_id);
        if !reconstruct_refs {
            match symbol {
                Some(mainline::BranchSymbol::Magic(n) | mainline::BranchSymbol::Literal(n)) => {
                    self.named.insert(n.to_string());
                }
                None => self.unnamed_revisions += 1,
            }
            return;
        }
        match symbol {
            Some(mainline::BranchSymbol::Magic(n)) if self.blocked.contains(n) => {
                // a symbol whose parent line is not imported: none of its revisions is imported
                self.blocked_revisions += 1;
            }
            Some(mainline::BranchSymbol::Magic(_)) => {
                // cut from the main line, but the branch point does not exist: the file left the branch
                self.missing_point_revisions += 1;
            }
            Some(mainline::BranchSymbol::Literal(_)) => self.vendor_revisions += 1,
            None => self.unnamed_revisions += 1,
        }
    }

    /// The drop records for what is not imported.
    fn records(&self, reconstruct_refs: bool) -> Vec<DropRecord> {
        let mut out = Vec::new();
        let rec = |what: String, reason: &str| DropRecord {
            class: LossClass::Other,
            what,
            reason: reason.to_string(),
        };
        if !reconstruct_refs {
            if self.excluded_revisions > 0 {
                out.push(rec(self.what(), BRANCHES_NEED_REFS));
            }
            return out;
        }
        if self.unnamed_revisions > 0 {
            out.push(rec(
                format!(
                    "CVS branch revisions on unnamed branches not imported ({})",
                    self.unnamed_revisions
                ),
                "a branch with no symbol has no name to identify it across files; keep the source repository",
            ));
        }
        if !self.blocked.is_empty() {
            out.push(rec(
                format!(
                    "CVS branches whose parent line is not imported ({} branches, {} revisions)",
                    self.blocked.len(),
                    self.blocked_revisions
                ),
                "the branch is cut from a line that is not imported (a branch with no symbol, a vendor \
                 branch that is no longer the default, or a branch that is itself not imported), so it \
                 has no parent to hang from; keep the source repository",
            ));
        }
        if self.vendor_revisions > 0 {
            out.push(rec(
                format!(
                    "CVS vendor-branch revisions after the vendor branch was cleared ({} revisions)",
                    self.vendor_revisions
                ),
                "vendor branches are imported as the main line while they are the default; their later \
                 imports are not reconstructed as a branch",
            ));
        }
        if self.missing_point_files > 0 {
            out.push(rec(
                format!(
                    "CVS branch symbols naming a missing revision ({} files, {} revisions)",
                    self.missing_point_files, self.missing_point_revisions
                ),
                "the branch-point revision the symbol names does not exist in that file (outdated, for \
                 example with `cvs admin -o`), so the file is not on the branch",
            ));
        }
        out
    }

    /// The legacy record's wording (without `--reconstruct-refs`).
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
    fn new(
        f: &scan::CvsFile,
        vendor_branch: Option<&RevNum>,
        branch_wanted: &BTreeSet<RevNum>,
    ) -> Self {
        let mut wanted: BTreeSet<RevNum> = f
            .rcs
            .revisions
            .iter()
            .filter(|(num, rev)| rev.state != "dead" && mainline::is_main_line(num, vendor_branch))
            .map(|(num, _)| num.clone())
            .collect();
        wanted.extend(branch_wanted.iter().cloned());
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

    /// The content of `num`, copied (a branch point's content is needed by the branches cut from it, and again
    /// by the main line when it is one of its revisions).
    fn peek(&self, rcs: &RcsFile, num: &RevNum) -> Result<Vec<u8>, Error> {
        if let Some(c) = self.many.as_ref().and_then(|m| m.get(num)) {
            return Ok(c.clone());
        }
        rcs.content_of(num)
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

/// Determine a changeset's path operations against the line's running tree, and update the tree.
fn ops_for(
    revs: &[FileRev],
    tree: &mut BTreeMap<String, BlobId>,
    builder: &mut IrBuilder,
) -> Vec<PathOp> {
    let mut ops = Vec::new();
    for fr in revs {
        if fr.is_dead() {
            if tree.remove(&fr.path).is_some() {
                ops.push(PathOp::Delete {
                    path: fr.path.clone(),
                    status: EpistemicStatus::Stated,
                });
            }
            // A dead revision for a path that was never live is a no-op (created-then-deleted off-mainline).
        } else {
            let blob = builder.add_blob(fr.content.clone());
            if tree.insert(fr.path.clone(), blob).is_none() {
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
fn derived_status(cs: &Changeset, opts: &Options, extra: &[(&str, String)]) -> EpistemicStatus {
    let mut params = BTreeMap::new();
    for (k, v) in extra {
        params.insert((*k).to_string(), v.clone());
    }
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

/// Reconstruct `Derived` tag/branch refs from per-file symbolic names (opt-in, RFC 007 D-5): a tag against
/// the changeset of its latest-dated named revision (main line or an imported branch), and a **branch**
/// (RFC 013 D-3) at its last atom. A symbol that resolves to nothing is counted, never skipped silently.
fn add_refs(
    files: &[scan::CvsFile],
    rev_to_atom: &HashMap<(String, String), (AtomId, i64)>,
    imported_branches: &BTreeMap<String, AtomId>,
    builder: &mut IrBuilder,
) -> Result<RefStats, Error> {
    let mut stats = RefStats::default();
    for sym in symbols::collect(files) {
        let mut params = BTreeMap::new();
        params.insert("source".to_string(), "cvs-symbol".to_string());
        let (target, kind) = if let Some(&atom) = imported_branches.get(&sym.name) {
            (atom, RefKind::Branch)
        } else {
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
            (target, kind)
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

/// One imported branch: its changesets (clustered within the branch only), the line it is cut from and where
/// on that line, and the tree it starts from.
struct BranchPlan {
    name: String,
    changesets: Vec<Changeset>,
    /// The line this branch is cut from: the main line, or another imported branch.
    parent_line: branches::LineRef,
    parent: branches::Parent,
    /// The branch's tree at the cut: every file on the branch that has a live branch point, at that revision's
    /// content, **whatever line the file's branch point is on**.
    cut_tree: BTreeMap<String, BlobId>,
}

/// A parent line's changesets, indexed for the rule **earliest-covering-changeset**: where each of its
/// revisions landed, and each file's revisions on the line in order.
struct LineIndex<'a> {
    cs_of: HashMap<(&'a str, &'a RevNum), usize>,
    per_file: HashMap<&'a str, Vec<(&'a RevNum, usize)>>,
    len: usize,
}

impl<'a> LineIndex<'a> {
    fn new(changesets: &'a [Changeset]) -> Self {
        let mut cs_of: HashMap<(&str, &RevNum), usize> = HashMap::new();
        let mut per_file: HashMap<&str, Vec<(&RevNum, usize)>> = HashMap::new();
        for (k, cs) in changesets.iter().enumerate() {
            for fr in &cs.revs {
                cs_of.insert((fr.path.as_str(), &fr.rev), k);
                per_file
                    .entry(fr.path.as_str())
                    .or_default()
                    .push((&fr.rev, k));
            }
        }
        for revs in per_file.values_mut() {
            revs.sort();
        }
        Self {
            cs_of,
            per_file,
            len: changesets.len(),
        }
    }
}

/// Plan every imported branch, in dependency order (RFC 013 D-3, §6): cluster its revisions, and choose its
/// parent on its parent line by the rule **earliest-covering-changeset** from each covered file's run of that
/// line's changesets during which the file is exactly at its branch-point revision.
///
/// A file whose branch point is on a *different* line than the parent line is on the branch (its tree at the
/// cut has it) but does not constrain the parent: the parent is then approximate.
#[allow(clippy::too_many_arguments)]
fn plan_branches(
    files: &[scan::CvsFile],
    main: &[Changeset],
    resolution: &branches::Resolution,
    on_branch: &BTreeMap<String, Vec<branches::OnBranch>>,
    point_blob: &HashMap<(usize, RevNum), BlobId>,
    mut branch_revs: BTreeMap<String, Vec<FileRev>>,
    skipped_points: &HashMap<usize, (RevNum, RevNum)>,
    opts: &Options,
) -> Vec<BranchPlan> {
    // Cluster every imported branch first: a branch cut from another needs that one's changesets.
    let mut clustered: BTreeMap<&str, Vec<Changeset>> = BTreeMap::new();
    for name in &resolution.order {
        let mut revs = branch_revs.remove(name).unwrap_or_default();
        revs.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.rev.cmp(&b.rev)));
        let changesets = if revs.is_empty() {
            Vec::new()
        } else {
            cluster::reconstruct(revs, opts.window_secs)
        };
        clustered.insert(name.as_str(), changesets);
    }
    // The parent choices (which borrow the changesets), then the plans (which take them).
    let mut chosen: Vec<(
        String,
        branches::LineRef,
        branches::Parent,
        BTreeMap<String, BlobId>,
    )> = Vec::new();
    let mut indexes: HashMap<branches::LineRef, LineIndex<'_>> = HashMap::new();
    for name in &resolution.order {
        let parent_line = resolution
            .parent
            .get(name)
            .cloned()
            .unwrap_or(branches::LineRef::Main);
        let index = indexes
            .entry(parent_line.clone())
            .or_insert_with(|| match &parent_line {
                branches::LineRef::Named(p) => {
                    LineIndex::new(clustered.get(p.as_str()).map_or(&[][..], Vec::as_slice))
                }
                _ => LineIndex::new(main),
            });
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut cut_tree: BTreeMap<String, BlobId> = BTreeMap::new();
        let mut live_files = 0usize;
        for on in on_branch.get(name).into_iter().flatten() {
            let Some(f) = files.get(on.file) else {
                continue;
            };
            if on.state != branches::PointState::Live {
                // The file was added on the branch (a dead `1.1` on the trunk): it does not constrain the
                // parent, and its branch revisions add it.
                continue;
            }
            live_files += 1;
            if let Some(blob) = point_blob.get(&(on.file, on.point.clone())) {
                cut_tree.insert(f.path.clone(), *blob);
            }
            if on.line != parent_line {
                continue; // on the branch, but its branch point is on another line
            }
            // A `cvs import`'s branch point is skipped as an op; the vendor revision stands in for it.
            let anchor = match skipped_points.get(&on.file) {
                Some((bp, first)) if *bp == on.point => first.clone(),
                _ => on.point.clone(),
            };
            let Some(&in_idx) = index.cs_of.get(&(f.path.as_str(), &anchor)) else {
                continue;
            };
            let out_idx = index
                .per_file
                .get(f.path.as_str())
                .and_then(|revs| revs.iter().find(|(r, _)| **r > anchor))
                .map_or(index.len, |(_, k)| *k)
                .max(in_idx + 1);
            runs.push((in_idx, out_idx));
        }
        let mut parent = branches::choose_parent(&runs, index.len);
        if let branches::Parent::Changeset { index: i, .. } = parent {
            if runs.len() < live_files {
                // a live file whose branch point is on another line (or not on this one) is not covered
                parent = branches::Parent::Changeset {
                    index: i,
                    exact: false,
                };
            }
        }
        chosen.push((name.clone(), parent_line, parent, cut_tree));
    }
    drop(indexes);
    chosen
        .into_iter()
        .map(|(name, parent_line, parent, cut_tree)| BranchPlan {
            changesets: clustered.remove(name.as_str()).unwrap_or_default(),
            name,
            parent_line,
            parent,
            cut_tree,
        })
        .collect()
}

/// A branch, built: the atoms of its changesets (in order), the trees after the ones a child is cut from, and
/// its last atom.
struct BuiltLine {
    last: Option<AtomId>,
    atoms: Vec<AtomId>,
    snapshots: HashMap<usize, BTreeMap<String, BlobId>>,
}

/// Build one branch's atoms: an optional **branch-point atom** that makes the parent's tree into the branch's
/// tree at the cut (exactly the files on the branch, each at its branch-point content), then the branch's
/// changesets chained in cluster order. `last` is the branch's last atom, or `None` when it has nothing to
/// import (no parent, no branch-point atom and no changeset).
#[allow(clippy::too_many_arguments)]
fn build_branch(
    plan: &BranchPlan,
    parent_atom: Option<AtomId>,
    parent_tree: &BTreeMap<String, BlobId>,
    snapshot_at: &BTreeSet<usize>,
    builder: &mut IrBuilder,
    repo_id: &[u8],
    opts: &Options,
    rev_to_atom: &mut HashMap<(String, String), (AtomId, i64)>,
    low_confidence: &mut usize,
) -> Result<BuiltLine, Error> {
    let rule = "earliest-covering-changeset".to_string();
    let exactness = match plan.parent {
        branches::Parent::Changeset { exact: true, .. } => "exact",
        branches::Parent::Changeset { exact: false, .. } => "approximate",
        branches::Parent::Root => "unconstrained",
    };
    let parent_line = match &plan.parent_line {
        branches::LineRef::Named(p) => Some(p.clone()),
        _ => None,
    };
    let point_params = |extra: &mut Vec<(&str, String)>| {
        extra.push(("line", plan.name.clone()));
        if let Some(p) = &parent_line {
            extra.push(("parent_line", p.clone()));
        }
        extra.push(("branch_point_rule", rule.clone()));
        extra.push(("branch_point", exactness.to_string()));
    };

    // The branch's tree at the cut: its files at their branch-point content.
    let mut branch_tree = plan.cut_tree.clone();

    let derived = |kind_params: &[(&str, &str)]| {
        let mut params = BTreeMap::new();
        params.insert("source".to_string(), "cvs-symbol".to_string());
        params.insert("line".to_string(), plan.name.clone());
        if let Some(p) = &parent_line {
            params.insert("parent_line".to_string(), p.clone());
        }
        for (k, v) in kind_params {
            params.insert((*k).to_string(), (*v).to_string());
        }
        EpistemicStatus::Derived(Derivation {
            kind: DerivationKind::ReconstructedBranch,
            by: DECODER.to_string(),
            decoder_version: decoder_version().to_string(),
            params,
            confidence: None,
        })
    };

    // The branch-point atom: only when the parent's tree is not already the branch's tree.
    let mut ops: Vec<PathOp> = Vec::new();
    for path in parent_tree.keys() {
        if !branch_tree.contains_key(path) {
            ops.push(PathOp::Delete {
                path: path.clone(),
                status: derived(&[("rule", "branch-point-tree")]),
            });
        }
    }
    for (path, blob) in &branch_tree {
        match parent_tree.get(path) {
            None => ops.push(PathOp::Add {
                path: path.clone(),
                blob: *blob,
                mode: MODE_REGULAR,
                status: derived(&[("rule", "branch-point-tree")]),
            }),
            Some(b) if b != blob => ops.push(PathOp::Modify {
                path: path.clone(),
                blob: *blob,
                mode: MODE_REGULAR,
                status: derived(&[("rule", "branch-point-tree")]),
            }),
            Some(_) => {}
        }
    }
    let mut last = parent_atom;
    let mut carried_point = false;
    if !ops.is_empty() {
        let atom = builder
            .add_atom(AtomDraft {
                parents: parent_atom.into_iter().collect(),
                ops,
                copies: Vec::new(),
                metadata: MetadataClaims::default(),
                source: SourceIdentity {
                    kind: SourceKind::Cvs,
                    repo_id: repo_id.to_vec(),
                    atom_id: format!("branch-point:{}", plan.name).into_bytes(),
                    signatures: Vec::new(),
                    extras: Vec::new(),
                },
                status: derived(&[
                    ("branch_point_rule", &rule),
                    ("branch_point", exactness),
                    ("rule", "branch-point-tree"),
                ]),
            })
            .map_err(Error::Ir)?;
        last = Some(atom);
        carried_point = true;
    }

    // The branch's own changesets, each applying its `Stated` ops to the branch's own tree.
    let mut atoms = Vec::with_capacity(plan.changesets.len());
    let mut snapshots = HashMap::new();
    for (i, cs) in plan.changesets.iter().enumerate() {
        let ops = ops_for(&cs.revs, &mut branch_tree, builder);
        let mut extra: Vec<(&str, String)> = Vec::new();
        if i == 0 && !carried_point {
            point_params(&mut extra);
        } else {
            extra.push(("line", plan.name.clone()));
        }
        let atom = builder
            .add_atom(AtomDraft {
                parents: last.into_iter().collect(),
                ops,
                copies: Vec::new(),
                metadata: metadata_of(cs),
                source: SourceIdentity {
                    kind: SourceKind::Cvs,
                    repo_id: repo_id.to_vec(),
                    atom_id: changeset_atom_id(cs),
                    signatures: Vec::new(),
                    extras: Vec::new(),
                },
                status: derived_status(cs, opts, &extra),
            })
            .map_err(Error::Ir)?;
        last = Some(atom);
        atoms.push(atom);
        if snapshot_at.contains(&i) {
            snapshots.insert(i, branch_tree.clone());
        }
        for fr in &cs.revs {
            rev_to_atom.insert((fr.path.clone(), fr.rev.to_dotted()), (atom, cs.date));
        }
        if cs.confidence < opts.confidence_floor {
            *low_confidence += 1;
        }
    }
    Ok(BuiltLine {
        last,
        atoms,
        snapshots,
    })
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
mod branch_tests;
#[cfg(test)]
mod cvs_fixtures;
#[cfg(test)]
mod tests;

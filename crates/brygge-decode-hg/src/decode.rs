//! The decode orchestration (RFC 005 D-2/D-3/D-5): read a Mercurial store and build a [`brygge_ir::Ir`]
//! over its **published view** (owner ruling D-4, revised 2026-09-23) — exactly what `hg clone` would
//! transfer, with source-recorded renames carried as `Stated` (SRC-H2).
//!
//! Changelog revisions are already in parent-first order (parents have lower revs), so we iterate them
//! directly, skipping any that are not in the published view (secret/archived/internal or hidden — see
//! [`crate::published`]). Per served changeset: diff its manifest against its first parent's into
//! literal `PathOp`s, read file content and any `copy:`/`copyrev:` metadata from the filelogs, and
//! resolve a source-recorded copy's true source atom via the 5-step chain in
//! [`resolve_copy_from_atom`] (RFC 011 D-6 / RFC 005 corrections handoff §2).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, CopyRecord, DropRecord, Extra, Identity, ImportProvenance, Ir, LossBoundary, LossClass,
    MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity, SourceKind, Text, Time,
};
use brygge_ir::status::EpistemicStatus;

use crate::revlog::{NULL_REV, Revlog};
use crate::{
    Error, Options, changelog, decoder_version, filelog, floor, fncache, manifest, published,
    requires,
};

const DECODER: &str = "brygge-decode-hg";

/// Decode the Mercurial repository at `path` (its working root or `.hg` directory) into an [`Ir`],
/// restricted to its published view.
///
/// # Errors
/// [`Error::Open`] if the path is not a Mercurial repository; [`Error::UnsupportedFormat`] /
/// [`Error::FloorRefusal`] for a refused format or feature; [`Error::Read`] on a malformed store;
/// [`Error::Ir`] on an IR invariant violation.
pub fn decode(path: &Path, opts: &Options) -> Result<Ir, Error> {
    let (root, store) = locate(path)?;
    let encoding = check_requirements(&root, &store)?;

    let changelog = Revlog::open(&store.join("00changelog.i"))?;
    let manifest_log = Revlog::open(&store.join("00manifest.i"))?;

    // The published view (RFC 005 corrections handoff §1, D-4): only served changesets become atoms. It is
    // computed first because the repository's identity is taken from it (below). An empty repository has
    // nothing to publish.
    let view = if changelog.is_empty() {
        None
    } else {
        Some(published::compute(&root, &store, &changelog)?)
    };

    // Git parity (RFC 010 CR-15): the smallest **root** node, not revision 0 — among the *served*
    // changesets (review 012 F-1). Mercurial's revision numbers are assigned by local pull/commit order,
    // not content, so two clones of one repository pulled in different orders can number the same
    // changesets differently; a repository with more than one root makes "revision 0" ambiguous besides.
    // Only served roots count: they are read (so their nodes are verified), and the identity then equals
    // what `hg clone` of the repository would give — a secret root is not part of the published history
    // and must not decide its identity. Nothing served → empty, as for an empty repository.
    let repo_id = view
        .as_ref()
        .map(|v| {
            (0..changelog.len())
                .filter(|&rev| v.served.get(rev).copied().unwrap_or(false))
                .filter_map(|rev| changelog.entry(rev))
                .filter(|e| e.p1 == NULL_REV && e.p2 == NULL_REV)
                .map(|e| e.node)
                .min()
                .map(|node| node.to_vec())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let provenance = ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Hg,
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

    let Some(view) = view else {
        builder.set_loss(loss_boundary(&store, 0, 0, 0, 0, 0, 0));
        return builder.finish().map_err(Error::Ir);
    };

    // manifest node -> manifest revlog revision
    let mut manifest_node_to_rev: HashMap<[u8; 20], usize> = HashMap::new();
    for mrev in 0..manifest_log.len() {
        if let Some(e) = manifest_log.entry(mrev) {
            manifest_node_to_rev.insert(e.node, mrev);
        }
    }

    let mut filelogs: HashMap<String, Revlog> = HashMap::new();
    let filelog_store = FilelogStore {
        dir: &store,
        dotencode: encoding.dotencode,
    };
    let mut node_to_atom: HashMap<[u8; 20], AtomId> = HashMap::new();
    let mut branch_of: HashMap<usize, String> = HashMap::new();
    // Changesets that close their branch (a `close` extra): Mercurial's branch tip prefers an open head.
    let mut closes_branch: HashSet<usize> = HashSet::new();
    let mut unresolved_copies = 0usize;
    let mut copy_on_existing_path = 0usize;
    let mut unrepresentable_offsets = 0usize;

    for crev in 0..changelog.len() {
        if !view.served.get(crev).copied().unwrap_or(false) {
            continue;
        }
        let cs = changelog::parse(&changelog.revision(crev)?)?;
        branch_of.insert(crev, cs.branch.clone());
        if closes_its_branch(&cs.extras) {
            closes_branch.insert(crev);
        }
        let this_manifest = manifest_of(&manifest_log, &manifest_node_to_rev, &cs.manifest_node)?;

        if this_manifest.contains_key(".hgsub") || this_manifest.contains_key(".hgsubstate") {
            return Err(Error::FloorRefusal {
                feature: floor::SUBREPO.to_string(),
                reason: "Mercurial subrepositories are refused rather than approximated, as Git \
                         submodules are"
                    .to_string(),
            });
        }

        let entry = changelog
            .entry(crev)
            .ok_or_else(|| Error::Read("changelog entry vanished".to_string()))?;
        let p1_rev = if entry.p1 == NULL_REV {
            None
        } else {
            usize::try_from(entry.p1).ok()
        };
        let p2_rev = if entry.p2 == NULL_REV {
            None
        } else {
            usize::try_from(entry.p2).ok()
        };
        let parent_manifest = match p1_rev {
            None => BTreeMap::new(),
            Some(p1) => {
                let pcs = changelog::parse(&changelog.revision(p1)?)?;
                manifest_of(&manifest_log, &manifest_node_to_rev, &pcs.manifest_node)?
            }
        };

        let parents = parent_atoms(&changelog, crev, &node_to_atom)?;

        let (ops, copies, unresolved, on_existing) = diff_manifests(
            &filelog_store,
            &mut filelogs,
            &mut builder,
            &parent_manifest,
            &this_manifest,
            &changelog,
            &manifest_log,
            &manifest_node_to_rev,
            &node_to_atom,
            p1_rev,
            p2_rev,
            &view.served,
        )?;
        unresolved_copies += unresolved;
        copy_on_existing_path += on_existing;

        let author = identity(&cs.user);
        let offset_minutes = offset_minutes(cs.tz);
        if offset_minutes.is_none() {
            unrepresentable_offsets += 1;
        }
        let metadata = MetadataClaims {
            author: Some(author),
            author_time: Some(Time {
                seconds: cs.time,
                offset_minutes,
            }),
            committer: None,
            commit_time: None,
            message: Some(Text {
                bytes: cs.description.clone(),
                encoding: None,
            }),
        };

        let mut extras: Vec<Extra> = Vec::with_capacity(cs.extras.len());
        // Every extra is carried exactly as stored, in stored order, `branch` included: Mercurial writes it
        // only on changesets not on `default`, so a changeset's named branch, and with it every head of
        // every branch, is derivable from the atoms (the refs name only each branch's tip).
        for (k, v) in &cs.extras {
            let label = String::from_utf8(k.clone()).map_err(|_| Error::FloorRefusal {
                feature: floor::NON_UTF8_EXTRA_KEY.to_string(),
                reason: format!(
                    "a changelog extra key is not valid UTF-8: {}",
                    crate::util::escape_bytes(k)
                ),
            })?;
            extras.push(Extra {
                label,
                bytes: v.clone(),
            });
        }

        let atom_id = builder.add_atom(AtomDraft {
            parents,
            ops,
            copies,
            metadata,
            source: SourceIdentity {
                kind: SourceKind::Hg,
                repo_id: repo_id.clone(),
                atom_id: entry.node.to_vec(),
                signatures: Vec::new(),
                extras,
            },
            status: EpistemicStatus::Stated,
        })?;
        node_to_atom.insert(entry.node, atom_id);
    }

    let unresolved_bookmarks = add_refs(
        &view.bookmarks,
        &changelog,
        &branch_of,
        &closes_branch,
        &node_to_atom,
        &mut builder,
    )?;
    builder.set_loss(loss_boundary(
        &store,
        view.not_published,
        view.hidden,
        unresolved_copies,
        copy_on_existing_path,
        unresolved_bookmarks,
        unrepresentable_offsets,
    ));
    builder.finish().map_err(Error::Ir)
}

/// Mercurial stores its `tz` field as seconds **west** of UTC (east is negative). RFC 011's
/// `Time.offset_minutes` is minutes **east**. `None` if the seconds value does not divide evenly into
/// minutes, or the result does not fit `i16` (RFC 005 corrections handoff §3).
fn offset_minutes(tz: i64) -> Option<i16> {
    if tz % 60 != 0 {
        return None;
    }
    i16::try_from(-(tz / 60)).ok()
}

/// Resolve the repository root and its `.hg/store` directory.
fn locate(path: &Path) -> Result<(PathBuf, PathBuf), Error> {
    // A path that does not exist says so, in the words every source kind uses (a dangling symlink exists).
    if let Err(e) = std::fs::symlink_metadata(path) {
        if e.kind() == std::io::ErrorKind::NotFound {
            return Err(Error::Open(format!("source not found: {}", path.display())));
        }
    }
    let root = if path.join(".hg").is_dir() {
        path.to_path_buf()
    } else if path.file_name().is_some_and(|n| n == ".hg") && path.is_dir() {
        path.parent().unwrap_or(path).to_path_buf()
    } else {
        return Err(Error::Open(format!(
            "{} is not a Mercurial repository (no .hg)",
            path.display()
        )));
    };
    let store = root.join(".hg").join("store");
    if !store.is_dir() {
        return Err(Error::Open(format!(
            "{} has no .hg/store (unsupported repository layout)",
            root.display()
        )));
    }
    Ok((root, store))
}

/// Read one `requires` file: absence is "no requirements" (`Ok("")`); any other read failure — a
/// permission error above all — is [`Error::Open`], not a silent empty read (RFC 010 CR-15). Today's
/// format-safety gate must not be disabled by something as ordinary as a permission bit.
fn read_requires_file(path: &Path) -> Result<String, Error> {
    match std::fs::read_to_string(path) {
        Ok(body) => Ok(body),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(Error::Open(format!("cannot read {}: {e}", path.display()))),
    }
}

/// Read and check `.hg/requires` (and `.hg/store/requires` under share-safe) — the format-safety gate.
fn check_requirements(root: &Path, store: &Path) -> Result<requires::StoreEncoding, Error> {
    let mut body = read_requires_file(&root.join(".hg").join("requires"))?;
    let store_body = read_requires_file(&store.join("requires"))?;
    if !store_body.is_empty() {
        body.push('\n');
        body.push_str(&store_body);
    }
    requires::check(&body)
}

/// Parse the manifest a changeset points at. `pub(crate)`: also used by [`crate::published`] to read
/// `.hgtags` from a non-obsolete head's manifest.
pub(crate) fn manifest_of(
    manifest_log: &Revlog,
    node_to_rev: &HashMap<[u8; 20], usize>,
    manifest_node: &[u8; 20],
) -> Result<BTreeMap<String, manifest::Entry>, Error> {
    // The null manifest (empty tree) has an all-zero node and no revlog entry.
    if manifest_node == &[0u8; 20] {
        return Ok(BTreeMap::new());
    }
    let mrev = node_to_rev
        .get(manifest_node)
        .ok_or_else(|| Error::Read("manifest node not found in 00manifest".to_string()))?;
    manifest::parse(&manifest_log.revision(*mrev)?)
}

/// Diff `parent` → `child` manifests into literal `Stated` path operations, plus `Stated` copy records
/// for source-recorded copies (SRC-H2), each with its **true** source atom resolved via
/// [`resolve_copy_from_atom`]. Returns `(ops, copies, unresolved_copy_count, copy_on_existing_path_count)`
/// — review 009 R-5: a `copy:` with no `copyrev:` counts as unresolved (never silently dropped), and a
/// copy stated on a path that already existed (a modify, not an add) has no home in the IR's copy model
/// and is counted separately.
#[allow(clippy::too_many_arguments)]
fn diff_manifests(
    store: &FilelogStore<'_>,
    filelogs: &mut HashMap<String, Revlog>,
    builder: &mut IrBuilder,
    parent: &BTreeMap<String, manifest::Entry>,
    child: &BTreeMap<String, manifest::Entry>,
    changelog: &Revlog,
    manifest_log: &Revlog,
    manifest_node_to_rev: &HashMap<[u8; 20], usize>,
    node_to_atom: &HashMap<[u8; 20], AtomId>,
    p1_rev: Option<usize>,
    p2_rev: Option<usize>,
    served: &[bool],
) -> Result<(Vec<PathOp>, Vec<CopyRecord>, usize, usize), Error> {
    let mut ops = Vec::new();
    let mut copies = Vec::new();
    let mut unresolved = 0usize;
    let mut on_existing_path = 0usize;

    for (path, entry) in child {
        let is_add = !parent.contains_key(path);
        let changed = match parent.get(path) {
            None => true,
            Some(prev) => prev.filenode != entry.filenode || prev.mode != entry.mode,
        };
        if !changed {
            continue;
        }
        let file = read_file(store, filelogs, path, &entry.filenode)?;
        let blob = builder.add_blob(file.content.clone());
        if is_add {
            ops.push(PathOp::Add {
                path: path.clone(),
                blob,
                mode: entry.mode,
                status: EpistemicStatus::Stated,
            });
            // A copy source recorded on an added file is a stated rename/copy (SRC-H2): carried Stated,
            // beside the literal Add (and the Delete of the source, if the source was removed).
            match (&file.copy_from, file.copy_from_rev) {
                (Some(from), Some(from_rev)) => {
                    let from_filelog = open_filelog(store, filelogs, from)?;
                    let resolved = resolve_copy_from_atom(
                        changelog,
                        manifest_log,
                        manifest_node_to_rev,
                        node_to_atom,
                        parent,
                        from,
                        &from_rev,
                        p1_rev,
                        p2_rev,
                        from_filelog,
                        served,
                    )?;
                    place_copy(resolved, from, path, &mut copies, &mut unresolved);
                }
                // `copy:` without `copyrev:` — the source is named but not pinned to a revision. Never
                // placed on a guessed atom; counted the same as any other unresolvable copy (R-5).
                (Some(_), None) => unresolved += 1,
                (None, _) => {}
            }
        } else {
            // A copy stated on a modify (`hg cp -f` onto an existing path): the IR's copy model carries
            // a copy's destination as an `Add`, so this relationship has no home here. The literal
            // Modify is still carried; only the copy metadata is not (R-5).
            if file.copy_from.is_some() {
                on_existing_path += 1;
            }
            ops.push(PathOp::Modify {
                path: path.clone(),
                blob,
                mode: entry.mode,
                status: EpistemicStatus::Stated,
            });
        }
    }
    for path in parent.keys() {
        if !child.contains_key(path) {
            ops.push(PathOp::Delete {
                path: path.clone(),
                status: EpistemicStatus::Stated,
            });
        }
    }
    Ok((ops, copies, unresolved, on_existing_path))
}

/// Record a resolved copy, or — when the resolver found no atom to place it on — omit it and count it. A
/// stated copy is never placed on a guessed atom (review 009 §3.1).
fn place_copy(
    resolved: Option<AtomId>,
    from: &str,
    to: &str,
    copies: &mut Vec<CopyRecord>,
    unresolved: &mut usize,
) {
    match resolved {
        Some(from_atom) => copies.push(CopyRecord {
            from: from.to_string(),
            from_atom,
            to: to.to_string(),
            status: EpistemicStatus::Stated,
        }),
        None => *unresolved += 1,
    }
}

/// Where the filelogs are, and how their file names are encoded (`.hg/requires`, RFC 013 D-1).
struct FilelogStore<'a> {
    /// `.hg/store`.
    dir: &'a Path,
    /// `dotencode` from the repository's requirements.
    dotencode: bool,
}

fn open_filelog<'a>(
    store: &FilelogStore<'_>,
    filelogs: &'a mut HashMap<String, Revlog>,
    path: &str,
) -> Result<&'a Revlog, Error> {
    if !filelogs.contains_key(path) {
        // A hashed name embeds the SHA-1 of its own path, so the index and the data file are named
        // separately; neither is derived from the other.
        let names = fncache::store_paths(path, store.dotencode)?;
        let index = store.dir.join(&names.index);
        let data = store.dir.join(&names.data);
        // Only a real `NotFound` from the read itself says "not found" (no separate `stat`).
        let index_missing = format!("filelog for {path} not found at {}", names.index);
        let data_missing = format!("the data file of the filelog for {path} ({})", names.data);
        let rl = Revlog::open_with_data(&index, &data, Some(&index_missing), &data_missing)?;
        filelogs.insert(path.to_string(), rl);
    }
    filelogs
        .get(path)
        .ok_or_else(|| Error::Read("filelog cache miss".to_string()))
}

/// Read a file revision's content and copy source, caching open filelogs by path.
fn read_file(
    store: &FilelogStore<'_>,
    filelogs: &mut HashMap<String, Revlog>,
    path: &str,
    filenode: &[u8; 20],
) -> Result<filelog::FileRev, Error> {
    let rl = open_filelog(store, filelogs, path)?;
    filelog::read_revision(rl, filenode)
}

/// The lookups the copy-source decision needs, as a seam: the real implementation reads the store
/// ([`StoreCopyEnv`]), and the unit tests drive the decision with a small in-memory graph (review 009 F-4).
trait CopyEnv {
    /// `copyrev`'s own linkrev in `from`'s filelog: `None` when the filenode is absent from that filelog
    /// altogether ([`Linkrev::Absent`]), `Unknown` when the entry carries no usable linkrev.
    fn copyrev_linkrev(&self) -> Linkrev;
    /// Whether changeset `rev`'s manifest maps `from` to exactly `copyrev`.
    fn manifest_maps(&self, rev: usize) -> Result<bool, Error>;
    /// The IR atom for changeset `rev`, if it was imported.
    fn atom_of(&self, rev: usize) -> Option<AtomId>;
    /// Changeset `rev`'s first parent, if any.
    fn first_parent(&self, rev: usize) -> Option<usize>;
    /// Whether `rev` is in the published (served) view.
    fn served(&self, rev: usize) -> bool;
    /// Whether `target` is an ancestor-or-self of any revision in `starts`.
    fn is_ancestor(&self, target: usize, starts: &[usize]) -> bool;
}

/// What the filelog says about `copyrev`'s linkrev.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Linkrev {
    /// `copyrev` is not a revision of `from`'s filelog at all: nothing can place the copy.
    Absent,
    /// It is present but carries no usable linkrev: step 3 cannot run and step 4 is unbounded below.
    Unknown,
    /// The changeset that introduced the filenode.
    Known(usize),
}

/// The copy-source decision (RFC 011 D-6 / RFC 005 corrections handoff §2): p1 (when its manifest maps
/// `from` to `copyrev` — passed in as `p1_maps`, since the caller already parsed that manifest), else p2,
/// else `copyrev`'s linkrev (if served and an ancestor of this atom), else this atom's first-parent
/// ancestry nearest-first (bounded below `copyrev`'s own linkrev — review 009 R-8: no changeset older
/// than the one that introduced a filenode can contain it), else `None` (the caller omits the copy and
/// counts it). A `copyrev` absent from `from`'s filelog returns `None` at once, without walking (F-4).
fn decide_copy_source(
    env: &impl CopyEnv,
    p1_rev: Option<usize>,
    p2_rev: Option<usize>,
    p1_maps: bool,
) -> Result<Option<AtomId>, Error> {
    let linkrev = env.copyrev_linkrev();
    if linkrev == Linkrev::Absent {
        return Ok(None);
    }

    // Step 1: p1. It must win, so an `hg mv` is a move (`from_atom` = the first parent).
    if let Some(p1) = p1_rev {
        if p1_maps {
            if let Some(atom) = env.atom_of(p1) {
                return Ok(Some(atom));
            }
        }
    }
    // Step 2: p2.
    if let Some(p2) = p2_rev {
        if env.manifest_maps(p2)? {
            if let Some(atom) = env.atom_of(p2) {
                return Ok(Some(atom));
            }
        }
    }
    // Step 3: the linkrev, if served and an ancestor of this atom.
    if let Linkrev::Known(lr) = linkrev {
        let starts: Vec<usize> = [p1_rev, p2_rev].into_iter().flatten().collect();
        if env.served(lr) && env.is_ancestor(lr, &starts) {
            if let Some(atom) = env.atom_of(lr) {
                return Ok(Some(atom));
            }
        }
    }
    // Step 4: walk the first-parent ancestry nearest first (starting past p1, already checked), stopping
    // once the walked revision is below copyrev's linkrev (R-8).
    let mut cur = p1_rev.and_then(|p1| env.first_parent(p1));
    while let Some(rev) = cur {
        if let Linkrev::Known(lr) = linkrev {
            if rev < lr {
                break;
            }
        }
        if env.manifest_maps(rev)? {
            if let Some(atom) = env.atom_of(rev) {
                return Ok(Some(atom));
            }
        }
        cur = env.first_parent(rev);
    }
    Ok(None)
}

/// The store-backed [`CopyEnv`].
struct StoreCopyEnv<'a> {
    changelog: &'a Revlog,
    manifest_log: &'a Revlog,
    manifest_node_to_rev: &'a HashMap<[u8; 20], usize>,
    node_to_atom: &'a HashMap<[u8; 20], AtomId>,
    from_path: &'a str,
    copyrev: &'a [u8; 20],
    from_filelog: &'a Revlog,
    served: &'a [bool],
}

impl CopyEnv for StoreCopyEnv<'_> {
    fn copyrev_linkrev(&self) -> Linkrev {
        let Some(frev) = filelog::find_rev(self.from_filelog, self.copyrev) else {
            return Linkrev::Absent;
        };
        match self.from_filelog.entry(frev) {
            Some(entry) if entry.link_rev != NULL_REV => {
                usize::try_from(entry.link_rev).map_or(Linkrev::Unknown, Linkrev::Known)
            }
            _ => Linkrev::Unknown,
        }
    }
    fn manifest_maps(&self, rev: usize) -> Result<bool, Error> {
        let cs = changelog::parse(&self.changelog.revision(rev)?)?;
        let manifest = manifest_of(
            self.manifest_log,
            self.manifest_node_to_rev,
            &cs.manifest_node,
        )?;
        Ok(manifest
            .get(self.from_path)
            .is_some_and(|e| &e.filenode == self.copyrev))
    }
    fn atom_of(&self, rev: usize) -> Option<AtomId> {
        self.changelog
            .entry(rev)
            .and_then(|e| self.node_to_atom.get(&e.node))
            .copied()
    }
    fn first_parent(&self, rev: usize) -> Option<usize> {
        self.changelog.entry(rev).and_then(|e| {
            if e.p1 == NULL_REV {
                None
            } else {
                usize::try_from(e.p1).ok()
            }
        })
    }
    fn served(&self, rev: usize) -> bool {
        self.served.get(rev).copied().unwrap_or(false)
    }
    fn is_ancestor(&self, target: usize, starts: &[usize]) -> bool {
        is_ancestor_or_self(self.changelog, target, starts)
    }
}

/// Resolve a copy's true source changeset against the store (see [`decide_copy_source`]).
#[allow(clippy::too_many_arguments)]
fn resolve_copy_from_atom(
    changelog: &Revlog,
    manifest_log: &Revlog,
    manifest_node_to_rev: &HashMap<[u8; 20], usize>,
    node_to_atom: &HashMap<[u8; 20], AtomId>,
    p1_manifest: &BTreeMap<String, manifest::Entry>,
    from_path: &str,
    copyrev: &[u8; 20],
    p1_rev: Option<usize>,
    p2_rev: Option<usize>,
    from_filelog: &Revlog,
    served: &[bool],
) -> Result<Option<AtomId>, Error> {
    let env = StoreCopyEnv {
        changelog,
        manifest_log,
        manifest_node_to_rev,
        node_to_atom,
        from_path,
        copyrev,
        from_filelog,
        served,
    };
    let p1_maps = p1_manifest
        .get(from_path)
        .is_some_and(|e| &e.filenode == copyrev);
    decide_copy_source(&env, p1_rev, p2_rev, p1_maps)
}

/// Whether `target` is an ancestor-or-self of any revision in `starts`, walking parent links backward.
fn is_ancestor_or_self(changelog: &Revlog, target: usize, starts: &[usize]) -> bool {
    let mut stack: Vec<usize> = starts.to_vec();
    let mut seen: HashSet<usize> = HashSet::new();
    while let Some(rev) = stack.pop() {
        if rev == target {
            return true;
        }
        if rev < target || !seen.insert(rev) {
            continue;
        }
        if let Some(e) = changelog.entry(rev) {
            for p in [e.p1, e.p2] {
                if p != NULL_REV {
                    if let Ok(pi) = usize::try_from(p) {
                        stack.push(pi);
                    }
                }
            }
        }
    }
    false
}

/// Map a changeset's parents to their IR atom ids.
fn parent_atoms(
    changelog: &Revlog,
    crev: usize,
    node_to_atom: &HashMap<[u8; 20], AtomId>,
) -> Result<Vec<AtomId>, Error> {
    let entry = changelog
        .entry(crev)
        .ok_or_else(|| Error::Read("changelog entry out of range".to_string()))?;
    let mut out = Vec::new();
    for prev in [entry.p1, entry.p2] {
        if prev != NULL_REV {
            let pnode = changelog
                .entry(prev as usize)
                .ok_or_else(|| Error::Read("parent changeset out of range".to_string()))?
                .node;
            if let Some(a) = node_to_atom.get(&pnode) {
                out.push(*a);
            }
        }
    }
    Ok(out)
}

/// Parse an hg user string (`"Name <email>"`, raw source bytes) into an [`Identity`] claim. The
/// `<`/`>` delimiters are ASCII, so this splits on raw bytes without requiring the whole string to be
/// valid UTF-8; each half is carried as `Text` exactly as the source gave it.
fn identity(user: &[u8]) -> Identity {
    if let Some(lt) = rposition(user, b'<') {
        let (name, rest) = user.split_at(lt);
        let rest = rest.get(1..).unwrap_or(&[]);
        if let Some(gt) = position(rest, b'>') {
            let email = rest.get(..gt).unwrap_or(&[]);
            return Identity {
                name: Text {
                    bytes: trim_ascii_whitespace(name).to_vec(),
                    encoding: None,
                },
                email: Some(Text {
                    bytes: email.to_vec(),
                    encoding: None,
                }),
            };
        }
    }
    Identity {
        name: Text {
            bytes: trim_ascii_whitespace(user).to_vec(),
            encoding: None,
        },
        email: None,
    }
}

fn position(haystack: &[u8], needle: u8) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

fn rposition(haystack: &[u8], needle: u8) -> Option<usize> {
    haystack.iter().rposition(|&b| b == needle)
}

/// Trim ASCII whitespace from both ends, mirroring `str::trim` for the raw-byte user line.
fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    bytes.get(start..end).unwrap_or(&[])
}

/// Whether a changeset closes its branch: Mercurial decides by the **presence** of the `close` extra, whatever
/// its value (`changelog.py` `branchinfo`: `b'close' in extra`; it writes `1`).
fn closes_its_branch(extras: &[(Vec<u8>, Vec<u8>)]) -> bool {
    extras.iter().any(|(k, _)| k.as_slice() == b"close")
}

/// The tip of every branch, from its heads in ascending revision order as `(revision, branch, open)`:
/// what Mercurial resolves the name to (`branchmap.branchtip`, which is what `hg update <branch>` checks
/// out): the tipmost (highest revision number in this repository) open head, or, when every head is closed,
/// the tipmost head. A later head replaces the kept one unless it would replace an open head by a closed one.
fn branch_tips<'a>(
    heads: impl Iterator<Item = (usize, &'a str, bool)>,
) -> BTreeMap<&'a str, usize> {
    let mut tip_of: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
    for (crev, branch, open) in heads {
        match tip_of.get(branch) {
            Some(&(_, true)) if !open => {}
            _ => {
                tip_of.insert(branch, (crev, open));
            }
        }
    }
    tip_of.into_iter().map(|(b, (crev, _))| (b, crev)).collect()
}

/// Add bookmarks (`Bookmark`) and one ref per named branch (`NamedBranch`, at its branch tip) as refs
/// (RFC 005 D-4), over the **served** set only. Returns how many bookmarks named a changeset this build did not import (not
/// served, or unknown) — counted, never skipped silently (RFC 005 corrections handoff §3).
fn add_refs(
    bookmarks: &[(String, [u8; 20])],
    changelog: &Revlog,
    branch_of: &HashMap<usize, String>,
    closes_branch: &HashSet<usize>,
    node_to_atom: &HashMap<[u8; 20], AtomId>,
    builder: &mut IrBuilder,
) -> Result<usize, Error> {
    let mut unresolved_bookmarks = 0usize;
    // Bookmarks come from the published view's single strict, bounded parse of `.hg/bookmarks`; this
    // function never reads the file itself.
    for (name, node) in bookmarks {
        match node_to_atom.get(node) {
            Some(target) => {
                builder.add_ref(RefRecord {
                    name: name.clone(),
                    kind: RefKind::Bookmark,
                    target: *target,
                    status: EpistemicStatus::Stated,
                    source: None,
                    annotation: None,
                })?;
            }
            None => unresolved_bookmarks += 1,
        }
    }

    // Named-branch heads (hg branch-head semantics), computed over the served set only: a changeset is
    // a head of its branch when no served child is on the *same* branch.
    let n = changelog.len();
    let mut has_same_branch_child = vec![false; n];
    for crev in 0..n {
        let (Some(e), Some(cb)) = (changelog.entry(crev), branch_of.get(&crev)) else {
            continue;
        };
        for p in [e.p1, e.p2] {
            if p == NULL_REV {
                continue;
            }
            if let Ok(pi) = usize::try_from(p) {
                if branch_of.get(&pi) == Some(cb) {
                    if let Some(slot) = has_same_branch_child.get_mut(pi) {
                        *slot = true;
                    }
                }
            }
        }
    }
    // One ref per branch, at what Mercurial itself resolves the name to (see `branch_tips`).
    let heads = (0..n).filter_map(|crev| {
        if has_same_branch_child.get(crev).copied().unwrap_or(true) {
            return None;
        }
        let branch = branch_of.get(&crev)?;
        Some((crev, branch.as_str(), !closes_branch.contains(&crev)))
    });
    let tip_of = branch_tips(heads);
    for (branch, crev) in tip_of {
        let Some(e) = changelog.entry(crev) else {
            continue;
        };
        if let Some(target) = node_to_atom.get(&e.node) {
            builder.add_ref(RefRecord {
                name: branch.to_string(),
                kind: RefKind::NamedBranch,
                target: *target,
                status: EpistemicStatus::Stated,
                source: None,
                annotation: None,
            })?;
        }
    }
    Ok(unresolved_bookmarks)
}

/// The Mercurial loss boundary (RFC 005 D-5), every drop class-stated (`PR-9`), plus the categories the
/// corrections handoff adds: the published view's two exclusions, unresolvable/uncarried copy sources,
/// bookmarks naming an unimported changeset, and unrepresentable timezone offsets (each omitted when
/// zero).
#[allow(clippy::too_many_arguments)]
fn loss_boundary(
    store: &Path,
    not_published: usize,
    hidden: usize,
    unresolved_copies: usize,
    copy_on_existing_path: usize,
    unresolved_bookmarks: usize,
    unrepresentable_offsets: usize,
) -> LossBoundary {
    let mut dropped = vec![
        DropRecord {
            class: LossClass::Representation,
            what: "revlog physical layout and delta chains".to_string(),
            reason: "representation not assertion; the logical revisions are preserved".to_string(),
        },
        DropRecord {
            class: LossClass::Representation,
            what: "dirstate and working copy".to_string(),
            reason: "local state, not history".to_string(),
        },
    ];
    if store.join("phaseroots").exists() {
        dropped.push(DropRecord {
            class: LossClass::Representation,
            what: "phases (public/draft/secret)".to_string(),
            reason: "local workflow state, not history".to_string(),
        });
    }
    if store.join("obsstore").exists() {
        dropped.push(DropRecord {
            class: LossClass::AdvisoryUnreliable,
            what: "obsolescence markers".to_string(),
            reason: "advisory metadata about rewritten changesets; never promoted to ancestry"
                .to_string(),
        });
    }
    if not_published > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("changesets not published: secret or archived ({not_published})"),
            reason: "not published by the source repository (phase), so not imported: brygge \
                     imports what the repository would publish"
                .to_string(),
        });
    }
    if hidden > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("hidden (obsolete) changesets ({hidden})"),
            reason: "rewritten or pruned by the source's own history editing (obsolescence \
                     markers); not part of the published history"
                .to_string(),
        });
    }
    if unresolved_copies > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("copy sources not resolvable ({unresolved_copies})"),
            reason:
                "the stated copy source could not be placed on an imported changeset; the copy is \
                     omitted rather than placed on a guess"
                    .to_string(),
        });
    }
    if copy_on_existing_path > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("copy metadata on an existing path not carried ({copy_on_existing_path})"),
            reason: "a copy stated on a modify (onto a path that already existed) has no home in the \
                     IR's copy model, which carries a copy's destination as an Add; the literal Modify \
                     is carried, the copy relationship is not"
                .to_string(),
        });
    }
    if unresolved_bookmarks > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("bookmarks naming unimported changesets ({unresolved_bookmarks})"),
            reason: "the bookmark names a changeset that is not published (secret, hidden) or not \
                     present"
                .to_string(),
        });
    }
    if unrepresentable_offsets > 0 {
        dropped.push(DropRecord {
            class: LossClass::Other,
            what: format!("unrepresentable timezone offsets ({unrepresentable_offsets})"),
            reason: "the stored offset does not divide evenly into minutes, or does not fit the IR's \
                     16-bit minutes-east representation; the time itself is still carried, the offset \
                     is absent"
                .to_string(),
        });
    }
    LossBoundary { dropped }
}

#[cfg(test)]
mod tests;

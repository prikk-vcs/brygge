//! The decode orchestration (RFC 005 D-2/D-3/D-5): read a Mercurial store and build a [`brygge_ir::Ir`],
//! mostly *Stated*, with **source-recorded renames carried as `Stated`** (SRC-H2).
//!
//! Changelog revisions are already in parent-first order (parents have lower revs), so we iterate them
//! directly. Per changeset: diff its manifest against its first parent's into literal `PathOp`s, read
//! file content and any `copy:` metadata from the filelogs, and mark a source-recorded copy/rename as a
//! `Stated` [`RenameHint`] beside the literal ops.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    AtomId, DropRecord, Identity, ImportProvenance, Ir, LossBoundary, LossClass, MetadataClaims,
    PathOp, RefKind, RefRecord, RenameHint, SourceIdentity, SourceKind,
};
use brygge_ir::status::EpistemicStatus;

use crate::revlog::{NULL_REV, Revlog};
use crate::{Error, Options, changelog, decoder_version, filelog, fncache, manifest, requires};

const DECODER: &str = "brygge-decode-hg";

/// Decode the Mercurial repository at `path` (its working root or `.hg` directory) into an [`Ir`].
///
/// # Errors
/// [`Error::Open`] if the path is not a Mercurial repository; [`Error::UnsupportedFormat`] /
/// [`Error::FloorRefusal`] for a refused format or feature; [`Error::Read`] on a malformed store;
/// [`Error::Ir`] on an IR invariant violation.
pub fn decode(path: &Path, opts: &Options) -> Result<Ir, Error> {
    let (root, store) = locate(path)?;
    check_requirements(&root, &store)?;

    let changelog = Revlog::open(&store.join("00changelog.i"))?;
    let manifest_log = Revlog::open(&store.join("00manifest.i"))?;

    let repo_id = changelog
        .entry(0)
        .map(|e| e.node.to_vec())
        .unwrap_or_default();

    let provenance = ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Hg,
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

    if changelog.is_empty() {
        builder.set_loss(loss_boundary(&store));
        return builder.finish().map_err(Error::Ir);
    }

    // manifest node -> manifest revlog revision
    let mut manifest_node_to_rev: HashMap<[u8; 20], usize> = HashMap::new();
    for mrev in 0..manifest_log.len() {
        if let Some(e) = manifest_log.entry(mrev) {
            manifest_node_to_rev.insert(e.node, mrev);
        }
    }

    let mut filelogs: HashMap<String, Revlog> = HashMap::new();
    let mut node_to_atom: HashMap<[u8; 20], AtomId> = HashMap::new();
    let mut branch_of: Vec<String> = Vec::with_capacity(changelog.len());

    for crev in 0..changelog.len() {
        let cs = changelog::parse(&changelog.revision(crev)?)?;
        branch_of.push(cs.branch.clone());
        let this_manifest = manifest_of(&manifest_log, &manifest_node_to_rev, &cs.manifest_node)?;

        if this_manifest.contains_key(".hgsub") || this_manifest.contains_key(".hgsubstate") {
            return Err(Error::FloorRefusal {
                feature: "subrepo".to_string(),
                reason:
                    "Mercurial subrepositories are refused rather than approximated (RFC 005 D-4, \
                         parity with the Git submodule floor)"
                        .to_string(),
            });
        }

        let entry = changelog
            .entry(crev)
            .ok_or_else(|| Error::Read("changelog entry vanished".to_string()))?;
        let parent_manifest = if entry.p1 == NULL_REV {
            BTreeMap::new()
        } else {
            let pcs = changelog::parse(&changelog.revision(entry.p1 as usize)?)?;
            manifest_of(&manifest_log, &manifest_node_to_rev, &pcs.manifest_node)?
        };

        let (ops, rename_hints) = diff_manifests(
            &store,
            &mut filelogs,
            &mut builder,
            &parent_manifest,
            &this_manifest,
        )?;

        let metadata = MetadataClaims {
            author: Some(identity(&cs.user)),
            committer: Some(identity(&cs.user)),
            message: Some(cs.description.clone()),
            author_time: Some(cs.time),
            commit_time: Some(cs.time),
        };
        let parents = parent_atoms(&changelog, crev, &node_to_atom)?;

        let atom_id = builder.add_atom(AtomDraft {
            parents,
            ops,
            rename_hints,
            metadata,
            source: SourceIdentity {
                kind: SourceKind::Hg,
                repo_id: repo_id.clone(),
                atom_id: entry.node.to_vec(),
                signatures: Vec::new(),
            },
            status: EpistemicStatus::Stated,
        });
        node_to_atom.insert(entry.node, atom_id);
    }

    add_refs(&root, &changelog, &branch_of, &node_to_atom, &mut builder)?;
    builder.set_loss(loss_boundary(&store));
    builder.finish().map_err(Error::Ir)
}

/// Resolve the repository root and its `.hg/store` directory.
fn locate(path: &Path) -> Result<(PathBuf, PathBuf), Error> {
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

/// Read and check `.hg/requires` (and `.hg/store/requires` under share-safe) — the format-safety gate.
fn check_requirements(root: &Path, store: &Path) -> Result<(), Error> {
    let mut body = std::fs::read_to_string(root.join(".hg").join("requires")).unwrap_or_default();
    let store_req = store.join("requires");
    if store_req.exists() {
        body.push('\n');
        body.push_str(&std::fs::read_to_string(&store_req).unwrap_or_default());
    }
    requires::check(&body)
}

/// Parse the manifest a changeset points at.
fn manifest_of(
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

/// Diff `parent` → `child` manifests into literal `Stated` path operations, plus `Stated` rename hints
/// for source-recorded copies (SRC-H2).
fn diff_manifests(
    store: &Path,
    filelogs: &mut HashMap<String, Revlog>,
    builder: &mut IrBuilder,
    parent: &BTreeMap<String, manifest::Entry>,
    child: &BTreeMap<String, manifest::Entry>,
) -> Result<(Vec<PathOp>, Vec<RenameHint>), Error> {
    let mut ops = Vec::new();
    let mut hints = Vec::new();

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
        let blob = builder.add_blob(file.content);
        if is_add {
            ops.push(PathOp::Add {
                path: path.clone(),
                blob,
                mode: entry.mode,
                status: EpistemicStatus::Stated,
            });
            // A copy source recorded on an added file is a stated rename/copy (SRC-H2): carried Stated,
            // beside the literal Add (and the Delete of the source, if the source was removed).
            if let Some(from) = file.copy_from {
                hints.push(RenameHint {
                    from,
                    to: path.clone(),
                    status: EpistemicStatus::Stated,
                });
            }
        } else {
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
    Ok((ops, hints))
}

/// Read a file revision's content and copy source, caching open filelogs by path.
fn read_file(
    store: &Path,
    filelogs: &mut HashMap<String, Revlog>,
    path: &str,
    filenode: &[u8; 20],
) -> Result<filelog::FileRev, Error> {
    if !filelogs.contains_key(path) {
        let sp = fncache::store_path(path)?;
        let rl = Revlog::open(&store.join(sp))?;
        filelogs.insert(path.to_string(), rl);
    }
    let rl = filelogs
        .get(path)
        .ok_or_else(|| Error::Read("filelog cache miss".to_string()))?;
    filelog::read_revision(rl, filenode)
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

/// Parse an hg user string (`"Name <email>"`) into an [`Identity`] claim.
fn identity(user: &str) -> Identity {
    if let Some((name, rest)) = user.rsplit_once('<') {
        if let Some((email, _)) = rest.split_once('>') {
            return Identity {
                name: name.trim().to_string(),
                email: email.to_string(),
            };
        }
    }
    Identity {
        name: user.trim().to_string(),
        email: String::new(),
    }
}

/// Add bookmarks (`Bookmark`) and named-branch heads (`NamedBranch`) as refs (RFC 005 D-4).
fn add_refs(
    root: &Path,
    changelog: &Revlog,
    branch_of: &[String],
    node_to_atom: &HashMap<[u8; 20], AtomId>,
    builder: &mut IrBuilder,
) -> Result<(), Error> {
    // Bookmarks: `.hg/bookmarks`, lines "<40-hex-node> <name>".
    let bookmarks_path = root.join(".hg").join("bookmarks");
    if let Ok(body) = std::fs::read_to_string(&bookmarks_path) {
        for line in body.lines() {
            if let Some((node_hex, name)) = line.trim().split_once(' ') {
                if let Ok(node) = crate::util::parse_hex20(node_hex) {
                    if let Some(target) = node_to_atom.get(&node) {
                        builder.add_ref(RefRecord {
                            name: name.trim().to_string(),
                            kind: RefKind::Bookmark,
                            target: *target,
                            status: EpistemicStatus::Stated,
                            source: None,
                        })?;
                    }
                }
            }
        }
    }

    // Named-branch heads (hg branch-head semantics): a changeset is a head of its branch when no child
    // is on the *same* branch. This labels each branch's tip(s) — including a tip whose only child is on
    // a different branch (which a pure topological-head test would miss).
    let n = changelog.len();
    let mut has_same_branch_child = vec![false; n];
    for crev in 0..n {
        let (Some(e), Some(cb)) = (changelog.entry(crev), branch_of.get(crev)) else {
            continue;
        };
        for p in [e.p1, e.p2] {
            if p == NULL_REV {
                continue;
            }
            if let Ok(pi) = usize::try_from(p) {
                if branch_of.get(pi) == Some(cb) {
                    if let Some(slot) = has_same_branch_child.get_mut(pi) {
                        *slot = true;
                    }
                }
            }
        }
    }
    for crev in 0..n {
        if has_same_branch_child.get(crev).copied().unwrap_or(true) {
            continue;
        }
        let (Some(e), Some(branch)) = (changelog.entry(crev), branch_of.get(crev)) else {
            continue;
        };
        if let Some(target) = node_to_atom.get(&e.node) {
            builder.add_ref(RefRecord {
                name: branch.clone(),
                kind: RefKind::NamedBranch,
                target: *target,
                status: EpistemicStatus::Stated,
                source: None,
            })?;
        }
    }
    Ok(())
}

/// The Mercurial loss boundary (RFC 005 D-5), every drop class-stated (`PR-9`).
fn loss_boundary(store: &Path) -> LossBoundary {
    let mut dropped = vec![
        DropRecord {
            class: LossClass::Representation,
            what: "revlog physical layout and delta chains".to_string(),
            reason: "representation not assertion; the logical revisions are preserved (PR-7)"
                .to_string(),
        },
        DropRecord {
            class: LossClass::Representation,
            what: "dirstate and working copy".to_string(),
            reason: "local state, not history (PR-7)".to_string(),
        },
    ];
    if store.join("phaseroots").exists() {
        dropped.push(DropRecord {
            class: LossClass::Representation,
            what: "phases (public/draft/secret)".to_string(),
            reason: "local workflow state, not history (PR-7, RFC 005 D-5)".to_string(),
        });
    }
    if store.join("obsstore").exists() {
        dropped.push(DropRecord {
            class: LossClass::AdvisoryUnreliable,
            what: "obsolescence markers".to_string(),
            reason: "advisory metadata about rewritten changesets; never promoted to ancestry \
                     (PR-8, RFC 005 D-5)"
                .to_string(),
        });
    }
    LossBoundary { dropped }
}

#[cfg(test)]
mod tests;

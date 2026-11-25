//! The running repository-tree model (RFC 006 D-1/§5b; RFC 020 D-1/D-2) — the SVN-specific core.
//!
//! SVN dumps a directory copy *implicitly* (it does not re-list the copied subtree), so the decoder keeps
//! a live `path → file` snapshot and evolves it revision by revision, expanding directory copies from the
//! historical snapshot they name. The path operations for a revision come from **diffing** the tree before
//! and after — which yields one op per path and so sidesteps the `replace`-as-two-same-path-ops ordering
//! trap the handoff warned of (RFC 006 D-3). Copies (`copyfrom`) are carried as `Stated` [`CopyRecord`]s
//! beside the diff (SRC-S3), with the **correct** `from_atom` (RFC 011 §5): each copy names the
//! already-assigned atom for its `copyfrom-rev`, via `revnum_to_atom`.
//!
//! Only files are stored; a directory exists implicitly when files sit under it, and an empty directory
//! has no IR representation (dropped-with-record) — the IR has no empty-dir entity, like Git/prikk.
//!
//! A path SVN states as **replaced** (`Node-action: replace`) gets `PathOp::Replace`, even when its
//! content is unchanged, because the source stated a new node at that path (CR-07.2, RFC 011 D-7) — never
//! folded into `Modify`, and never silently dropped when identical.
//!
//! ## The tree, shared (RFC 020 D-1)
//!
//! A snapshot is [`Tree`], a cheap handle (`Rc<Dir>` underneath) to a **persistent** trie: [`Dir`] holds
//! this level's files and its subdirectories, each subdirectory itself an `Rc<Dir>`. Cloning a [`Tree`] —
//! what every retained snapshot and every revision's rolling `prev` does — is an `Rc::clone`, O(1)
//! regardless of the tree's size. A change (`Tree::insert`/`Tree::remove_subtree`) copies only the
//! directories on its own path (`Rc::make_mut`): every subtree it does not touch stays shared with
//! whichever other snapshot (a `kept` retained tree, the previous revision's tree) still holds it. A
//! directory copy (`svn cp <src>@<rev> <dst>`) inserts the source subtree's `Rc` at `<dst>` directly
//! (`Tree::dir_at` + `Tree::flat_entries`): the lookup is O(depth), not O(files) — the **stated** layer
//! still records one `Add` and one `CopyRecord` per copied file, unchanged (the artifact must not
//! change), so that expansion still costs one op per file, same as before.
//!
//! **No `Rc` cycle is possible:** a copy always inserts a subtree taken from an *earlier* revision's
//! snapshot into a path of the *current*, newer tree being built. The current tree can therefore never
//! appear inside its own history of subtrees — the graph of "subtree reachable from" is a DAG by
//! construction, never a cycle, so `Rc::make_mut`'s ordinary copy-on-write is sound without any cycle
//! bookkeeping.
//!
//! **The ordering pitfall (RFC 020 batch S's handoff):** a flat `BTreeMap<String, _>` orders *whole
//! paths* by byte value, so `"a.c"` (`.` = `0x2E`) sorts before `"a/b"` (`/` = `0x2F`). A trie walked
//! component by component, naively comparing a directory's bare name against a sibling file's name,
//! gets this backwards: comparing `"a"` (the directory) against `"a.c"` (the file) as bare strings puts
//! `"a"` first, the opposite of the flat order. [`Tree::flat_entries`] (and everything built on it) merges
//! files and subdirectories by comparing a file's name against a subdirectory's name *with a trailing
//! `/` appended* (not the bare name) — `cmp_file_vs_dir` — which reproduces the flat map's own byte order
//! exactly. [`IrBuilder::add_atom`] re-sorts every atom's `ops` and `copies` canonically by path before
//! storing them regardless (so the *artifact* never depended on this), but [`Dir`]'s own iteration order
//! is still made to match the flat map's, both because the level-1 property test below holds the two to
//! that standard directly, and so that no future consumer can be tripped up by it.
//!
//! **Why `max_tree_entries` no longer gates an ordinary file add (RFC 020 D-2):** before this, *every*
//! `apply_file`/`apply_dir` insertion checked the live tree's total size, because a flat clone-per-copy
//! made the live tree's size the only thing standing between an adversarial dump and unbounded memory. A
//! shared tree's logical size costs nothing extra to *hold* — what can still cost a lot, in one step, is
//! **expanding** a tree into entries: a directory copy's per-file `Add`+`CopyRecord` expansion
//! ([`apply_dir`]), and `--branch-lines`'s own per-file walk when a new line's first atom is built from a
//! copy source (`crate::branch_lines::diff_relative`). `max_tree_entries` is checked at exactly those two
//! points now (against the *resulting* live/relative tree's size), not on every ordinary single-file
//! change; ordinary growth is bounded elsewhere (`max_dump_bytes`, `max_node_text_bytes`, `max_ir_ops`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use brygge_ir::BlobId;
use brygge_ir::builder::IrBuilder;
use brygge_ir::model::{AtomId, CopyRecord, PathOp};
use brygge_ir::status::EpistemicStatus;

use crate::dumpstream::{
    Checksums, NodeAction, NodeKind, NodeRecord, PropBlock, PropChange, TextBody,
};
use crate::source::Limits;
use crate::{Error, checksum, props, svndiff};

/// A file in the tree: its content address, and its two mode properties.
///
/// The two mode properties are kept **separately**, not only as the derived `mode`: a file can carry both
/// `svn:executable` and `svn:special`, and a property delta that removes `svn:special` must leave it
/// executable. `mode` is the derived IR mode ([`props::mode_of`]) and is what the diff compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Content address in the IR store. For an `svn:special` file this is the bare link target, not SVN's
    /// text (`link <target>`); [`svn_text`] gives SVN's text.
    pub blob: BlobId,
    /// IR file mode (regular / executable / symlink), derived from the two flags.
    pub mode: u32,
    /// `svn:executable` is set.
    pub exec: bool,
    /// `svn:special` is set (the file is a symlink).
    pub special: bool,
}

// ---- the shared tree (RFC 020 D-1) ------------------------------------------------------------------

/// One directory level: its own files, and its subdirectories by name. Reachable only through a
/// [`Tree`]'s `Rc`, so every subtree not on a change's own path is shared, never copied, by every
/// snapshot that holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Dir {
    files: BTreeMap<String, FileEntry>,
    dirs: BTreeMap<String, Rc<Dir>>,
}

impl Dir {
    /// Every `(path, FileEntry)` under this directory, `path` relative to it and with `prefix`
    /// prepended, in **flat map order** (the module doc's ordering pitfall) — files and subdirectories
    /// merged by [`cmp_file_vs_dir`], not by comparing bare names.
    ///
    /// **Two collisions, not one:** a file's bare name can collide with a sibling *directory's* name
    /// (handled by [`cmp_file_vs_dir`]) — but two sibling *directories* can collide with each other the
    /// same way (`"a"` and `"a.c"` as directory names: `self.dirs`, a `BTreeMap<String, _>`, orders them
    /// by the bare names, `"a"` before `"a.c"`, but the flat order compares `"a/"` against `"a.c/"`,
    /// which disagrees whenever one name is a prefix of the other — exactly `"a"`/`"a.c"`'s case). So
    /// `self.dirs` is first re-sorted by its *own* effective key (name with `/` appended) before it is
    /// merged against `self.files`, which needs no such correction (two file names are never compared
    /// with an implicit suffix on either side).
    fn flat_entries_into(&self, prefix: &str, out: &mut Vec<(String, FileEntry)>) {
        let mut files = self.files.iter().peekable();
        let mut dirs: Vec<(&String, &Rc<Dir>)> = self.dirs.iter().collect();
        dirs.sort_by(|(a, _), (b, _)| cmp_dir_vs_dir(a, b));
        let mut dirs = dirs.into_iter().peekable();
        loop {
            match (files.peek().copied(), dirs.peek().copied()) {
                (None, None) => break,
                (Some((fname, fentry)), None) => {
                    out.push((format!("{prefix}{fname}"), fentry.clone()));
                    files.next();
                }
                (None, Some((dname, dsub))) => {
                    dsub.flat_entries_into(&format!("{prefix}{dname}/"), out);
                    dirs.next();
                }
                (Some((fname, fentry)), Some((dname, dsub))) => {
                    if cmp_file_vs_dir(fname, dname) == std::cmp::Ordering::Less {
                        out.push((format!("{prefix}{fname}"), fentry.clone()));
                        files.next();
                    } else {
                        dsub.flat_entries_into(&format!("{prefix}{dname}/"), out);
                        dirs.next();
                    }
                }
            }
        }
    }
}

/// Compare a file's bare name against a subdirectory's bare name **as if the directory's name had `/`
/// appended** — the one comparison that makes a component-wise trie walk agree with the flat map's own
/// byte order (the module doc's ordering pitfall: `.` = `0x2E` sorts before `/` = `0x2F`, so `"a.c"` <
/// `"a/b"` as full paths, which comparing bare `"a.c"` against bare `"a"` would get backwards).
fn cmp_file_vs_dir(file_name: &str, dir_name: &str) -> std::cmp::Ordering {
    file_name
        .bytes()
        .cmp(dir_name.bytes().chain(std::iter::once(b'/')))
}

/// Compare two sibling subdirectories' bare names **as if both had `/` appended** — the same
/// correction as [`cmp_file_vs_dir`], needed because two directory names can have the same prefix
/// relationship a file and a directory name can (`"a"` and `"a.c"`): comparing them bare, as
/// `self.dirs`'s `BTreeMap` does on its own, disagrees with the flat map's order in exactly that case.
fn cmp_dir_vs_dir(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes()
        .chain(std::iter::once(b'/'))
        .cmp(b.bytes().chain(std::iter::once(b'/')))
}

/// Split `path` into its first component and the rest (`None` if `path` has no further `/`).
fn split_first(path: &str) -> (&str, Option<&str>) {
    match path.split_once('/') {
        Some((first, rest)) => (first, Some(rest)),
        None => (path, None),
    }
}

/// Split `path` into its directory components (joined, `""` if `path` has only one component) and its
/// last component — the walk [`Tree::insert`]/[`Tree::insert_unbudgeted`] need: every component but the
/// last is a directory to descend into (creating it if absent), and the last is the file's own name.
fn split_last(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((dirs, last)) => (dirs, last),
        None => ("", path),
    }
}

/// A snapshot of the repository tree: a cheap (`Rc::clone`, O(1)) handle to a persistent trie (RFC 020
/// D-1 — see the module doc). `BTreeMap`/`Rc` from `std` only: no new dependency, no `unsafe`, and no
/// `Arc` (the decoder is single-threaded).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree(Rc<Dir>);

impl Tree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The file at `path`, if any. `path` never names a directory: only files are stored (the module
    /// doc).
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&FileEntry> {
        let (first, rest) = split_first(path);
        match rest {
            None => self.0.files.get(first),
            Some(rest) => self.0.dirs.get(first).and_then(|d| dir_get(d, rest)),
        }
    }

    /// Whether a **file** exists at exactly `path` (never true for a path that is only a directory — the
    /// module doc: only files are stored).
    #[must_use]
    pub fn contains_key(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    /// Whether `path` names a file, or a directory with at least one file somewhere under it — i.e.
    /// whether anything at all is "live" at or under `path` (CR-07.3's live-ref check, CR-07.5's
    /// layout-honesty count). Relies on the invariant that an existing [`Dir`] node always holds at
    /// least one file transitively (nothing ever inserts an empty directory node, and
    /// [`Tree::remove_subtree`] prunes one back out the moment it becomes empty) — so a subtree's mere
    /// existence already means it is live.
    #[must_use]
    pub fn is_live_under(&self, prefix: &str) -> bool {
        self.contains_key(prefix) || self.dir_at(prefix).is_some()
    }

    /// The subtree at `path` (a directory, by SVN convention — the module doc), as its own cheap
    /// [`Tree`] handle, or `None` if `path` names nothing (an empty directory copy, or a path absent
    /// from this snapshot — both are a copy of nothing, not an error; see [`crate::tree::apply_dir`]).
    #[must_use]
    pub fn dir_at(&self, path: &str) -> Option<Tree> {
        let mut node = &self.0;
        for comp in path.split('/') {
            node = node.dirs.get(comp)?;
        }
        Some(Tree(Rc::clone(node)))
    }

    /// Every `(path, FileEntry)` in this tree, in flat map order (the module doc's ordering pitfall).
    #[must_use]
    pub fn flat_entries(&self) -> Vec<(String, FileEntry)> {
        let mut out = Vec::new();
        self.0.flat_entries_into("", &mut out);
        out
    }

    /// The names of the directory at `path`'s own immediate children — both files and subdirectories,
    /// one level only, never recursing into them. Empty if `path` names nothing, or names a file (which
    /// has no children of its own).
    ///
    /// RFC 020 review 169 F-1: `--branch-lines`'s live-root set under a layout container (`branches`,
    /// `tags`) comes from here — O(the container's own child count) — never from flattening the whole
    /// repository tree to find which roots still exist, which made that detection cost O(tree size) on
    /// every revision (quadratic over a tag-heavy history, since the tree grows with every tag).
    #[must_use]
    pub(crate) fn child_names(&self, path: &str) -> Vec<String> {
        let Some(dir) = self.dir_at(path) else {
            return Vec::new();
        };
        let mut names: Vec<String> = dir.0.files.keys().cloned().collect();
        names.extend(dir.0.dirs.keys().cloned());
        names
    }

    /// Insert `entry` at `path`, copying every directory on the path (`Rc::make_mut`) and creating any
    /// that do not yet exist. Every `Dir` node actually cloned, and every brand-new `Dir` node or file
    /// entry created, is charged against [`Limits::max_tree_nodes`] via `budget` (RFC 020 D-2) —
    /// cumulative, never decreasing, so it bounds the total work the tree has ever done, not its live
    /// size. `budget`'s live-entry count ([`Budget::live_entries`]) is updated too, so a later caller
    /// (a directory copy's expansion) can cheaply ask "how big is the tree right now" without a full
    /// walk.
    ///
    /// # Errors
    /// [`Error::ResourceLimit`] if this would cross `max_tree_nodes`.
    pub fn insert(
        &mut self,
        path: &str,
        entry: FileEntry,
        budget: &mut Budget<'_>,
    ) -> Result<(), Error> {
        let (dirs, last) = split_last(path);
        let mut node = &mut self.0;
        if !dirs.is_empty() {
            for comp in dirs.split('/') {
                charge_clone(node, budget)?;
                let d = Rc::make_mut(node);
                if !d.dirs.contains_key(comp) {
                    budget.charge_node()?;
                }
                node = d.dirs.entry(comp.to_string()).or_default();
            }
        }
        charge_clone(node, budget)?;
        let d = Rc::make_mut(node);
        let is_new = !d.files.contains_key(last);
        if is_new {
            budget.charge_node()?;
            budget.live_entries = budget.live_entries.saturating_add(1);
        }
        d.files.insert(last.to_string(), entry);
        Ok(())
    }

    /// [`Tree::insert`], without charging any ceiling (`crate::branch_lines`'s own per-root partition,
    /// RFC 020 D-1: a *secondary*, derived structure whose total insertions are already bounded by the
    /// whole-repository tree's own, budget-checked construction — every entry it ever holds came from
    /// that tree's `flat_entries`, so charging again here would double-count the same work without
    /// bounding anything new). Still an `Rc::make_mut` walk, so an unrelated root's subtree stays
    /// shared; just not a chargeable one.
    pub(crate) fn insert_unbudgeted(&mut self, path: &str, entry: FileEntry) {
        let (dirs, last) = split_last(path);
        let mut node = &mut self.0;
        if !dirs.is_empty() {
            for comp in dirs.split('/') {
                let d = Rc::make_mut(node);
                node = d.dirs.entry(comp.to_string()).or_default();
            }
        }
        let d = Rc::make_mut(node);
        d.files.insert(last.to_string(), entry);
    }

    /// Remove the file at exactly `path`, and (SVN delete-of-a-directory semantics) everything under
    /// `path/...` if `path` is also a directory. Returns the number of file entries actually removed
    /// (`budget.live_entries` is decremented by the same amount). A path absent from this tree removes
    /// nothing and charges nothing — only an actual clone or structural change is chargeable work.
    ///
    /// # Errors
    /// [`Error::ResourceLimit`] if a `Dir` node clone this requires would cross `max_tree_nodes`.
    pub fn remove_subtree(&mut self, path: &str, budget: &mut Budget<'_>) -> Result<usize, Error> {
        let removed = remove_at(&mut self.0, path, budget)?;
        budget.live_entries = budget.live_entries.saturating_sub(removed);
        Ok(removed)
    }
}

/// Internal, borrow-friendly lookup used by [`Tree::get`]'s dir-walk branch.
fn dir_get<'a>(dir: &'a Rc<Dir>, path: &str) -> Option<&'a FileEntry> {
    let (first, rest) = split_first(path);
    match rest {
        None => dir.files.get(first),
        Some(rest) => dir.dirs.get(first).and_then(|d| dir_get(d, rest)),
    }
}

/// If `Rc::make_mut(node)` is about to clone (strong count > 1), charge one node against the budget
/// first — the clone itself is unavoidable once called, so the charge (and its possible refusal) happens
/// before it, not after.
fn charge_clone(node: &Rc<Dir>, budget: &mut Budget<'_>) -> Result<(), Error> {
    if Rc::strong_count(node) > 1 {
        budget.charge_node()?;
    }
    Ok(())
}

/// [`Tree::remove_subtree`]'s own recursion.
fn remove_at(node: &mut Rc<Dir>, path: &str, budget: &mut Budget<'_>) -> Result<usize, Error> {
    let (first, rest) = split_first(path);
    match rest {
        None => {
            if !node.files.contains_key(first) && !node.dirs.contains_key(first) {
                return Ok(0);
            }
            charge_clone(node, budget)?;
            let d = Rc::make_mut(node);
            let mut removed = 0usize;
            if d.files.remove(first).is_some() {
                removed = removed.saturating_add(1);
            }
            if let Some(sub) = d.dirs.remove(first) {
                removed = removed.saturating_add(count_entries(&sub));
            }
            Ok(removed)
        }
        Some(rest) => {
            if !node.dirs.contains_key(first) {
                return Ok(0);
            }
            charge_clone(node, budget)?;
            let d = Rc::make_mut(node);
            let Some(child) = d.dirs.get_mut(first) else {
                // Checked present just above `make_mut`, which does not change the key set — stays a
                // defined `Ok(0)` rather than an `unreachable!`/`expect` if that invariant is ever wrong.
                return Ok(0);
            };
            let removed = remove_at(child, rest, budget)?;
            if removed > 0 && child.files.is_empty() && child.dirs.is_empty() {
                // Keep the invariant `Tree::is_live_under` relies on: an existing `Dir` node always
                // holds at least one file. No charge: this shrinks the tree, it never clones or creates.
                d.dirs.remove(first);
            }
            Ok(removed)
        }
    }
}

/// The total number of file entries anywhere under `dir` (used only when a whole subtree is dropped at
/// once, to keep the live-entry count exact).
fn count_entries(dir: &Dir) -> usize {
    dir.files.len() + dir.dirs.values().map(|d| count_entries(d)).sum::<usize>()
}

/// The resource accounting for one decode: the ceilings, and how much the deltas have reconstructed so
/// far (RFC 013 D-2, T-8), and how much the tree has cost so far (RFC 020 D-2).
pub struct Budget<'a> {
    /// The ceilings.
    pub limits: &'a Limits,
    /// The bytes every delta's reconstructed target has taken so far.
    pub reconstructed: usize,
    /// The cumulative count of `Dir` nodes cloned or created, and file entries created, by every
    /// [`Tree::insert`]/[`Tree::remove_subtree`] call so far (RFC 020 D-2) — never decreasing, so it
    /// bounds the tree's total lifetime work, not any one snapshot's live size. **Live ≤ cumulative
    /// always** (a node still live was allocated exactly once, and removal never decrements this
    /// counter), so bounding the cumulative count bounds the live size too, with no `Drop` accounting
    /// needed to prove it. Checked at each allocation ([`Budget::charge_node`]).
    tree_nodes: usize,
    /// The current live tree's total file count, maintained incrementally by every
    /// [`Tree::insert`]/[`Tree::remove_subtree`] call (RFC 020 D-2) — an O(1) answer to "how big is the
    /// tree right now", read only at the points that expand a tree into entries (a directory copy's
    /// expansion, `--branch-lines`'s own per-file walk for a new line), never on an ordinary single-file
    /// change (the module doc explains why).
    pub live_entries: usize,
}

impl<'a> Budget<'a> {
    /// A budget with nothing spent.
    #[must_use]
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            reconstructed: 0,
            tree_nodes: 0,
            live_entries: 0,
        }
    }

    /// Charge one node (a `Dir` clone or creation, or a new file entry) against `max_tree_nodes` (RFC 020
    /// D-2), refusing before the cumulative count would cross it.
    ///
    /// # Errors
    /// [`Error::ResourceLimit`] if this would cross `max_tree_nodes`.
    fn charge_node(&mut self) -> Result<(), Error> {
        let next = self.tree_nodes.saturating_add(1);
        if next > self.limits.max_tree_nodes {
            return Err(Error::ResourceLimit {
                what: "the tree nodes brygge has allocated while decoding this dump".to_string(),
                ceiling: format!("{} nodes", self.limits.max_tree_nodes),
            });
        }
        self.tree_nodes = next;
        Ok(())
    }
}

/// SVN's text for an entry: what the dump's checksums and deltas are about. The IR blob of an `svn:special`
/// file is the bare target, and SVN's text is `link ` followed by it (`props::symlink_target` strips exactly
/// that prefix), so this is exactly reversible. **The one place** an entry's SVN text is made: every delta
/// base and every checksum uses it.
///
/// # Errors
/// [`Error::Read`] if the blob is not in the builder (a decoder bug, not bad input).
pub fn svn_text(entry: &FileEntry, builder: &IrBuilder) -> Result<Vec<u8>, Error> {
    let blob = builder
        .blob(&entry.blob)?
        .ok_or_else(|| Error::Read("a tracked file's content is missing (internal)".to_string()))?;
    if entry.special {
        let mut text = Vec::with_capacity(blob.len().saturating_add(5));
        text.extend_from_slice(b"link ");
        text.extend_from_slice(&blob);
        Ok(text)
    } else {
        Ok(blob)
    }
}

/// The IR blob for SVN text under a flag: an `svn:special` file's blob is the text without `link `, and a text
/// without it is refused, never guessed (CR-07.1).
fn blob_for(
    svn_text: Vec<u8>,
    special: bool,
    path: &str,
    builder: &mut IrBuilder,
) -> Result<BlobId, Error> {
    let content = if special {
        props::symlink_target(&svn_text)
            .ok_or_else(|| Error::Read(format!("svn:special file without a link target: {path}")))?
    } else {
        svn_text
    };
    Ok(builder.add_blob(content)?)
}

/// Where a revision's copy records go as they are found: the destination-relative pairs the
/// single-spine path reads directly, and each one's source revision, which only `--branch-lines`
/// reads (to re-resolve `from_atom` against a root's own line history instead of a single per-revision
/// atom). Bundled into one parameter so `apply_file`/`apply_dir` stay under the usual argument count.
struct CopyOut<'a> {
    copies: &'a mut Vec<CopyRecord>,
    source_revs: &'a mut Vec<u64>,
}

/// Review 151 D-1: within one revision, a node that **deletes or replaces** a path `P` voids every
/// earlier copy record of this revision whose `to` is `P` or under `P`. SVN's own meaning: the explicit
/// delete-then-add (or delete) restates the path, so an earlier record from a directory copy's snapshot
/// expansion no longer describes the result. So each destination keeps exactly one record per revision:
/// the last node's. Without this, a directory copy whose dump also restates a file inside it as its own
/// copy (same source, or a different one) left two records for the same destination — a non-canonical
/// duplicate when the sources agreed, and a false second copy claim when they did not (confirmed on a
/// real repository: codeblocks r5310).
fn void_copies_under(copy_out: &mut CopyOut<'_>, path: &str) {
    let prefix = format!("{path}/");
    let kept: Vec<(CopyRecord, u64)> = std::mem::take(copy_out.copies)
        .into_iter()
        .zip(std::mem::take(copy_out.source_revs))
        .filter(|(c, _)| !(c.to == *path || c.to.starts_with(&prefix)))
        .collect();
    for (copy, rev) in kept {
        copy_out.copies.push(copy);
        copy_out.source_revs.push(rev);
    }
}

/// The outcome of applying one revision's node records.
pub struct Applied {
    /// The tree after the revision.
    pub tree: Tree,
    /// The literal path operations (diff of before → after), all `Stated`.
    pub ops: Vec<PathOp>,
    /// Source-recorded copies as `Stated` copy records, each naming the correct source atom.
    pub copies: Vec<CopyRecord>,
    /// Each `copies` entry's source revision (`copyfrom-rev`), positionally paired — RFC 019 Amendment
    /// A: `--branch-lines` has no single per-revision atom to resolve `from_atom` against, so it
    /// re-resolves each copy against the source path's own root-line atom at this revision instead. The
    /// single-spine path (this struct's only other reader) ignores it.
    pub copy_source_revs: Vec<u64>,
    /// Which loss categories the revision's node properties touched.
    pub prop_loss: props::PropLoss,
    /// How many directories added or replaced this revision (by `add`, `replace` or copy) ended up with
    /// no file under them after the whole revision was applied (CR-07.4; review 010 R-4 for `replace`).
    pub empty_dirs: usize,
}

/// Apply a revision's node records to the previous tree, producing the new tree, the diff ops, and the
/// stated copy records. `revnum_to_atom` maps every already-processed revision to its atom id, so a
/// `copyfrom-rev` resolves to the correct `from_atom` (RFC 011 §5) regardless of how far back it points.
///
/// # Errors
/// [`Error::FloorRefusal`] for a refused property; [`Error::Read`] for a copyfrom that names a missing
/// revision/path or a malformed change, or a symlink whose content is not `link <target>`; or
/// [`Error::ResourceLimit`] (RFC 020 D-2) if a directory copy's expansion would cross `max_tree_entries`
/// or `max_copy_expansion`, or the tree's cumulative node allocations would cross `max_tree_nodes`.
pub fn apply_revision(
    prev: &Tree,
    nodes: Vec<NodeRecord>,
    kept: &HashMap<u64, Tree>,
    revnum_to_atom: &HashMap<u64, AtomId>,
    builder: &mut IrBuilder,
    budget: &mut Budget<'_>,
) -> Result<Applied, Error> {
    // Pass 1 (before any mutation): which paths this revision replaced (CR-07.2), and which directories
    // it added (CR-07.4) — both need the tree's *before* state, and the replaced set also needs the
    // *after* state, computed below once the revision has been fully applied.
    let mut replaced_paths: HashSet<String> = HashSet::new();
    let mut replaced_dirs: Vec<String> = Vec::new();
    let mut added_dirs: Vec<String> = Vec::new();
    for node in &nodes {
        match (node.kind, node.action) {
            (Some(NodeKind::File), NodeAction::Replace) => {
                replaced_paths.insert(node.path.clone());
            }
            (Some(NodeKind::Dir), NodeAction::Replace) => {
                collect_subtree_paths(prev, &node.path, &mut replaced_paths);
                replaced_dirs.push(node.path.clone());
                // Review 010 R-4: a replaced directory is a stated directory too — leaving it out of
                // empty-dir accounting would drop it with no record if it ends up empty.
                added_dirs.push(node.path.clone());
            }
            (Some(NodeKind::Dir), NodeAction::Add) => {
                added_dirs.push(node.path.clone());
            }
            _ => {}
        }
    }

    let mut tree = prev.clone();
    let mut copies = Vec::new();
    let mut copy_source_revs = Vec::new();
    let mut copy_out = CopyOut {
        copies: &mut copies,
        source_revs: &mut copy_source_revs,
    };
    let mut prop_loss = props::PropLoss::default();

    for node in nodes {
        if let Some(block) = &node.props {
            // The entries the block sets (the whole set for a full block): what is refused and classified.
            let set: props::Owned = match block {
                PropBlock::Full(p) => p.clone(),
                PropBlock::Delta(changes) => changes
                    .iter()
                    .filter_map(|c| match c {
                        PropChange::Set(k, v) => Some((k.clone(), v.clone())),
                        PropChange::Delete(_) => None,
                    })
                    .collect(),
            };
            props::check_externals(&node.path, &set)?;
            let l = props::classify_loss(&set);
            prop_loss.mergeinfo |= l.mergeinfo;
            prop_loss.workflow |= l.workflow;
            prop_loss.custom |= l.custom;
        }
        match node.action {
            NodeAction::Delete => {
                void_copies_under(&mut copy_out, &node.path);
                tree.remove_subtree(&node.path, budget)?;
            }
            NodeAction::Add | NodeAction::Change | NodeAction::Replace => {
                if node.action == NodeAction::Replace {
                    void_copies_under(&mut copy_out, &node.path);
                    tree.remove_subtree(&node.path, budget)?;
                }
                match node.kind {
                    Some(NodeKind::Dir) => {
                        apply_dir(&mut tree, &mut copy_out, kept, revnum_to_atom, node, budget)?;
                    }
                    Some(NodeKind::File) => {
                        apply_file(
                            &mut tree,
                            &mut copy_out,
                            kept,
                            revnum_to_atom,
                            builder,
                            budget,
                            node,
                        )?;
                    }
                    None => {
                        // A kindless change on an existing file is a property-only change; on a
                        // directory (a tracked-only-by-children path) there is nothing to do.
                        if tree.contains_key(&node.path) {
                            apply_file(
                                &mut tree,
                                &mut copy_out,
                                kept,
                                revnum_to_atom,
                                builder,
                                budget,
                                node,
                            )?;
                        }
                    }
                }
            }
        }
    }

    // Pass 2: the *after* half of each replaced directory's file set (CR-07.2), and whether each added
    // directory ended up with any file under it at all (CR-07.4) — both need the final tree.
    for node_path in &replaced_dirs {
        collect_subtree_paths(&tree, node_path, &mut replaced_paths);
    }
    let mut empty_dirs = 0usize;
    for dir in &added_dirs {
        if !tree.is_live_under(dir) {
            empty_dirs = empty_dirs.saturating_add(1);
        }
    }

    let ops = diff(prev, &tree, &replaced_paths);
    Ok(Applied {
        tree,
        ops,
        copies,
        copy_source_revs,
        prop_loss,
        empty_dirs,
    })
}

/// Insert `dir` itself (if it is a file path — directories are not stored) and every file under it into
/// `out`.
fn collect_subtree_paths(tree: &Tree, dir: &str, out: &mut HashSet<String>) {
    if tree.contains_key(dir) {
        out.insert(dir.to_string());
    }
    if let Some(sub) = tree.dir_at(dir) {
        for (rel, _) in sub.flat_entries() {
            out.insert(format!("{dir}/{rel}"));
        }
    }
}

/// Verify one stated checksum against the text it names. A consistency check, not authenticity (the dump is
/// untrusted and states any checksum it likes); it is also the end-to-end check of our own delta application.
fn verify_sum(
    path: &str,
    header: &str,
    stated: Option<&str>,
    text: &[u8],
    hash: fn(&[u8]) -> Result<String, Error>,
) -> Result<(), Error> {
    let Some(want) = stated else {
        return Ok(());
    };
    if !hash(text)?.eq_ignore_ascii_case(want.trim()) {
        return Err(Error::Read(format!(
            "{path}: {header} mismatch; the dump or its base is not what it states"
        )));
    }
    Ok(())
}

/// Apply a file add/change/replace.
///
/// The **base** of a text or property delta (RFC 013 D-2): the copy source at its revision for a node with
/// `Node-copyfrom-*`, the path's current entry for a `change`, and empty text with no properties for an `add`
/// or `replace` without a copy source. The two mode flags come from the node's own property block (a full
/// block: its set; a delta: the base's flags changed by its entries), else the copy source's, else the
/// existing entry's (CR-07.1). The content is SVN's text under the resolved flag: the fulltext, or the
/// reconstruction of a delta against the base's SVN text, then checked against every checksum the node
/// states; a node with no text keeps its base's content, **recomputed** when a property change flips
/// `svn:special` (`RR-svn-special-toggle`): becoming special takes the link target out of `link <target>`,
/// ceasing to be special makes the target the whole text.
fn apply_file(
    tree: &mut Tree,
    copy_out: &mut CopyOut<'_>,
    kept: &HashMap<u64, Tree>,
    revnum_to_atom: &HashMap<u64, AtomId>,
    builder: &mut IrBuilder,
    budget: &mut Budget<'_>,
    node: NodeRecord,
) -> Result<(), Error> {
    let source = match &node.copyfrom {
        Some((crev, cpath)) => Some(lookup_file(kept, *crev, cpath)?),
        None => None,
    };
    let existing = tree.get(&node.path).cloned();
    // What content and flags a node with no text and no block of its own inherits (CR-07.1).
    let inherited = source.as_ref().or(existing.as_ref());
    // The base a delta applies to.
    let delta_base = match (&source, node.action) {
        (Some(src), _) => Some(src),
        (None, NodeAction::Change) => existing.as_ref(),
        (None, _) => None,
    };

    let base_flags = delta_base.map_or((false, false), |b| (b.exec, b.special));
    let (exec, special) = match &node.props {
        Some(PropBlock::Full(np)) => (
            props::has(np, props::SVN_EXECUTABLE),
            props::has(np, props::SVN_SPECIAL),
        ),
        Some(PropBlock::Delta(changes)) => {
            let (mut exec, mut special) = base_flags;
            for change in changes {
                match change {
                    PropChange::Set(k, _) if k == props::SVN_EXECUTABLE => exec = true,
                    PropChange::Set(k, _) if k == props::SVN_SPECIAL => special = true,
                    PropChange::Delete(k) if k == props::SVN_EXECUTABLE => exec = false,
                    PropChange::Delete(k) if k == props::SVN_SPECIAL => special = false,
                    _ => {}
                }
            }
            (exec, special)
        }
        None => inherited.map_or((false, false), |e| (e.exec, e.special)),
    };
    let mode = props::mode_of(exec, special);

    // The node's SVN text, if it carries any; each stated checksum checked on the way.
    let svn_text_of_node: Option<Vec<u8>> = match node.text {
        Some(TextBody::Full(text)) => Some(text),
        Some(TextBody::Delta(delta)) => {
            let base_text = match delta_base {
                Some(b) => svn_text(b, builder)?,
                None if node.action == NodeAction::Change => {
                    return Err(Error::Read(format!(
                        "{}: delta against a base not present in this dump; an incremental or partial \
                         delta dump cannot be decoded alone",
                        node.path
                    )));
                }
                None => Vec::new(),
            };
            verify_base(&node.path, &node.checksums, &base_text)?;
            let remaining = budget
                .limits
                .max_dump_bytes
                .saturating_sub(budget.reconstructed);
            let allowed = budget.limits.max_node_text_bytes.min(remaining);
            let target = svndiff::apply(&delta, &base_text, allowed).map_err(|e| match e {
                svndiff::DiffError::UnsupportedVersion(v) => Error::UnsupportedFormat {
                    what: format!("svndiff version {v}"),
                    reason: format!("{}: this build reads svndiff version 0", node.path),
                },
                svndiff::DiffError::Malformed(m) => {
                    Error::Read(format!("{}: text delta: {m}", node.path))
                }
                svndiff::DiffError::TooLarge { .. } => {
                    if budget.limits.max_node_text_bytes <= remaining {
                        Error::ResourceLimit {
                            what: format!("the text of '{}'", node.path),
                            ceiling: format!("{} bytes", budget.limits.max_node_text_bytes),
                        }
                    } else {
                        Error::ResourceLimit {
                            what: "the text every delta reconstructs, in total".to_string(),
                            ceiling: format!("{} bytes", budget.limits.max_dump_bytes),
                        }
                    }
                }
            })?;
            budget.reconstructed = budget.reconstructed.saturating_add(target.len());
            Some(target)
        }
        None => None,
    };
    if let Some(text) = &svn_text_of_node {
        verify_content(&node.path, &node.checksums, text)?;
    }
    if let (Some(src), Some(_)) = (&source, &svn_text_of_node) {
        // A copy with text states its source's checksum too (`svnadmin`; `svnrdump` writes none).
        let src_text = svn_text(src, builder)?;
        verify_sum(
            &node.path,
            "Text-copy-source-md5",
            node.checksums.copy_md5.as_deref(),
            &src_text,
            checksum::md5_hex,
        )?;
        verify_sum(
            &node.path,
            "Text-copy-source-sha1",
            node.checksums.copy_sha1.as_deref(),
            &src_text,
            checksum::sha1_hex,
        )?;
    }

    let blob = if let Some(text) = svn_text_of_node {
        blob_for(text, special, &node.path, builder)?
    } else if let Some(base) = inherited {
        if base.special == special {
            base.blob
        } else {
            // A property change flipped `svn:special` on a node with no text: SVN's text is unchanged and
            // the IR blob follows the new flag.
            let text = svn_text(base, builder)?;
            blob_for(text, special, &node.path, builder)?
        }
    } else {
        return Err(Error::Read(format!(
            "file '{}' has no content and no copy source",
            node.path
        )));
    };

    if let Some((crev, cpath)) = &node.copyfrom {
        copy_out.copies.push(CopyRecord {
            from: cpath.clone(),
            from_atom: atom_for(revnum_to_atom, *crev)?,
            to: node.path.clone(),
            status: EpistemicStatus::Stated,
        });
        copy_out.source_revs.push(*crev);
    }
    // RFC 020 D-2: an ordinary single-file change is no longer checked against `max_tree_entries` (the
    // module doc explains why) — only `Tree::insert`'s own cumulative `max_tree_nodes` charge applies.
    tree.insert(
        &node.path,
        FileEntry {
            blob,
            mode,
            exec,
            special,
        },
        budget,
    )?;
    Ok(())
}

/// `Text-delta-base-md5` / `-sha1` against the base's SVN text.
fn verify_base(path: &str, sums: &Checksums, base: &[u8]) -> Result<(), Error> {
    verify_sum(
        path,
        "Text-delta-base-md5",
        sums.base_md5.as_deref(),
        base,
        checksum::md5_hex,
    )?;
    verify_sum(
        path,
        "Text-delta-base-sha1",
        sums.base_sha1.as_deref(),
        base,
        checksum::sha1_hex,
    )
}

/// `Text-content-md5` / `-sha1` against the resulting SVN text (fulltext nodes included).
fn verify_content(path: &str, sums: &Checksums, text: &[u8]) -> Result<(), Error> {
    verify_sum(
        path,
        "Text-content-md5",
        sums.content_md5.as_deref(),
        text,
        checksum::md5_hex,
    )?;
    verify_sum(
        path,
        "Text-content-sha1",
        sums.content_sha1.as_deref(),
        text,
        checksum::sha1_hex,
    )
}

/// Apply a directory add/change/replace: expand a copy from its historical snapshot. A directory
/// property-only change, or a plain `add`/`replace` with no copy source, touches no files — CR-07.4's
/// empty-directory accounting happens once, after the whole revision, from the final tree.
///
/// RFC 020 D-1: the source subtree is looked up directly (`Tree::dir_at`, O(depth)), not found by
/// scanning the whole source snapshot — the win over 0.8.1, which cloned the whole tree per
/// copy-source revision instead of sharing it. The expansion itself — one `Add`+`CopyRecord` per copied
/// file — is unchanged and still costs one op per file (the artifact must not change), bounded by
/// `max_copy_expansion` as before, and now also by `max_tree_entries` against the *resulting* live tree
/// size (D-2: this is one of the two places that still checks it).
fn apply_dir(
    tree: &mut Tree,
    copy_out: &mut CopyOut<'_>,
    kept: &HashMap<u64, Tree>,
    revnum_to_atom: &HashMap<u64, AtomId>,
    node: NodeRecord,
    budget: &mut Budget<'_>,
) -> Result<(), Error> {
    let Some((crev, cpath)) = &node.copyfrom else {
        return Ok(());
    };
    let src = snapshot_for(kept, *crev)?;
    let from_atom = atom_for(revnum_to_atom, *crev)?;
    // Reproduces exactly what scanning the whole source tree for `spath == cpath || spath starts_with
    // "{cpath}/"` would collect — just without the whole-tree scan: a direct file at `cpath` itself
    // (possible in principle, since files and directories are independent maps at every level — see the
    // module doc), plus every file under the `cpath` subtree, if one exists.
    let mut additions: Vec<(String, FileEntry)> = Vec::new();
    if let Some(direct) = src.get(cpath) {
        additions.push((String::new(), direct.clone()));
    }
    if let Some(sub) = src.dir_at(cpath) {
        for (rel, entry) in sub.flat_entries() {
            additions.push((rel, entry));
        }
    }
    let limits = budget.limits;
    if additions.len() > limits.max_copy_expansion {
        return Err(Error::ResourceLimit {
            what: "one directory copy's expansion".to_string(),
            ceiling: format!("{} entries", limits.max_copy_expansion),
        });
    }
    // RFC 020 D-2: checked here, once, against the *resulting* live tree size — the other of the two
    // points that still check `max_tree_entries` (the module doc explains why ordinary adds no longer
    // do). A conservative, single check before inserting anything: every addition could in principle be
    // a genuinely new path (the cheapest case to reason about; an addition that overwrites an existing
    // path only ever makes the true total smaller than this bound).
    if budget.live_entries.saturating_add(additions.len()) > limits.max_tree_entries {
        return Err(Error::ResourceLimit {
            what: "one tree in this repository".to_string(),
            ceiling: format!("{} entries", limits.max_tree_entries),
        });
    }
    for (rel, entry) in additions {
        let to = if rel.is_empty() {
            node.path.clone()
        } else {
            format!("{}/{rel}", node.path)
        };
        copy_out.copies.push(CopyRecord {
            from: if rel.is_empty() {
                cpath.clone()
            } else {
                format!("{cpath}/{rel}")
            },
            from_atom,
            to: to.clone(),
            status: EpistemicStatus::Stated,
        });
        copy_out.source_revs.push(*crev);
        tree.insert(&to, entry, budget)?;
    }
    Ok(())
}

/// Resolve `rev`'s already-assigned atom id (RFC 011 §5). A `copyfrom-rev` always names an earlier,
/// already-processed revision, so every lookup here should succeed; a miss means a malformed dump
/// (a forward reference) rather than a decoder bug.
fn atom_for(revnum_to_atom: &HashMap<u64, AtomId>, rev: u64) -> Result<AtomId, Error> {
    revnum_to_atom.get(&rev).copied().ok_or_else(|| {
        Error::Read(format!(
            "copyfrom references revision {rev}, whose atom is not yet known (a forward reference)"
        ))
    })
}

/// Diff `prev` → `new` into `Stated` path operations (one op per changed path). A path named in
/// `replaced` gets `PathOp::Replace` whenever it is present on both sides, even with identical content
/// (CR-07.2, RFC 011 D-7) — the source stated a new node, not a modification. The order the two trees are
/// walked in does not affect the artifact: [`IrBuilder::add_atom`] sorts every atom's `ops` canonically
/// by path before storing them.
///
/// RFC 020 D-1's own efficiency win, not just D-1's memory one: `prev` and `new` are walked **together**,
/// comparing `Rc<Dir>` identity at every directory and skipping straight past any pair that is
/// `Rc::ptr_eq` — a subtree neither side touched this revision is never even read, let alone flattened.
/// An ordinary revision (one file changed) costs O(depth), not O(whole tree): the fix for a real,
/// measured regression (codeblocks' decode time was ~37% *slower* than 0.8.1 with an earlier version of
/// this function that called [`Tree::flat_entries`] on both whole trees every revision — correct, same
/// ops, but exactly the O(whole tree)-per-revision cost this walk exists to avoid).
///
/// `pub(crate)`: review 169 F-1 — `crate::branch_lines` reuses this exact function (with an empty
/// `replaced`) for a new line's first atom, instead of a second, separately-written diff that would
/// need its own `Rc::ptr_eq` short-circuiting to get the same win.
pub(crate) fn diff(prev: &Tree, new: &Tree, replaced: &HashSet<String>) -> Vec<PathOp> {
    let mut ops = Vec::new();
    diff_dir(&prev.0, &new.0, "", replaced, &mut ops);
    ops
}

/// [`diff`]'s own recursion, one `Dir` level at a time. Files and subdirectories are each compared in
/// their own map's bare-name order (not merged against each other the way [`Dir::flat_entries_into`]
/// must): order never reaches the artifact here (see [`diff`]'s own doc), only *which* ops are produced.
fn diff_dir(
    prev: &Rc<Dir>,
    new: &Rc<Dir>,
    prefix: &str,
    replaced: &HashSet<String>,
    ops: &mut Vec<PathOp>,
) {
    if Rc::ptr_eq(prev, new) {
        return;
    }
    let mut prev_files = prev.files.iter().peekable();
    let mut new_files = new.files.iter().peekable();
    loop {
        match (prev_files.peek().copied(), new_files.peek().copied()) {
            (None, None) => break,
            (Some((p, _)), None) => {
                ops.push(delete_op(format!("{prefix}{p}")));
                prev_files.next();
            }
            (None, Some((n, e))) => {
                ops.push(add_op(format!("{prefix}{n}"), e));
                new_files.next();
            }
            (Some((p, pe)), Some((n, ne))) => match p.as_str().cmp(n.as_str()) {
                std::cmp::Ordering::Less => {
                    ops.push(delete_op(format!("{prefix}{p}")));
                    prev_files.next();
                }
                std::cmp::Ordering::Greater => {
                    ops.push(add_op(format!("{prefix}{n}"), ne));
                    new_files.next();
                }
                std::cmp::Ordering::Equal => {
                    let path = format!("{prefix}{n}");
                    if replaced.contains(&path) {
                        ops.push(replace_op(path, ne));
                    } else if pe.blob != ne.blob || pe.mode != ne.mode {
                        ops.push(modify_op(path, ne));
                    }
                    prev_files.next();
                    new_files.next();
                }
            },
        }
    }
    let mut prev_dirs = prev.dirs.iter().peekable();
    let mut new_dirs = new.dirs.iter().peekable();
    loop {
        match (prev_dirs.peek().copied(), new_dirs.peek().copied()) {
            (None, None) => break,
            (Some((p, psub)), None) => {
                emit_all(psub, &format!("{prefix}{p}/"), ops, Emit::Delete);
                prev_dirs.next();
            }
            (None, Some((n, nsub))) => {
                emit_all(nsub, &format!("{prefix}{n}/"), ops, Emit::Add);
                new_dirs.next();
            }
            (Some((p, psub)), Some((n, nsub))) => match p.as_str().cmp(n.as_str()) {
                std::cmp::Ordering::Less => {
                    emit_all(psub, &format!("{prefix}{p}/"), ops, Emit::Delete);
                    prev_dirs.next();
                }
                std::cmp::Ordering::Greater => {
                    emit_all(nsub, &format!("{prefix}{n}/"), ops, Emit::Add);
                    new_dirs.next();
                }
                std::cmp::Ordering::Equal => {
                    diff_dir(psub, nsub, &format!("{prefix}{n}/"), replaced, ops);
                    prev_dirs.next();
                    new_dirs.next();
                }
            },
        }
    }
}

fn add_op(path: String, e: &FileEntry) -> PathOp {
    PathOp::Add {
        path,
        blob: e.blob,
        mode: e.mode,
        status: EpistemicStatus::Stated,
    }
}

fn modify_op(path: String, e: &FileEntry) -> PathOp {
    PathOp::Modify {
        path,
        blob: e.blob,
        mode: e.mode,
        status: EpistemicStatus::Stated,
    }
}

fn replace_op(path: String, e: &FileEntry) -> PathOp {
    PathOp::Replace {
        path,
        blob: e.blob,
        mode: e.mode,
        status: EpistemicStatus::Stated,
    }
}

fn delete_op(path: String) -> PathOp {
    PathOp::Delete {
        path,
        status: EpistemicStatus::Stated,
    }
}

/// Whether [`emit_all`] should treat every file it finds under a whole prev-only or new-only subtree as
/// added or deleted — a path that exists on only one side can never be in `diff`'s `replaced` set (that
/// requires presence on both sides at the exact path), so neither case needs to consult it.
enum Emit {
    Add,
    Delete,
}

/// Every file under `dir`, each as an `Add` or `Delete` op (whichever `emit` asks for) — used only for a
/// whole subtree present on just one side (a directory wholly added or wholly removed this revision),
/// so this walk's cost is bounded by the size of *that* change, not the whole tree.
fn emit_all(dir: &Dir, prefix: &str, ops: &mut Vec<PathOp>, emit: Emit) {
    for (path, entry) in {
        let mut out = Vec::new();
        dir.flat_entries_into(prefix, &mut out);
        out
    } {
        ops.push(match emit {
            Emit::Add => add_op(path, &entry),
            Emit::Delete => delete_op(path),
        });
    }
}

/// The retained snapshot for a revision. Only revisions a `copyfrom` names are retained (RFC 010
/// increment 1); a reference to a revision not present in this dump is a typed refusal.
fn snapshot_for(kept: &HashMap<u64, Tree>, rev: u64) -> Result<&Tree, Error> {
    kept.get(&rev).ok_or_else(|| {
        Error::Read(format!(
            "copyfrom references revision {rev}, which is not present in this dumpstream"
        ))
    })
}

fn lookup_file(kept: &HashMap<u64, Tree>, rev: u64, path: &str) -> Result<FileEntry, Error> {
    let t = snapshot_for(kept, rev)?;
    t.get(path).cloned().ok_or_else(|| {
        Error::Read(format!(
            "copyfrom source file '{path}' at revision {rev} not found"
        ))
    })
}

#[cfg(test)]
mod tests;

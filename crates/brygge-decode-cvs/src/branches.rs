//! CVS branch history (RFC 013 D-3): which files are on which named branch, on which line each branch is cut
//! from, and where it is cut on that line. Pure logic over reconstructed changesets; `decode` builds the atoms.
//!
//! **A branch is identified by its symbol name**, never by number (numbers are per file). A file is *on* a
//! branch when its symbols give that name a **magic** branch number (`1.2.0.4`: the branch is `1.2.4`, cut
//! from revision `1.2`; `1.2.4.3.0.2`: the branch `1.2.4.3.2`, cut from a branch revision); a literal
//! (odd-length) symbol is a vendor branch, which is not a line here.
//!
//! **A branch's parent line** is the line its branch-point revisions lie on: the main line (a trunk revision,
//! or a vendor revision while it is the default), or another named branch. When a symbol's branch points lie
//! on several lines (a mixed working copy) the line holding most of them wins, ties to the main line, then to
//! the lower symbol name (handoff §6). A branch whose parent line is not imported is counted, never guessed.

use std::collections::{BTreeMap, BTreeSet};

use crate::mainline::{BranchSymbol, branch_of, branch_symbol, is_main_line};
use crate::rcs::RevNum;
use crate::scan::CvsFile;

/// A line of history: the main line, a named branch, or a line that cannot be a parent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineRef {
    /// The main line (the trunk, and the vendor branch while it is the default).
    Main,
    /// The branch this symbol names.
    Named(String),
    /// A branch with no symbol in its file, or a vendor branch that is no longer the default: no name, so it
    /// cannot be imported or be a parent.
    Unknown,
}

/// One file that is on a named branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnBranch {
    /// Index into the decoder's file list.
    pub file: usize,
    /// The branch's own number in this file, e.g. `1.2.4` or `1.2.4.3.2`.
    pub branch_id: RevNum,
    /// The branch-point revision it is cut from, e.g. `1.2` or `1.2.4.3`.
    pub point: RevNum,
    /// The line that branch-point revision is on, in this file.
    pub line: LineRef,
    /// Whether the branch-point revision exists, and is live.
    pub state: PointState,
}

/// What a file's branch-point revision is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointState {
    /// The symbol names a revision the file does not have (outdated, e.g. after `cvs admin -o`): the file
    /// leaves the branch, and is counted.
    Missing,
    /// The revision is `dead` (the file was added on the branch): the file is on the branch, and does not
    /// constrain its parent.
    Dead,
    /// A live revision: the file is on the branch at that content.
    Live,
}

/// The branch number and branch point a magic symbol names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MagicBranch {
    /// The branch's own number in the file, e.g. `1.2.4` or `1.1.1.1.2`.
    pub branch_id: RevNum,
    /// The revision it is cut from, e.g. `1.2` or `1.1.1.1`.
    pub point: RevNum,
}

/// The branch a **magic** symbol number names: `0` second from the end and an even length of at least four.
/// The branch point is the number without its last two components; the branch is the point plus the last
/// component. `None` for a tag or a literal (vendor) branch number.
#[must_use]
pub fn magic_branch(rev: &RevNum) -> Option<MagicBranch> {
    let n = rev.0.len();
    if n < 4 || n % 2 != 0 || rev.0.get(n - 2) != Some(&0) {
        return None;
    }
    let point = RevNum(rev.0.get(..n - 2)?.to_vec());
    let mut branch_id = point.0.clone();
    branch_id.push(*rev.0.get(n - 1)?);
    Some(MagicBranch {
        branch_id: RevNum(branch_id),
        point,
    })
}

/// The line `revision` is on **in this file** (review 036 F-1, RFC 013 §6): the main line by
/// [`is_main_line`] (a trunk revision, or a revision of the default vendor branch); otherwise the named
/// branch whose number the revision lies on, or [`LineRef::Unknown`] when that branch has no magic symbol
/// (an unnamed branch, or a vendor branch that is no longer the default).
#[must_use]
pub fn line_of(file: &CvsFile, revision: &RevNum) -> LineRef {
    if is_main_line(revision, file.rcs.branch.as_ref()) {
        return LineRef::Main;
    }
    match branch_symbol(&file.rcs, &branch_of(revision)) {
        Some(BranchSymbol::Magic(name)) => LineRef::Named(name.to_string()),
        Some(BranchSymbol::Literal(_)) | None => LineRef::Unknown,
    }
}

/// Every named branch, by symbol name, with the files on it (in file order), the line each file's branch
/// point is on, and whether that revision exists and is live. Deterministic: names sort, files keep their (path-sorted) order.
#[must_use]
pub fn discover(files: &[CvsFile]) -> BTreeMap<String, Vec<OnBranch>> {
    let mut out: BTreeMap<String, Vec<OnBranch>> = BTreeMap::new();
    for (i, f) in files.iter().enumerate() {
        for (name, rev) in &f.rcs.symbols {
            if let Some(m) = magic_branch(rev) {
                let line = line_of(f, &m.point);
                let state = match f.rcs.revisions.get(&m.point) {
                    None => PointState::Missing,
                    Some(r) if r.state == "dead" => PointState::Dead,
                    Some(_) => PointState::Live,
                };
                out.entry(name.clone()).or_default().push(OnBranch {
                    file: i,
                    branch_id: m.branch_id,
                    point: m.point,
                    line,
                    state,
                });
            }
        }
    }
    out
}

/// Which branches are imported, on which parent line, and in what order (RFC 013 §6).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    /// Each imported branch's parent line: [`LineRef::Main`] or [`LineRef::Named`] (never `Unknown`).
    pub parent: BTreeMap<String, LineRef>,
    /// The imported branches in dependency order: a branch comes after its parent line; within that, by name.
    pub order: Vec<String>,
    /// Branches whose parent line is not imported (an unnamed line, a line that is itself dropped, or a
    /// cycle): dropped and counted, never guessed.
    pub blocked: BTreeSet<String>,
}

/// The parent line of one branch: the line holding most of its (live) branch points, ties to the main line,
/// then to the lower name; `None` when no branch point lies on a line that can be a parent.
#[must_use]
pub fn majority_line(entries: &[&OnBranch]) -> Option<LineRef> {
    let mut count: BTreeMap<&LineRef, usize> = BTreeMap::new();
    for e in entries {
        if e.state == PointState::Live && e.line != LineRef::Unknown {
            *count.entry(&e.line).or_default() += 1;
        }
    }
    // `LineRef` orders Main < Named(by name), so the first maximum in key order wins the ties as ruled.
    let mut best: Option<(&LineRef, usize)> = None;
    for (line, n) in count {
        if best.is_none_or(|(_, b)| n > b) {
            best = Some((line, n));
        }
    }
    best.map(|(l, _)| l.clone())
}

/// Decide which branches are imported. `entries` are the files on each branch **whose branch-point revision
/// exists** (the caller drops the `Missing` ones first: they leave the branch and are counted).
///
/// A branch is imported when its parent line is the main line, or a branch already imported; a branch none of
/// whose files constrains it (every file was added on the branch) is imported with the main line as its
/// nominal parent and no parent edge. The rest (a live branch point on no line that can be a parent, or on a
/// line that is itself not imported, or a cycle) are `blocked`. Branches are taken in waves in name order
/// until no more can be.
#[must_use]
pub fn resolve(entries: &BTreeMap<String, Vec<OnBranch>>) -> Resolution {
    let mut want: BTreeMap<&str, Option<LineRef>> = BTreeMap::new();
    for (name, on) in entries {
        let refs: Vec<&OnBranch> = on.iter().collect();
        let constrained = on.iter().any(|e| e.state == PointState::Live);
        let parent = if constrained {
            majority_line(&refs)
        } else {
            Some(LineRef::Main)
        };
        want.insert(name.as_str(), parent);
    }
    let mut out = Resolution::default();
    let mut imported: BTreeSet<&str> = BTreeSet::new();
    loop {
        let mut progressed = false;
        for (&name, parent) in &want {
            if imported.contains(name) {
                continue;
            }
            let ready = match parent {
                Some(LineRef::Main) => true,
                Some(LineRef::Named(p)) => p != name && imported.contains(p.as_str()),
                Some(LineRef::Unknown) | None => false,
            };
            if ready {
                imported.insert(name);
                out.order.push(name.to_string());
                if let Some(p) = parent {
                    out.parent.insert(name.to_string(), p.clone());
                }
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    for &name in want.keys() {
        if !imported.contains(name) {
            out.blocked.insert(name.to_string());
        }
    }
    out
}

/// Where a branch's first changeset hangs, on the line it leaves (RFC 013 D-3, as amended).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parent {
    /// No file constrains the branch point, so it has no parent on the line: its first changeset is a root.
    Root,
    /// The changeset at `index` of the parent line.
    Changeset {
        /// Position in the parent line's changeset order.
        index: usize,
        /// `true` when every covered file is at its branch point there; `false` for the approximation.
        exact: bool,
    },
}

/// The parent rule **earliest-covering-changeset**, from each covered file's run `[in, out)` of the parent
/// line's changesets during which the file is exactly at its branch-point revision (`out` is where its next
/// revision on that line arrives, or `line_len` at the end of the line).
///
/// - **Exact**: `max(in) < min(out)`: the parent is `c[max(in)]`, the earliest changeset at which every
///   covered file is at its branch point.
/// - **Approximate**: otherwise, the changeset with the most covered files at their branch points, the
///   earliest on ties.
/// - **No covered file**: [`Parent::Root`].
#[must_use]
pub fn choose_parent(runs: &[(usize, usize)], line_len: usize) -> Parent {
    if runs.is_empty() {
        return Parent::Root;
    }
    let max_in = runs.iter().map(|r| r.0).max().unwrap_or(0);
    let min_out = runs.iter().map(|r| r.1).min().unwrap_or(0);
    if max_in < min_out {
        return Parent::Changeset {
            index: max_in,
            exact: true,
        };
    }
    // Count, for every changeset, the covered files at their branch points (a difference array).
    let mut delta = vec![0i64; line_len + 1];
    for &(i, o) in runs {
        let o = o.min(line_len);
        if let Some(d) = delta.get_mut(i) {
            *d += 1;
        }
        if let Some(d) = delta.get_mut(o) {
            *d -= 1;
        }
    }
    let mut best = (0i64, 0usize);
    let mut running = 0i64;
    for (k, d) in delta.iter().take(line_len).enumerate() {
        running += d;
        if running > best.0 {
            best = (running, k);
        }
    }
    Parent::Changeset {
        index: best.1,
        exact: false,
    }
}

#[cfg(test)]
mod tests;

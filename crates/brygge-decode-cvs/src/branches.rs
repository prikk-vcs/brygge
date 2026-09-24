//! CVS branch history (RFC 013 D-3): which files are on which named branch, and where a branch is cut from
//! the line it leaves. Pure logic over reconstructed changesets; `decode` builds the atoms.
//!
//! **A branch is identified by its symbol name**, never by number (numbers are per file). A file is *on* a
//! branch when its symbols give that name a **magic** branch number (`1.2.0.4`: the branch is `1.2.4`, cut
//! from revision `1.2`); a literal (odd-length) symbol is a vendor branch, which is not a line here. This
//! step handles branches cut from the main line (a trunk revision, or a revision of the vendor branch while it
//! is the default); a symbol cut from any other line, in any file, is counted, not imported.

use std::collections::{BTreeMap, BTreeSet};

use crate::mainline::is_main_line;
use crate::rcs::RevNum;
use crate::scan::CvsFile;

/// One file that is on a named branch cut from the main line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnBranch {
    /// Index into the decoder's file list.
    pub file: usize,
    /// The branch's own number in this file, e.g. `1.2.4`.
    pub branch_id: RevNum,
    /// The branch-point revision it is cut from, e.g. `1.2` (a trunk revision).
    pub point: RevNum,
}

/// How a magic branch symbol's number cuts a file: from a main-line revision, or from a revision of another
/// line (a branch of a branch, or a vendor branch that is no longer the default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cut {
    /// Cut from a revision of the main line: an ordinary trunk revision, or, while the file's vendor branch is
    /// the default, a revision on it (`cvs import` then `cvs tag -b` with no edit gives `1.1.1.1.0.2`).
    MainLine(OnBranchPoint),
    /// Cut from a revision that is not on the main line.
    OffLine,
}

/// The branch number and branch point a magic symbol names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnBranchPoint {
    /// The branch's own number in the file, e.g. `1.2.4` or `1.1.1.1.2`.
    pub branch_id: RevNum,
    /// The revision it is cut from, e.g. `1.2` or `1.1.1.1`.
    pub point: RevNum,
}

/// Classify a symbol's revision **by the line its branch point is on, not by its length** (review 036 F-1).
///
/// A magic branch number has `0` second from the end and an even length of at least four: the branch point
/// is the number without its last two components and the branch is the point plus the last component. It
/// is cut from the main line when [`is_main_line`] says the branch point is on it *in this file*. Anything
/// else is a tag or a literal (vendor) branch number: `None`.
#[must_use]
pub fn classify(rev: &RevNum, vendor_branch: Option<&RevNum>) -> Option<Cut> {
    let n = rev.0.len();
    if n < 4 || n % 2 != 0 || rev.0.get(n - 2) != Some(&0) {
        return None;
    }
    let point = RevNum(rev.0.get(..n - 2)?.to_vec());
    let mut branch_id = point.0.clone();
    branch_id.push(*rev.0.get(n - 1)?);
    if is_main_line(&point, vendor_branch) {
        Some(Cut::MainLine(OnBranchPoint {
            branch_id: RevNum(branch_id),
            point,
        }))
    } else {
        Some(Cut::OffLine)
    }
}

/// The branches found by name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Discovered {
    /// Every named branch cut from the main line in **every** file that has it, by symbol name, with the
    /// files on it (in file order). Deterministic: names sort, files keep their (path-sorted) order.
    pub main: BTreeMap<String, Vec<OnBranch>>,
    /// Names with at least one file whose branch point is off the main line (review 036 F-2). Such a symbol is
    /// not imported at all in this step (its parent line would differ by file): all its revisions are counted.
    pub off_line: BTreeSet<String>,
}

/// Find every named branch by its symbol, and split those cut from the main line in all their files from
/// those cut from another line in any.
#[must_use]
pub fn discover(files: &[CvsFile]) -> Discovered {
    let mut out = Discovered::default();
    for (i, f) in files.iter().enumerate() {
        for (name, rev) in &f.rcs.symbols {
            match classify(rev, f.rcs.branch.as_ref()) {
                Some(Cut::MainLine(c)) => {
                    out.main.entry(name.clone()).or_default().push(OnBranch {
                        file: i,
                        branch_id: c.branch_id,
                        point: c.point,
                    })
                }
                Some(Cut::OffLine) => {
                    out.off_line.insert(name.clone());
                }
                None => {}
            }
        }
    }
    for name in &out.off_line {
        out.main.remove(name);
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

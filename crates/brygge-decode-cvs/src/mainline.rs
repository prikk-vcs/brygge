//! Main-line revision classification (owner ruling D-2; CVS corrections handoff §2.1). A CVS trunk
//! checkout is what a user gets without `-r`: trunk revisions, plus — while a vendor branch is set —
//! that branch's own revisions (a `cvs import` merges onto trunk logically, even though RCS stores it as
//! a branch). Every other revision is a **branch revision**: not imported until branch-aware threading
//! lands (0.3.0), but never silently — it is dropped with a record and its count.

use crate::rcs::{RcsFile, RevNum};

/// True when `rev` belongs to the main line: a trunk revision (`is_trunk`), or — while `vendor_branch`
/// is `Some` — a revision directly on that branch.
#[must_use]
pub fn is_main_line(rev: &RevNum, vendor_branch: Option<&RevNum>) -> bool {
    if rev.is_trunk() {
        return true;
    }
    match vendor_branch {
        Some(vb) => is_direct_branch_revision(rev, vb),
        None => false,
    }
}

/// True when `rev` is a revision directly on the branch numbered `branch_id`: one component deeper, same
/// prefix (e.g. `1.1.1.2` is directly on `1.1.1`; `1.1.1.2.1.1`, a branch of a branch, is not).
#[must_use]
pub fn is_direct_branch_revision(rev: &RevNum, branch_id: &RevNum) -> bool {
    rev.0.len() == branch_id.0.len() + 1 && rev.0.get(..branch_id.0.len()) == Some(&branch_id.0[..])
}

/// The revision a branch descends from: `1.1.1` (and `1.2.2`) branch from `1.1`/`1.2` respectively — the
/// number with its last component dropped.
#[must_use]
pub fn branch_point(branch_id: &RevNum) -> RevNum {
    let n = branch_id.0.len().saturating_sub(1);
    RevNum(branch_id.0.get(..n).unwrap_or(&[]).to_vec())
}

/// The branch a revision lives on: its own number with the last component dropped (e.g. `1.2.2.1` lives
/// on branch `1.2.2`).
#[must_use]
pub fn branch_of(rev: &RevNum) -> RevNum {
    let n = rev.0.len().saturating_sub(1);
    RevNum(rev.0.get(..n).unwrap_or(&[]).to_vec())
}

/// Convert a branch's real number (as its own revisions use it, e.g. `1.2.2`) to CVS's "magic branch
/// number" form (`1.2.0.2`) — a `0` inserted before the last component — which is how a branch tag is
/// recorded in the symbols table. `None` for a number too short to be a branch id (fewer than 3
/// components).
#[must_use]
pub fn magic_branch_number(branch_id: &RevNum) -> Option<RevNum> {
    if branch_id.0.len() < 3 {
        return None;
    }
    let mut v = branch_id.0.clone();
    let last = v.pop()?;
    v.push(0);
    v.push(last);
    Some(RevNum(v))
}

/// The symbol name (if any) `file` gives the branch `branch_id`, resolved through **either** form RCS
/// uses for a branch symbol (review 008 R-2): the magic branch number (`1.2.0.2`), or — for a vendor
/// branch, which RCS stores literally — the branch's own number (`1.1.1`) directly. ("Numbers differ
/// from file to file, so numbers cannot identify a branch" — the *name* is looked up per file, per the
/// handoff.)
#[must_use]
pub fn branch_symbol_name<'a>(file: &'a RcsFile, branch_id: &RevNum) -> Option<&'a str> {
    let magic = magic_branch_number(branch_id);
    file.symbols
        .iter()
        .find(|(_, r)| Some(r) == magic.as_ref() || r == branch_id)
        .map(|(name, _)| name.as_str())
}

/// The vendor branch's first (earliest) revision, if `file` has one: the entry in the branch point's
/// `branches` list whose number is directly on `vendor_branch`.
#[must_use]
pub fn vendor_branch_first_revision(file: &RcsFile, vendor_branch: &RevNum) -> Option<RevNum> {
    let bp = branch_point(vendor_branch);
    file.revisions.get(&bp)?.branches.iter().find_map(|b| {
        if is_direct_branch_revision(b, vendor_branch) {
            Some(b.clone())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests;

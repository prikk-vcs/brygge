//! Per-file symbolic names → reconstructed tag/branch refs (RFC 007 §5c/D-5). A CVS tag/branch is a
//! symbolic name in each `,v`'s admin section naming a per-file revision; a repo-wide tag/branch is the
//! *set* of those revisions — so a reconstructed ref is a `Derived` judgment (opt-in), never `Stated`.

use std::collections::BTreeMap;

use crate::rcs::RevNum;
use crate::scan::CvsFile;

/// A collected repository-wide symbol.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The symbolic name.
    pub name: String,
    /// True if it names a branch (a magic branch number), false for a tag.
    pub is_branch: bool,
    /// The `(path, revision)` pairs it names across files.
    pub named: Vec<(String, RevNum)>,
}

/// Whether a symbol's revision is a CVS **magic branch number** (`1.2.0.2` — the second-to-last component
/// is `0`), as opposed to a plain tagged revision (`1.2`).
fn is_branch_rev(rev: &RevNum) -> bool {
    let n = rev.0.len();
    n >= 2 && rev.0.get(n - 2) == Some(&0)
}

/// Collect symbols across all files, grouped by name, in deterministic (name-sorted) order.
#[must_use]
pub fn collect(files: &[CvsFile]) -> Vec<Symbol> {
    let mut by_name: BTreeMap<String, (bool, Vec<(String, RevNum)>)> = BTreeMap::new();
    for f in files {
        for (name, rev) in &f.rcs.symbols {
            let entry = by_name.entry(name.clone()).or_insert((false, Vec::new()));
            entry.0 |= is_branch_rev(rev);
            entry.1.push((f.path.clone(), rev.clone()));
        }
    }
    by_name
        .into_iter()
        .map(|(name, (is_branch, mut named))| {
            named.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            Symbol {
                name,
                is_branch,
                named,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;

//! The changeset reconstructor (RFC 007 §5b/D-3) — the research core.
//!
//! CVS has no atomic commit, so a changeset is *reconstructed* by clustering per-file revisions on
//! `(author, log)` within a time window. The grouping is **deterministic** (a security property, VF-1):
//! revisions are sorted by `(date, path, rev)` with a total tie-break, then grouped in a single greedy
//! pass. Each changeset carries a **confidence** from its time-tightness; the floor (D-8) flags the
//! low-confidence ones. Every resulting atom is `Derived(ReconstructedChangeset)` (SRC-C1).

use crate::rcs::RevNum;

/// One per-file revision — the atomic input to reconstruction.
#[derive(Debug, Clone)]
pub struct FileRev {
    /// Repo-relative file path.
    pub path: String,
    /// The RCS revision number.
    pub rev: RevNum,
    /// Commit time (epoch seconds).
    pub date: i64,
    /// Committer login (claim).
    pub author: String,
    /// Log message (claim).
    pub log: Vec<u8>,
    /// RCS state (`dead` = a deletion).
    pub state: String,
    /// Reconstructed content (empty for a `dead` revision).
    pub content: Vec<u8>,
}

impl FileRev {
    /// Whether this revision marks the file deleted.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.state == "dead"
    }
}

/// A reconstructed changeset: a group of per-file revisions and its derived confidence.
#[derive(Debug, Clone)]
pub struct Changeset {
    /// The per-file revisions grouped into this changeset (in sorted input order).
    pub revs: Vec<FileRev>,
    /// The clustered author.
    pub author: String,
    /// The clustered log message.
    pub log: Vec<u8>,
    /// The representative time (the latest per-file date in the group).
    pub date: i64,
    /// A confidence in `0..=100` from the cluster's time-tightness (D-8).
    pub confidence: u8,
}

/// Reconstruct changesets from per-file revisions by clustering on `(author, log)` within `window` seconds.
#[must_use]
pub fn reconstruct(mut revs: Vec<FileRev>, window: u64) -> Vec<Changeset> {
    // Deterministic total order: date, then path, then revision, then state.
    revs.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.rev.cmp(&b.rev))
            .then_with(|| a.state.cmp(&b.state))
    });

    let window = i64::try_from(window).unwrap_or(i64::MAX);
    let mut groups: Vec<Vec<FileRev>> = Vec::new();
    let mut current: Vec<FileRev> = Vec::new();
    let mut last_date = 0i64;

    for fr in revs {
        let joins = match current.first() {
            None => false,
            Some(head) => {
                head.author == fr.author
                    && head.log == fr.log
                    && fr.date - last_date <= window
                    && !current.iter().any(|c| c.path == fr.path)
            }
        };
        if joins {
            last_date = fr.date;
            current.push(fr);
        } else {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
            last_date = fr.date;
            current.push(fr);
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }

    groups
        .into_iter()
        .map(|revs| {
            let author = revs.first().map(|r| r.author.clone()).unwrap_or_default();
            let log = revs.first().map(|r| r.log.clone()).unwrap_or_default();
            let first_date = revs.iter().map(|r| r.date).min().unwrap_or(0);
            let date = revs.iter().map(|r| r.date).max().unwrap_or(0);
            let confidence = confidence_of(date - first_date, window);
            Changeset {
                revs,
                author,
                log,
                date,
                confidence,
            }
        })
        .collect()
}

/// Confidence from a cluster's total time span: a same-instant commit is fully confident; one spread
/// across (or beyond) the window is increasingly suspect.
fn confidence_of(span: i64, window: i64) -> u8 {
    if span <= 0 {
        return 100;
    }
    let w = window.max(1);
    let penalty = (span.saturating_mul(100) / w).min(100);
    u8::try_from(100 - penalty).unwrap_or(0)
}

#[cfg(test)]
mod tests;

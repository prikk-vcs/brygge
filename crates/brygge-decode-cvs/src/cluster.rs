//! The changeset reconstructor (RFC 007 §5b/D-3, corrections handoff §2.2, rule `span-overlap-v1`).
//!
//! CVS has no atomic commit, so a changeset is *reconstructed* by clustering per-file revisions on
//! `(author, log)` within a time window. The grouping is **deterministic** (a security property, VF-1):
//! within each `(author, log)` group, revisions are sorted by `(date, path, rev)` and split wherever the
//! gap between consecutive revisions exceeds the window, or a path repeats (a cluster holds at most one
//! revision per path — a repeat starts a new cluster). Clusters are then ordered by `(latest date,
//! smallest "path@rev" string)`; a **per-file order fix** repairs the rare case where that ordering would
//! place a later-numbered revision of one path before an earlier-numbered one (an inter-cluster date-skew
//! artifact, not a real reordering), extracting the later revision into its own singleton, counted.
//! Each resulting changeset carries a **confidence** from its time-tightness and how much its paths
//! overlap other nearby clusters (D-8 flags the low-confidence ones). Every resulting atom is
//! `Derived(ReconstructedChangeset)` (SRC-C1).

use std::collections::{BTreeMap, BTreeSet};

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
    /// Committer login (claim), as the source's raw bytes — no lossy conversion (review 008 R-4).
    pub author: Vec<u8>,
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

    fn path_at_rev(&self) -> String {
        format!("{}@{}", self.path, self.rev.to_dotted())
    }
}

/// A reconstructed changeset: a group of per-file revisions and its derived confidence.
#[derive(Debug, Clone)]
pub struct Changeset {
    /// The per-file revisions grouped into this changeset (in sorted input order).
    pub revs: Vec<FileRev>,
    /// The clustered author, as the source's raw bytes (review 008 R-4).
    pub author: Vec<u8>,
    /// The clustered log message.
    pub log: Vec<u8>,
    /// The representative time (the latest per-file date in the group).
    pub date: i64,
    /// A confidence in `0..=100` from the cluster's time-tightness and path overlap (D-8, rule
    /// `span-overlap-v1`).
    pub confidence: u8,
    /// True when this changeset is a singleton created by the per-file order fix (§2.2 step 5).
    pub order_split: bool,
}

struct Cluster {
    revs: Vec<FileRev>,
    order_split: bool,
}

/// Reconstruct changesets from per-file revisions by clustering on `(author, log)` within `window`
/// seconds, per the exact algorithm in the CVS corrections handoff §2.2.
#[must_use]
pub fn reconstruct(revs: Vec<FileRev>, window: u64) -> Vec<Changeset> {
    let window_i = i64::try_from(window).unwrap_or(i64::MAX);

    // Step 1: group by (author, log bytes).
    let mut groups: BTreeMap<(Vec<u8>, Vec<u8>), Vec<FileRev>> = BTreeMap::new();
    for fr in revs {
        groups
            .entry((fr.author.clone(), fr.log.clone()))
            .or_default()
            .push(fr);
    }

    // Step 2+3: within each group, sort by (date, path, rev); split on window gaps or a repeated path.
    let mut clusters: Vec<Cluster> = Vec::new();
    for (_, mut group_revs) in groups {
        group_revs.sort_by(|a, b| {
            a.date
                .cmp(&b.date)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.rev.cmp(&b.rev))
        });
        let mut current: Vec<FileRev> = Vec::new();
        let mut last_date = 0i64;
        for fr in group_revs {
            let repeats_path = current.iter().any(|c| c.path == fr.path);
            // Saturating: an attacker-chosen date or window must never wrap (review 008 R-3).
            let gap_too_big = !current.is_empty() && fr.date.saturating_sub(last_date) > window_i;
            if !current.is_empty() && (repeats_path || gap_too_big) {
                clusters.push(Cluster {
                    revs: std::mem::take(&mut current),
                    order_split: false,
                });
            }
            last_date = fr.date;
            current.push(fr);
        }
        if !current.is_empty() {
            clusters.push(Cluster {
                revs: current,
                order_split: false,
            });
        }
    }

    // Step 4: order clusters by (latest date, smallest "path@rev" string).
    clusters.sort_by_key(cluster_sort_key);

    // Step 5: fix per-file order — a later-numbered revision must never precede an earlier one for the
    // same path.
    let (clusters, splits) = fix_per_file_order(clusters);
    let _ = splits; // recorded per-atom via `Changeset::order_split`, not returned separately

    // Confidence, computed once the final cluster order (and thus every cluster's date range) is fixed.
    finalize(clusters, window_i)
}

fn cluster_sort_key(c: &Cluster) -> (i64, String) {
    let latest = c.revs.iter().map(|r| r.date).max().unwrap_or(0);
    let min_key = c
        .revs
        .iter()
        .map(FileRev::path_at_rev)
        .min()
        .unwrap_or_default();
    (latest, min_key)
}

/// Extract any revision that a later-numbered path revision would otherwise precede, turning it into its
/// own singleton cluster placed immediately after the cluster holding the path's preceding revision
/// (handoff §2.2 step 5). Returns the adjusted cluster list and the number of splits performed.
fn fix_per_file_order(mut clusters: Vec<Cluster>) -> (Vec<Cluster>, usize) {
    // For each path, the (revnum, cluster position) pairs in the current order.
    let mut by_path: BTreeMap<String, Vec<(RevNum, usize)>> = BTreeMap::new();
    for (idx, c) in clusters.iter().enumerate() {
        for fr in &c.revs {
            by_path
                .entry(fr.path.clone())
                .or_default()
                .push((fr.rev.clone(), idx));
        }
    }

    // A revision is misplaced when, walking its path's revisions in ascending revnum order, its cluster
    // position is *earlier* than a lower-numbered revision's position already seen.
    let mut extract: BTreeSet<(String, RevNum)> = BTreeSet::new();
    let mut anchor: BTreeMap<(String, RevNum), usize> = BTreeMap::new(); // extracted -> predecessor's original idx
    for (path, mut prs) in by_path {
        prs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut max_pos = None::<usize>;
        for (rev, pos) in prs {
            match max_pos {
                Some(mp) if pos < mp => {
                    extract.insert((path.clone(), rev.clone()));
                    anchor.insert((path.clone(), rev), mp);
                }
                Some(mp) => max_pos = Some(mp.max(pos)),
                None => max_pos = Some(pos),
            }
        }
    }

    if extract.is_empty() {
        return (clusters, 0);
    }

    let splits = extract.len();
    // Pull the extracted revisions out of their clusters, remembering each one's anchor (predecessor's
    // original cluster index).
    let mut singles: Vec<(usize, FileRev)> = Vec::new();
    for (idx, c) in clusters.iter_mut().enumerate() {
        let mut i = 0;
        while i < c.revs.len() {
            let Some(fr) = c.revs.get(i) else { break };
            let key = (fr.path.clone(), fr.rev.clone());
            if extract.contains(&key) {
                let fr = c.revs.remove(i);
                let anchor_idx = anchor.get(&key).copied().unwrap_or(idx);
                singles.push((anchor_idx, fr));
            } else {
                i += 1;
            }
        }
    }

    // Rebuild: original clusters (dropping any left empty), each immediately followed by any singles
    // anchored to it. Singles anchored to the same cluster keep a stable (path, rev) order.
    singles.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.path.cmp(&b.1.path))
            .then_with(|| a.1.rev.cmp(&b.1.rev))
    });
    let mut out: Vec<Cluster> = Vec::with_capacity(clusters.len() + singles.len());
    for (idx, c) in clusters.into_iter().enumerate() {
        if !c.revs.is_empty() {
            out.push(c);
        }
        for (anchor_idx, fr) in singles.iter().filter(|(a, _)| *a == idx) {
            let _ = anchor_idx;
            out.push(Cluster {
                revs: vec![fr.clone()],
                order_split: true,
            });
        }
    }
    (out, splits)
}

fn finalize(clusters: Vec<Cluster>, window: i64) -> Vec<Changeset> {
    let ranges: Vec<(i64, i64, BTreeSet<String>)> = clusters
        .iter()
        .map(|c| {
            let earliest = c.revs.iter().map(|r| r.date).min().unwrap_or(0);
            let latest = c.revs.iter().map(|r| r.date).max().unwrap_or(0);
            let paths: BTreeSet<String> = c.revs.iter().map(|r| r.path.clone()).collect();
            (earliest, latest, paths)
        })
        .collect();

    clusters
        .into_iter()
        .zip(ranges.iter())
        .enumerate()
        .map(|(i, (c, (earliest, latest, my_paths)))| {
            let author = c.revs.first().map(|r| r.author.clone()).unwrap_or_default();
            let log = c.revs.first().map(|r| r.log.clone()).unwrap_or_default();
            // Saturating throughout: an attacker-chosen date or window must never wrap (review 008 R-3).
            let span = latest.saturating_sub(*earliest);
            let time_score = time_score_of(span, window);

            let widened_lo = earliest.saturating_sub(window);
            let widened_hi = latest.saturating_add(window);
            let overlap = my_paths
                .iter()
                .filter(|p| {
                    ranges.iter().enumerate().any(|(j, (e2, l2, paths2))| {
                        j != i && *l2 >= widened_lo && *e2 <= widened_hi && paths2.contains(*p)
                    })
                })
                .count();

            let paths_n = my_paths.len().max(1);
            // Integer confidence, division last: floor(time_score * (paths - overlap/2) / paths).
            let numerator = i64::from(time_score) * (2 * paths_n as i64 - overlap as i64);
            let denominator = 2 * paths_n as i64;
            let mut confidence = u8::try_from((numerator / denominator).clamp(0, 100)).unwrap_or(0);
            if c.order_split {
                confidence = confidence.min(50);
            }

            Changeset {
                date: *latest,
                revs: c.revs,
                author,
                log,
                confidence,
                order_split: c.order_split,
            }
        })
        .collect()
}

/// A same-instant commit is fully confident; one spread across (or beyond) the window is increasingly
/// suspect.
fn time_score_of(span: i64, window: i64) -> u8 {
    if span <= 0 {
        return 100;
    }
    let w = window.max(1);
    let penalty = (span.saturating_mul(100) / w).min(100);
    u8::try_from(100 - penalty).unwrap_or(0)
}

#[cfg(test)]
mod tests;

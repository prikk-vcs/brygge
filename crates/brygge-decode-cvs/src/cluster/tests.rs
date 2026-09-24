//! Unit tests for changeset reconstruction: clustering by (author, log, window), and confidence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{FileRev, reconstruct};
use crate::rcs::RevNum;

fn fr(path: &str, date: i64, author: &str, log: &str) -> FileRev {
    fr_rev(path, &[1, 1], date, author, log)
}

fn fr_rev(path: &str, rev: &[u32], date: i64, author: &str, log: &str) -> FileRev {
    FileRev {
        path: path.to_string(),
        rev: RevNum(rev.to_vec()),
        date,
        author: author.as_bytes().to_vec(),
        log: log.as_bytes().to_vec(),
        state: "Exp".to_string(),
        content: Vec::new(),
    }
}

#[test]
fn revisions_sharing_author_log_and_window_cluster_into_one_changeset() {
    let revs = vec![
        fr("a.c", 1000, "alice", "add feature"),
        fr("b.c", 1002, "alice", "add feature"),
        fr("c.c", 1003, "alice", "add feature"),
    ];
    let cs = reconstruct(revs, 180);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].revs.len(), 3);
    assert_eq!(cs[0].author, b"alice");
    assert_eq!(cs[0].date, 1003); // representative = latest
    assert_eq!(cs[0].confidence, 99); // tight span (3s over a 180s window) → penalty 1
}

#[test]
fn a_different_log_or_author_splits_the_changeset() {
    let revs = vec![
        fr("a.c", 1000, "alice", "one"),
        fr("b.c", 1001, "alice", "two"), // different log
        fr("c.c", 1002, "bob", "two"),   // different author
    ];
    let cs = reconstruct(revs, 180);
    assert_eq!(cs.len(), 3);
}

#[test]
fn a_gap_beyond_the_window_splits_the_changeset() {
    let revs = vec![
        fr("a.c", 1000, "alice", "same"),
        fr("b.c", 5000, "alice", "same"), // 4000s later, beyond a 180s window
    ];
    let cs = reconstruct(revs, 180);
    assert_eq!(cs.len(), 2);
}

#[test]
fn the_same_file_twice_cannot_share_one_changeset() {
    // Two revisions of the same path within the window must not collapse into one changeset.
    let revs = vec![
        fr("a.c", 1000, "alice", "same"),
        fr("a.c", 1001, "alice", "same"),
    ];
    let cs = reconstruct(revs, 180);
    assert_eq!(cs.len(), 2);
}

#[test]
fn interleaved_authors_produce_two_whole_changesets_not_fragments() {
    // Grouping by (author, log) FIRST — not a single chronological greedy pass — means an interleaved
    // commit from a different author never fragments the other author's run.
    let revs = vec![
        fr("a.c", 1000, "alice", "alice work"),
        fr("b.c", 1005, "bob", "bob work"),
        fr("c.c", 1010, "alice", "alice work"),
        fr("d.c", 1015, "bob", "bob work"),
    ];
    let cs = reconstruct(revs, 180);
    assert_eq!(cs.len(), 2, "one changeset per author, not four fragments");
    for c in &cs {
        assert_eq!(c.revs.len(), 2, "each author's two revisions stay together");
    }
}

/// Worked confidence numbers (CVS corrections handoff §6): two single-revision, same-instant clusters
/// (`time_score = 100` each) that both touch `shared.c` within each other's window-widened range. Each
/// has `paths = 1`, `overlap = 1`, so `confidence = floor(100 * (2*1 - 1) / (2*1)) = floor(50) = 50` —
/// below the plain time score of 100, exactly because of the overlap.
#[test]
fn an_overlapping_cluster_has_confidence_below_its_time_score() {
    let a = vec![fr_rev("shared.c", &[1, 1], 1000, "alice", "logA")];
    let b = vec![fr_rev("shared.c", &[1, 2], 1090, "bob", "logB")];
    let mut revs = a;
    revs.extend(b);
    let cs = reconstruct(revs, 180);
    assert_eq!(cs.len(), 2);
    for c in &cs {
        assert_eq!(
            c.confidence, 50,
            "time_score 100, overlap 1, paths 1 -> floor(100*1/2)=50"
        );
    }
}

#[test]
fn a_skewed_date_that_would_reverse_revision_order_produces_a_split() {
    // Cluster A (alice/logA): f@1.3 at t=2000 — a *later* revision.
    // Cluster B (bob/logB): f@1.2 at t=1000, plus other.c@3000 pulling B's own "latest date" past A's.
    // By (latest date) alone, A (2000) would sort before B (3000), placing f@1.3 before f@1.2 — wrong,
    // since 1.2 must always precede 1.3. The fix extracts f@1.3 into its own singleton, right after B.
    let revs = vec![
        fr_rev("f.c", &[1, 3], 2000, "alice", "logA"),
        fr_rev("f.c", &[1, 2], 1000, "bob", "logB"),
        fr_rev("other.c", &[1, 1], 3000, "bob", "logB"),
    ];
    let cs = reconstruct(revs, 3000);
    assert_eq!(
        cs.len(),
        2,
        "cluster A is fully absorbed into the split singleton"
    );
    assert!(!cs[0].order_split);
    assert!(
        cs[0]
            .revs
            .iter()
            .any(|r| r.path == "f.c" && r.rev == RevNum(vec![1, 2])),
        "f@1.2 comes first: {:?}",
        cs[0]
            .revs
            .iter()
            .map(|r| (&r.path, &r.rev))
            .collect::<Vec<_>>()
    );
    assert!(cs[1].order_split);
    assert_eq!(cs[1].revs.len(), 1);
    assert_eq!(cs[1].revs[0].path, "f.c");
    assert_eq!(cs[1].revs[0].rev, RevNum(vec![1, 3]));
    assert!(
        cs[1].confidence <= 50,
        "a split singleton's confidence is capped at 50"
    );
}

#[test]
fn a_wide_cluster_scores_low_confidence() {
    // Same author/log, each within window of the previous, but the total span is large → low confidence.
    let mut revs = Vec::new();
    for i in 0..10i64 {
        revs.push(fr(&format!("f{i}.c"), 1000 + i * 100, "alice", "big"));
    }
    let cs = reconstruct(revs, 180);
    assert_eq!(
        cs.len(),
        1,
        "chained within-window additions stay one changeset"
    );
    assert!(
        cs[0].confidence < 50,
        "a 900s span against a 180s window is low confidence"
    );
}

#[test]
fn an_extreme_window_and_dates_never_overflow_or_panic() {
    // Review 008 R-3: all date/window arithmetic saturates. `window_secs` reaches this code both from
    // the CLI and from an untrusted artifact's provenance (`verify --against-source`), so it must never
    // wrap regardless of how implausible the input is.
    let revs = vec![
        fr_rev("a.c", &[1, 1], i64::MIN, "alice", "x"),
        fr_rev("a.c", &[1, 2], i64::MAX, "alice", "x"),
        fr_rev("b.c", &[1, 1], 0, "alice", "x"),
    ];
    let cs = reconstruct(revs, u64::MAX);
    // No panic reaching here is the main assertion; also sanity-check the result is well-formed.
    for c in &cs {
        assert!(c.confidence <= 100);
    }
}

// ---- RFC 010 increment 3b: the overlap count, indexed per path ---------------------------------------------

mod overlap {
    use std::collections::BTreeSet;

    use super::super::{Range, overlap_counts};

    /// The rule as it was written before the index (O(changesets squared x paths)), kept as the oracle.
    fn naive(ranges: &[Range], window: i64) -> Vec<usize> {
        ranges
            .iter()
            .enumerate()
            .map(|(i, (earliest, latest, paths))| {
                let widened_lo = earliest.saturating_sub(window);
                let widened_hi = latest.saturating_add(window);
                paths
                    .iter()
                    .filter(|p| {
                        ranges.iter().enumerate().any(|(j, (e2, l2, paths2))| {
                            j != i && *l2 >= widened_lo && *e2 <= widened_hi && paths2.contains(*p)
                        })
                    })
                    .count()
            })
            .collect()
    }

    fn range(e: i64, l: i64, paths: &[&str]) -> Range {
        (e, l, paths.iter().map(|p| (*p).to_string()).collect())
    }

    #[test]
    fn a_changeset_never_overlaps_itself() {
        let r = vec![range(10, 20, &["a", "b"])];
        assert_eq!(overlap_counts(&r, 0), vec![0]);
        assert_eq!(overlap_counts(&r, 1_000), vec![0]);
    }

    #[test]
    fn the_bounds_are_inclusive_at_both_ends() {
        // j = [20, 30] and i = [0, 10] with window 10: widened i = [-10, 20]. j's earliest 20 <= 20 touches.
        let r = vec![range(0, 10, &["a"]), range(20, 30, &["a"])];
        assert_eq!(overlap_counts(&r, 10), vec![1, 1]);
        // one second less and they no longer touch (either way round).
        assert_eq!(overlap_counts(&r, 9), vec![0, 0]);
        assert_eq!(naive(&r, 9), vec![0, 0]);
    }

    #[test]
    fn only_a_shared_path_counts() {
        let r = vec![range(0, 10, &["a", "b"]), range(5, 15, &["b", "c"])];
        assert_eq!(overlap_counts(&r, 0), vec![1, 1], "only `b` is shared");
    }

    #[test]
    fn saturation_at_the_extremes_is_the_same_as_before() {
        let r = vec![
            range(i64::MIN, i64::MIN + 5, &["a"]),
            range(i64::MAX - 5, i64::MAX, &["a"]),
            range(0, 0, &["a"]),
        ];
        for window in [0, 1, i64::MAX / 2, i64::MAX] {
            assert_eq!(
                overlap_counts(&r, window),
                naive(&r, window),
                "window {window}"
            );
        }
    }

    /// A fixed-seed generator (xorshift), so the run is the same every time.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// Several thousand random inputs, the index against the oracle: random paths, dates and windows, including
    /// window 0, equal dates, `i64` extremes (saturation), inverted ranges, changesets with no path, and many
    /// changesets on one path.
    #[test]
    fn the_index_equals_the_naive_rule_on_generated_inputs() {
        let dates: [i64; 14] = [
            i64::MIN,
            i64::MIN + 1,
            -5,
            0,
            0,
            1,
            2,
            3,
            10,
            100,
            1_000,
            i64::MAX - 1,
            i64::MAX,
            i64::MAX,
        ];
        let windows: [i64; 8] = [0, 0, 1, 2, 180, 1_000, i64::MAX / 2, i64::MAX];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for case in 0..6_000 {
            let n = 1 + rng.below(30);
            // a small path pool makes many changesets share one path; a pool of 1 makes them all share it.
            let pool = 1 + rng.below(if case % 5 == 0 { 1 } else { 8 });
            let ranges: Vec<Range> = (0..n)
                .map(|_| {
                    let a = dates[rng.below(dates.len())].saturating_add(rng.below(4) as i64);
                    let b = dates[rng.below(dates.len())].saturating_add(rng.below(4) as i64);
                    // mostly a proper range; sometimes inverted (the predicate must agree regardless)
                    let (e, l) = if rng.below(10) == 0 {
                        (a.max(b), a.min(b))
                    } else {
                        (a.min(b), a.max(b))
                    };
                    let mut paths = BTreeSet::new();
                    for _ in 0..rng.below(5) {
                        paths.insert(format!("p{}", rng.below(pool)));
                    }
                    (e, l, paths)
                })
                .collect();
            let window = windows[rng.below(windows.len())];
            assert_eq!(
                overlap_counts(&ranges, window),
                naive(&ranges, window),
                "case {case}: window {window}, ranges {ranges:?}"
            );
        }
    }
}

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

//! Unit tests for changeset reconstruction: clustering by (author, log, window), and confidence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{FileRev, reconstruct};
use crate::rcs::RevNum;

fn fr(path: &str, date: i64, author: &str, log: &str) -> FileRev {
    FileRev {
        path: path.to_string(),
        rev: RevNum(vec![1, 1]),
        date,
        author: author.to_string(),
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
    assert_eq!(cs[0].author, "alice");
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

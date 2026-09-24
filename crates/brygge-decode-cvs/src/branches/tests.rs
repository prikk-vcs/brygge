//! Unit tests for the branch-point rule (RFC 013 D-3, earliest-covering-changeset).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{Parent, choose_parent};

#[test]
fn no_covered_file_gives_no_parent() {
    assert_eq!(choose_parent(&[], 5), Parent::Root);
}

#[test]
fn an_exact_cut_takes_the_earliest_changeset_where_every_file_is_at_its_branch_point() {
    // file a is at its branch point on [1, 4), file b on [3, 6): both at 3 (and 4 for both; 5 for b only).
    let runs = [(1, 4), (3, 6)];
    assert_eq!(
        choose_parent(&runs, 6),
        Parent::Changeset {
            index: 3,
            exact: true
        }
    );
}

#[test]
fn a_single_file_is_exact_at_the_changeset_that_brought_its_branch_point() {
    assert_eq!(
        choose_parent(&[(2, 5)], 5),
        Parent::Changeset {
            index: 2,
            exact: true
        }
    );
}

#[test]
fn a_cut_across_changesets_is_approximate_with_the_most_files_and_the_earliest_on_ties() {
    // a on [0, 2), b on [1, 3), c on [4, 5): no changeset has all three. Changeset 1 has a and b (2 files);
    // changeset 4 has only c. The most is at 1.
    let runs = [(0, 2), (1, 3), (4, 5)];
    assert_eq!(
        choose_parent(&runs, 5),
        Parent::Changeset {
            index: 1,
            exact: false
        }
    );
    // a tie between changesets 0 (a) and 4 (c): the earliest.
    let tie = [(0, 1), (4, 5)];
    assert_eq!(
        choose_parent(&tie, 5),
        Parent::Changeset {
            index: 0,
            exact: false
        }
    );
}

#[test]
fn a_run_that_reaches_the_end_of_the_line_counts_up_to_the_last_changeset() {
    // one file's run ends at the line's end (its next revision never arrives), the other's is short.
    let runs = [(0, 1), (2, 4)];
    assert_eq!(
        choose_parent(&runs, 4),
        Parent::Changeset {
            index: 0,
            exact: false
        }
    );
}

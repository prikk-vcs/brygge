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

// ---- the parent line and the import order (RFC 013 §6) ------------------------------------------------

use std::collections::BTreeMap;

use super::{LineRef, OnBranch, PointState, majority_line, resolve};
use crate::rcs::RevNum;

fn on(file: usize, line: LineRef, state: PointState) -> OnBranch {
    OnBranch {
        file,
        branch_id: RevNum(vec![1, 2, 2]),
        point: RevNum(vec![1, 2]),
        line,
        state,
    }
}

fn named(n: &str) -> LineRef {
    LineRef::Named(n.to_string())
}

fn live(file: usize, line: LineRef) -> OnBranch {
    on(file, line, PointState::Live)
}

#[test]
fn the_majority_line_wins_ties_go_to_the_main_line_then_the_lower_name() {
    let pick = |v: &[OnBranch]| majority_line(&v.iter().collect::<Vec<_>>());
    assert_eq!(
        pick(&[
            live(0, named("B")),
            live(1, named("B")),
            live(2, LineRef::Main)
        ]),
        Some(named("B"))
    );
    assert_eq!(
        pick(&[live(0, named("B")), live(1, LineRef::Main)]),
        Some(LineRef::Main)
    );
    assert_eq!(
        pick(&[live(0, named("B")), live(1, named("A"))]),
        Some(named("A"))
    );
    // a dead branch point does not vote, and a line that cannot be a parent never wins
    assert_eq!(
        pick(&[on(0, LineRef::Main, PointState::Dead), live(1, named("B"))]),
        Some(named("B"))
    );
    assert_eq!(
        pick(&[
            live(0, LineRef::Unknown),
            live(1, LineRef::Unknown),
            live(2, named("Z"))
        ]),
        Some(named("Z"))
    );
    assert_eq!(pick(&[live(0, LineRef::Unknown)]), None);
    assert_eq!(pick(&[]), None);
}

fn entries(v: &[(&str, Vec<OnBranch>)]) -> BTreeMap<String, Vec<OnBranch>> {
    v.iter()
        .map(|(n, e)| ((*n).to_string(), e.clone()))
        .collect()
}

#[test]
fn branches_are_imported_after_their_parent_line_by_name_and_the_rest_are_blocked() {
    let r = resolve(&entries(&[
        ("A", vec![live(0, named("Z"))]), // hangs from Z, which sorts later
        ("Z", vec![live(0, LineRef::Main)]),
        ("M", vec![live(0, named("A"))]),       // hangs from A
        ("U", vec![live(0, LineRef::Unknown)]), // on an unnamed line
        ("X", vec![live(0, named("Y"))]),       // a cycle: X on Y, Y on X
        ("Y", vec![live(0, named("X"))]),
        ("S", vec![live(0, named("S"))]),    // on itself
        ("G", vec![live(0, named("Nope"))]), // on a branch that does not exist
        ("D", vec![on(0, LineRef::Unknown, PointState::Dead)]), // only added on the branch
    ]));
    assert_eq!(
        r.order,
        ["D", "Z", "A", "M"],
        "parents first, by name within a wave"
    );
    assert_eq!(r.parent["A"], named("Z"));
    assert_eq!(r.parent["M"], named("A"));
    assert_eq!(r.parent["Z"], LineRef::Main);
    assert_eq!(r.parent["D"], LineRef::Main, "no file constrains it");
    assert_eq!(
        r.blocked.iter().map(String::as_str).collect::<Vec<_>>(),
        ["G", "S", "U", "X", "Y"]
    );
}

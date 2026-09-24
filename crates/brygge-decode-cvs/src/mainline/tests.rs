#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::rcs::Revision;

fn rev(s: &str) -> RevNum {
    RevNum(s.split('.').map(|p| p.parse().unwrap()).collect())
}

#[test]
fn trunk_revisions_are_always_main_line() {
    assert!(is_main_line(&rev("1.1"), None));
    assert!(is_main_line(&rev("1.7"), Some(&rev("1.1.1"))));
}

#[test]
fn vendor_branch_revisions_are_main_line_only_while_set() {
    let vb = rev("1.1.1");
    assert!(is_main_line(&rev("1.1.1.1"), Some(&vb)));
    assert!(is_main_line(&rev("1.1.1.2"), Some(&vb)));
    assert!(!is_main_line(&rev("1.1.1.1"), None));
}

#[test]
fn a_branch_of_a_branch_is_not_directly_on_the_vendor_branch() {
    let vb = rev("1.1.1");
    assert!(!is_direct_branch_revision(&rev("1.1.1.1.1.1"), &vb));
}

#[test]
fn an_ordinary_branch_revision_is_not_main_line() {
    // 1.2.2.1 is a branch off 1.2, not the vendor branch 1.1.1.
    assert!(!is_main_line(&rev("1.2.2.1"), Some(&rev("1.1.1"))));
    assert!(!is_main_line(&rev("1.2.2.1"), None));
}

#[test]
fn branch_point_and_branch_of_are_inverse_ish() {
    assert_eq!(branch_point(&rev("1.1.1")), rev("1.1"));
    assert_eq!(branch_point(&rev("1.2.2")), rev("1.2"));
    assert_eq!(branch_of(&rev("1.2.2.1")), rev("1.2.2"));
}

#[test]
fn magic_branch_number_inserts_a_zero_before_the_last_component() {
    assert_eq!(magic_branch_number(&rev("1.2.2")), Some(rev("1.2.0.2")));
    assert_eq!(magic_branch_number(&rev("1.1.1")), Some(rev("1.1.0.1")));
    assert_eq!(magic_branch_number(&rev("1.1")), None); // too short to be a branch id
}

#[test]
fn branch_symbol_resolves_through_the_magic_number_and_the_literal_number() {
    let file = crate::rcs::RcsFile {
        head: rev("1.2"),
        symbols: vec![("REL_BRANCH".to_string(), rev("1.2.0.2"))],
        expand: None,
        branch: None,
        revisions: BTreeMap::new(),
    };
    assert_eq!(
        branch_symbol(&file, &rev("1.2.2")),
        Some(BranchSymbol::Magic("REL_BRANCH"))
    );
    assert_eq!(branch_symbol(&file, &rev("1.2.4")), None);
    // A vendor branch is named by its literal (odd-length) number.
    let vendor = crate::rcs::RcsFile {
        symbols: vec![("vendor".to_string(), rev("1.1.1"))],
        ..file
    };
    assert_eq!(
        branch_symbol(&vendor, &rev("1.1.1")),
        Some(BranchSymbol::Literal("vendor"))
    );
}

#[test]
fn vendor_branch_first_revision_is_found_via_the_branch_points_branches_list() {
    let mut revisions = BTreeMap::new();
    revisions.insert(
        rev("1.1"),
        Revision {
            date: 0,
            author: "a".into(),
            state: "Exp".into(),
            branches: vec![rev("1.1.1.1")],
            next: None,
            log: Vec::new(),
            text: Vec::new(),
        },
    );
    let file = crate::rcs::RcsFile {
        head: rev("1.1"),
        symbols: Vec::new(),
        expand: None,
        branch: Some(rev("1.1.1")),
        revisions,
    };
    assert_eq!(
        vendor_branch_first_revision(&file, &rev("1.1.1")),
        Some(rev("1.1.1.1"))
    );
}

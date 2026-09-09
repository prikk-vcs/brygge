//! Unit tests for branch/tag convention classification.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use brygge_ir::RefKind;

use super::classify;
use crate::options::LayoutPolicy;

#[test]
fn classifies_the_standard_layout() {
    let l = LayoutPolicy::default();

    let trunk = classify("trunk/src/main.rs", &l).unwrap();
    assert_eq!(trunk.name, "trunk");
    assert_eq!(trunk.kind, RefKind::Branch);
    assert_eq!(trunk.prefix, "trunk");

    let branch = classify("branches/feature-x/src/main.rs", &l).unwrap();
    assert_eq!(branch.name, "feature-x");
    assert_eq!(branch.kind, RefKind::Branch);
    assert_eq!(branch.prefix, "branches/feature-x");

    let tag = classify("tags/v1.0/readme", &l).unwrap();
    assert_eq!(tag.name, "v1.0");
    assert_eq!(tag.kind, RefKind::Tag);
    assert_eq!(tag.prefix, "tags/v1.0");
}

#[test]
fn a_path_outside_the_convention_is_unclassified() {
    let l = LayoutPolicy::default();
    assert!(classify("some/other/path", &l).is_none());
    assert!(classify("readme.txt", &l).is_none());
    // the bare container directory with no child is not a branch.
    assert!(classify("branches", &l).is_none());
}

#[test]
fn a_custom_policy_is_honoured() {
    let l = LayoutPolicy {
        trunk: "main".to_string(),
        branches: "b".to_string(),
        tags: "t".to_string(),
    };
    assert_eq!(classify("main/x", &l).unwrap().name, "trunk");
    assert_eq!(classify("b/foo/x", &l).unwrap().name, "foo");
    // the default names no longer match.
    assert!(classify("trunk/x", &l).is_none());
}

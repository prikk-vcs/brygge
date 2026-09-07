//! Tests for the format-safety gate (RFC 005 §1). Pure text-parsing — no Mercurial needed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::Error;

#[test]
fn a_typical_modern_zlib_repo_passes() {
    // The requirement set a default `hg init` writes for a zlib, generaldelta, sparserevlog store.
    let body =
        "dotencode\nfncache\ngeneraldelta\nrevlogv1\nsparserevlog\nstore\npersistent-nodemap\n";
    assert!(check(body).is_ok());
}

#[test]
fn empty_and_blank_lines_are_ignored() {
    assert!(check("").is_ok());
    assert!(check("\n\nrevlogv1\n\nstore\n\n").is_ok());
}

#[test]
fn zstd_compression_is_refused_as_unsupported_format() {
    let body = "revlogv1\nstore\nrevlog-compression-zstd\n";
    match check(body) {
        Err(Error::UnsupportedFormat { requirement, .. }) => {
            assert_eq!(requirement, "revlog-compression-zstd");
        }
        other => panic!("expected UnsupportedFormat, got {other:?}"),
    }
}

#[test]
fn revlogv2_and_treemanifest_and_narrow_are_refused() {
    for req in [
        "revlogv2",
        "changelogv2",
        "treemanifest",
        "narrowhg",
        "narrow",
    ] {
        let body = format!("revlogv1\nstore\n{req}\n");
        assert!(
            matches!(check(&body), Err(Error::UnsupportedFormat { .. })),
            "{req} should be an unsupported format"
        );
    }
}

#[test]
fn largefiles_and_lfs_are_floor_refusals() {
    for req in ["largefiles", "lfs"] {
        let body = format!("revlogv1\nstore\n{req}\n");
        match check(&body) {
            Err(Error::FloorRefusal { feature, .. }) => assert_eq!(feature, req),
            other => panic!("expected FloorRefusal for {req}, got {other:?}"),
        }
    }
}

#[test]
fn an_unknown_requirement_is_refused_not_ignored() {
    let body = "revlogv1\nstore\nsome-future-requirement-we-do-not-know\n";
    match check(body) {
        Err(Error::UnsupportedFormat {
            requirement,
            reason,
        }) => {
            assert_eq!(requirement, "some-future-requirement-we-do-not-know");
            assert!(reason.contains("unknown"));
        }
        other => panic!("expected UnsupportedFormat for an unknown requirement, got {other:?}"),
    }
}

#[test]
fn the_first_unsupported_requirement_is_reported() {
    // Order in the file is preserved; the first refusal wins.
    let body = "revlogv1\nstore\ntreemanifest\nrevlog-compression-zstd\n";
    match check(body) {
        Err(Error::UnsupportedFormat { requirement, .. }) => {
            assert_eq!(requirement, "treemanifest")
        }
        other => panic!("got {other:?}"),
    }
}

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
fn blank_lines_are_ignored() {
    assert!(check("\n\nrevlogv1\n\nstore\nfncache\n\n").is_ok());
}

#[test]
fn zstd_compression_is_supported() {
    // Modern hg defaults to zstd; the reader reads it (ruzstd), so the gate must accept it.
    let body = "revlogv1\nstore\nfncache\nrevlog-compression-zstd\ngeneraldelta\n";
    assert!(check(body).is_ok());
}

// ---- RFC 013 D-1: the store encoding is read from the requirements -----------------------------------------

#[test]
fn dotencode_is_read_from_the_requirements() {
    let with = check("dotencode\nfncache\nrevlogv1\nstore\n").unwrap();
    assert!(with.dotencode);
    // `fncache` without `dotencode`: the leading dot/space rule does not apply.
    let without = check("fncache\nrevlogv1\nstore\n").unwrap();
    assert!(!without.dotencode);
}

#[test]
fn a_store_without_fncache_is_refused_by_name() {
    // Mercurial before 1.1 (2008): a different file-name encoding, which this reader does not implement.
    for body in [
        "revlogv1\nstore\n",
        "revlogv1\nstore\ndotencode\n",
        "",
        "revlogv1\n",
    ] {
        match check(body) {
            Err(Error::UnsupportedFormat {
                requirement,
                reason,
            }) => {
                assert_eq!(requirement, "store-without-fncache", "{body:?}");
                assert!(reason.contains("before 1.1"), "{reason}");
            }
            other => panic!("expected a store-without-fncache refusal for {body:?}, got {other:?}"),
        }
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

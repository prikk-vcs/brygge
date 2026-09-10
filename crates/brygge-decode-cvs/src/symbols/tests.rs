//! Unit tests for symbol collection and branch/tag classification.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::collect;
use crate::rcs::{RcsFile, RevNum};
use crate::scan::CvsFile;

fn file(path: &str, symbols: &[(&str, &[u32])]) -> CvsFile {
    CvsFile {
        path: path.to_string(),
        rcs: RcsFile {
            head: RevNum(vec![1, 1]),
            symbols: symbols
                .iter()
                .map(|(n, r)| (n.to_string(), RevNum(r.to_vec())))
                .collect(),
            expand: None,
            revisions: BTreeMap::new(),
        },
    }
}

#[test]
fn classifies_tags_and_magic_branch_numbers() {
    let files = vec![
        file("a.c", &[("REL_1", &[1, 2]), ("DEV", &[1, 2, 0, 2])]),
        file("b.c", &[("REL_1", &[1, 3])]),
    ];
    let syms = collect(&files);
    // sorted by name: DEV, REL_1.
    let dev = syms.iter().find(|s| s.name == "DEV").unwrap();
    assert!(dev.is_branch, "1.2.0.2 is a magic branch number");
    let rel = syms.iter().find(|s| s.name == "REL_1").unwrap();
    assert!(!rel.is_branch, "1.2 / 1.3 are plain tagged revisions");
    // REL_1 names a revision in both files.
    assert_eq!(rel.named.len(), 2);
}

//! Unit tests for the pure mapping helpers. The real round trip (decode → encode → `git fast-import`)
//! lives in `tests/round_trip.rs`, against real fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use brygge_ir::Text;

#[test]
fn regular_executable_and_symlink_modes_are_accepted() {
    assert_eq!(GitMode::from_unix(0o100_644).unwrap(), GitMode::Regular);
    assert_eq!(GitMode::from_unix(0o100_755).unwrap(), GitMode::Executable);
    assert_eq!(GitMode::from_unix(0o120_000).unwrap(), GitMode::Symlink);
}

#[test]
fn a_gitlink_mode_is_refused_by_name_not_coerced() {
    let err = GitMode::from_unix(0o160_000).unwrap_err();
    assert!(err.contains("160000"), "{err}");
}

#[test]
fn a_present_identity_needs_nothing_supplied() {
    let identity = Identity {
        name: Text::utf8("A U Thor"),
        email: Some(Text::utf8("a@example.com")),
    };
    let mut supplied = Supplied::default();
    let line = identity_line(Some(&identity), &mut supplied, "author");
    assert_eq!(line, "A U Thor <a@example.com>");
    assert!(supplied.trailer().is_none());
}

#[test]
fn a_missing_email_is_supplied_as_empty_and_marked() {
    let identity = Identity {
        name: Text::utf8("A U Thor"),
        email: None,
    };
    let mut supplied = Supplied::default();
    let line = identity_line(Some(&identity), &mut supplied, "author");
    assert_eq!(line, "A U Thor <>");
    assert_eq!(supplied.trailer().unwrap(), "Encoded-supplied: author");
}

#[test]
fn a_missing_identity_entirely_is_supplied_and_marked() {
    let mut supplied = Supplied::default();
    let line = identity_line(None, &mut supplied, "committer");
    assert_eq!(line, "unknown <>");
    assert_eq!(supplied.trailer().unwrap(), "Encoded-supplied: committer");
}

#[test]
fn a_newline_in_a_name_is_folded_to_a_space() {
    let identity = Identity {
        name: Text::utf8("A U\nThor"),
        email: Some(Text::utf8("a@example.com")),
    };
    let mut supplied = Supplied::default();
    let line = identity_line(Some(&identity), &mut supplied, "author");
    assert_eq!(line, "A U Thor <a@example.com>");
}

#[test]
fn a_present_time_with_a_negative_offset_formats_correctly() {
    let time = Time {
        seconds: 1_700_000_000,
        offset_minutes: Some(-330),
    };
    let mut supplied = Supplied::default();
    let w = when(Some(time), &mut supplied, "author-time", "author-time-zone");
    assert_eq!(w, "1700000000 -0530");
    assert!(supplied.trailer().is_none());
}

#[test]
fn a_missing_time_zone_is_supplied_as_plus_0000_and_marked() {
    let time = Time {
        seconds: 1_700_000_000,
        offset_minutes: None,
    };
    let mut supplied = Supplied::default();
    let w = when(Some(time), &mut supplied, "author-time", "author-time-zone");
    assert_eq!(w, "1700000000 +0000");
    assert_eq!(
        supplied.trailer().unwrap(),
        "Encoded-supplied: author-time-zone"
    );
}

#[test]
fn a_missing_time_entirely_is_supplied_as_epoch_zero_and_both_marked() {
    let mut supplied = Supplied::default();
    let w = when(None, &mut supplied, "author-time", "author-time-zone");
    assert_eq!(w, "0 +0000");
    assert_eq!(
        supplied.trailer().unwrap(),
        "Encoded-supplied: author-time, author-time-zone"
    );
}

#[test]
fn an_ordinary_path_is_unquoted() {
    assert_eq!(path_token("src/main.rs"), "src/main.rs");
    assert_eq!(
        path_token("a path with spaces.txt"),
        "a path with spaces.txt"
    );
}

#[test]
fn a_path_starting_with_a_quote_is_quoted_and_escaped() {
    assert_eq!(path_token("\"quoted\".txt"), "\"\\\"quoted\\\".txt\"");
}

#[test]
fn a_path_with_a_newline_is_quoted_and_escaped() {
    assert_eq!(path_token("a\nb"), "\"a\\nb\"");
}

#[test]
fn a_stated_status_has_no_derived_trailer() {
    assert!(derived_trailer("atom", &EpistemicStatus::Stated).is_none());
}

#[test]
fn a_derived_status_renders_its_kind_and_params() {
    use brygge_ir::{Derivation, DerivationKind};
    use std::collections::BTreeMap;
    let mut params = BTreeMap::new();
    params.insert("window_secs".to_string(), "180".to_string());
    let status = EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::ReconstructedChangeset,
        by: "brygge-decode-cvs".to_string(),
        decoder_version: "0.5.0".to_string(),
        params,
        confidence: None,
    });
    let line = derived_trailer("atom", &status).unwrap();
    assert_eq!(
        line,
        "Brygge-Derived: atom reconstructed-changeset window_secs=180"
    );
}

//! Unit tests for property classification and the `svn:date` → epoch conversion.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn props(pairs: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect()
}

#[test]
fn parses_a_svn_date_to_epoch_seconds() {
    // 1970-01-01T00:00:00Z is epoch 0.
    assert_eq!(parse_svn_date("1970-01-01T00:00:00.000000Z"), Some(0));
    // 2000-01-01T00:00:00Z = 946684800.
    assert_eq!(
        parse_svn_date("2000-01-01T00:00:00.000000Z"),
        Some(946_684_800)
    );
    // A known SVN timestamp: 2024-09-08T12:00:00Z.
    assert_eq!(
        parse_svn_date("2024-09-08T12:00:00.000000Z"),
        Some(1_725_796_800)
    );
}

#[test]
fn a_malformed_date_yields_no_time_rather_than_a_wrong_one() {
    assert_eq!(parse_svn_date("not-a-date"), None);
    assert_eq!(parse_svn_date("2024-13-01T00:00:00Z"), None); // month 13
    assert_eq!(parse_svn_date(""), None);
}

#[test]
fn file_mode_reflects_special_and_executable() {
    assert_eq!(file_mode(&props(&[])), MODE_REGULAR);
    assert_eq!(file_mode(&props(&[("svn:executable", "*")])), MODE_EXEC);
    assert_eq!(file_mode(&props(&[("svn:special", "*")])), MODE_SYMLINK);
    // special wins over executable (a symlink is not an exec regular file).
    assert_eq!(
        file_mode(&props(&[("svn:special", "*"), ("svn:executable", "*")])),
        MODE_SYMLINK
    );
}

#[test]
fn symlink_target_strips_the_link_prefix() {
    assert_eq!(symlink_target(b"link target/path"), b"target/path".to_vec());
    // content without the prefix is carried verbatim (defensive).
    assert_eq!(symlink_target(b"target/path"), b"target/path".to_vec());
}

#[test]
fn externals_are_refused() {
    assert!(check_externals("trunk", &props(&[("svn:externals", "^/lib lib")])).is_err());
    assert!(check_externals("trunk", &props(&[("svn:ignore", "*.o")])).is_ok());
}

#[test]
fn loss_classification_sorts_properties_into_categories() {
    let l = classify_loss(&props(&[
        ("svn:mergeinfo", "/trunk:1-3"),
        ("svn:keywords", "Id"),
        ("my:custom", "x"),
        ("svn:executable", "*"),
        ("svn:date", "..."),
    ]));
    assert!(l.mergeinfo);
    assert!(l.workflow); // keywords
    assert!(l.custom); // my:custom
    // svn:executable and svn:date are not loss.
}

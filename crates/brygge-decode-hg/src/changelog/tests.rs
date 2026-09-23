//! Tests for changelog parsing (RFC 005).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::parse;

#[test]
fn non_utf8_changeset_metadata_is_an_unsupported_format() {
    // RFC 010 CR-15: non-UTF-8 changeset metadata is a refusal (CL-08 exit 20), not a generic read
    // failure (exit 1) — the CLI maps `UnsupportedFormat` to `FLOOR_REFUSAL`. This build carries text
    // as `str`; RFC 011 will carry it byte-exact.
    let mut text = vec![0u8; 40]; // a plausible-looking manifest-hex line's worth of bytes
    text.push(0xFF); // not valid UTF-8 anywhere in the buffer
    match parse(&text) {
        Err(crate::Error::UnsupportedFormat { requirement, .. }) => {
            assert_eq!(requirement, "non-UTF-8 changeset metadata");
        }
        other => panic!("expected UnsupportedFormat, got {other:?}"),
    }
}

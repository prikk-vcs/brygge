//! Tests for changelog parsing (RFC 005, byte-exact text per RFC 011 §5).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::parse;

fn changelog_bytes(user: &[u8], date_line: &[u8], description: &[u8]) -> Vec<u8> {
    let mut text = vec![b'0'; 40]; // a valid (all-zero) 40-hex manifest node
    text.push(b'\n');
    text.extend_from_slice(user);
    text.push(b'\n');
    text.extend_from_slice(date_line);
    text.push(b'\n');
    text.push(b'\n');
    text.extend_from_slice(description);
    text
}

#[test]
fn non_utf8_user_and_description_round_trip_as_raw_bytes() {
    // RFC 011 §5: changeset text is now carried byte-exact — non-UTF-8 content in the user line or the
    // description is no longer a refusal, only a byte sequence the caller carries as `Text`.
    let user = b"Non\xffUtf8 User <user@example.com>";
    let description = b"a message with a stray byte: \xff and more text";
    let bytes = changelog_bytes(user, b"0 0", description);
    let cs = parse(&bytes).expect("non-UTF-8 changeset metadata is no longer refused");
    assert_eq!(cs.user, user);
    assert_eq!(cs.description, description);
}

#[test]
fn a_non_ascii_manifest_line_is_still_rejected() {
    // The manifest-hex line is structurally fixed-format and must stay parseable regardless of the
    // changeset's own text encoding.
    let mut text = vec![0xFFu8; 40];
    text.push(b'\n');
    text.extend_from_slice(b"user\n0 0\n\nmsg");
    assert!(parse(&text).is_err());
}

#[test]
fn a_non_ascii_date_line_is_rejected() {
    let text = changelog_bytes(b"user <u@x>", b"\xff\xff not valid utf-8", b"msg");
    assert!(parse(&text).is_err());
}

#[test]
fn branch_defaults_and_is_extracted_from_extras() {
    let bytes = changelog_bytes(b"user <u@x>", b"1700000000 0", b"msg");
    let cs = parse(&bytes).unwrap();
    assert_eq!(cs.branch, "default");
    assert_eq!(cs.time, 1_700_000_000);

    let bytes = changelog_bytes(b"user <u@x>", b"1700000000 0 branch:feature", b"msg");
    let cs = parse(&bytes).unwrap();
    assert_eq!(cs.branch, "feature");
}

// ---- review 009 R-2: extras are bytes, decoded per changelog.py's decodeextra/_string_unescape -------

/// Mercurial's `changelog.py::_string_escape`: the exact four-case escaping its own writer performs
/// (`\\`, `\n`, `\r`, `\0`) — used here to build realistic *encoded* extras bytes from an arbitrary
/// *decoded* value, so the round-trip test constructs input the way `hg` itself would.
fn string_escape(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    for &b in value {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0 => out.extend_from_slice(b"\\0"),
            _ => out.push(b),
        }
    }
    out
}

#[test]
fn a_binary_extra_value_round_trips_byte_exact() {
    // transplant_source stores a raw 20-byte node as an extra value — arbitrary bytes, not UTF-8, and
    // including bytes (like NUL) that `_string_escape` must protect.
    let value: Vec<u8> = (0u8..=255).collect();
    let mut field = b"transplant_source:".to_vec();
    field.extend_from_slice(&string_escape(&value));
    let date_line = [b"1700000000 0 ".to_vec(), field].concat();
    let bytes = changelog_bytes(b"user <u@x>", &date_line, b"msg");
    let cs = parse(&bytes).unwrap();
    let (key, got) = cs
        .extras
        .iter()
        .find(|(k, _)| k == b"transplant_source")
        .unwrap();
    assert_eq!(key, b"transplant_source");
    assert_eq!(got, &value);
}

#[test]
fn a_backslash_in_an_extra_value_round_trips() {
    // `_string_escape` writes a real backslash as `\\` (two bytes); the reader must undo that exactly.
    let date_line = b"1700000000 0 key:a\\\\b".to_vec();
    let bytes = changelog_bytes(b"user <u@x>", &date_line, b"msg");
    let cs = parse(&bytes).unwrap();
    let (_, v) = cs.extras.iter().find(|(k, _)| k == b"key").unwrap();
    assert_eq!(v, b"a\\b");
}

#[test]
fn a_nul_escaped_extra_value_round_trips() {
    // `_string_escape` writes a real NUL byte as `\0` (backslash, '0') — distinct from the `\0`
    // separator between extras entries, which is a literal NUL byte, never this two-char escape.
    let date_line = b"1700000000 0 key:a\\0b".to_vec();
    let bytes = changelog_bytes(b"user <u@x>", &date_line, b"msg");
    let cs = parse(&bytes).unwrap();
    let (_, v) = cs.extras.iter().find(|(k, _)| k == b"key").unwrap();
    assert_eq!(v, b"a\0b");
}

#[test]
fn an_entry_with_no_colon_is_rejected() {
    let date_line = b"1700000000 0 nocolonhere".to_vec();
    let bytes = changelog_bytes(b"user <u@x>", &date_line, b"msg");
    assert!(parse(&bytes).is_err());
}

#[test]
fn a_duplicate_extra_key_is_rejected() {
    let date_line = b"1700000000 0 a:1\0a:2".to_vec();
    let bytes = changelog_bytes(b"user <u@x>", &date_line, b"msg");
    assert!(parse(&bytes).is_err());
}

#[test]
fn an_unrecognized_escape_is_rejected() {
    // `\q` is not among the escapes Mercurial's own writer ever produces — refused, not passed through
    // literally the way Python's `codecs.escape_decode` would (review 009 R-2: deliberately stricter).
    let date_line = b"1700000000 0 key:bad\\qescape".to_vec();
    let bytes = changelog_bytes(b"user <u@x>", &date_line, b"msg");
    assert!(parse(&bytes).is_err());
}

#[test]
fn octal_and_hex_escapes_decode_correctly() {
    let date_line = b"1700000000 0 key:\\101\\x42".to_vec(); // \101 = 'A', \x42 = 'B'
    let bytes = changelog_bytes(b"user <u@x>", &date_line, b"msg");
    let cs = parse(&bytes).unwrap();
    let (_, v) = cs.extras.iter().find(|(k, _)| k == b"key").unwrap();
    assert_eq!(v, b"AB");
}

#[test]
fn a_non_utf8_time_or_timezone_token_is_rejected_even_with_valid_extras() {
    let bytes = changelog_bytes(b"user <u@x>", b"\xff\xff 0 branch:feature", b"msg");
    assert!(parse(&bytes).is_err());
}

#[test]
fn a_signed_hex_escape_is_rejected_like_python_does() {
    // `u8::from_str_radix("+f", 16)` succeeds, but Python's `escape_decode` raises on `\x+f`
    // (review 009 F-1): both bytes must be ASCII hex digits.
    for bad in [&b"\\x+f"[..], b"\\x-1", b"\\xg0", b"\\x 1"] {
        let mut date_line = b"1700000000 0 key:".to_vec();
        date_line.extend_from_slice(bad);
        let bytes = changelog_bytes(b"user <u@x>", &date_line, b"msg");
        assert!(
            matches!(parse(&bytes), Err(crate::Error::Read(_))),
            "{bad:?} must be a Read error"
        );
    }
    // The valid neighbours still decode, including upper-case hex digits.
    let bytes = changelog_bytes(b"user <u@x>", b"1700000000 0 key:\\x4F\\x0a", b"msg");
    let cs = parse(&bytes).unwrap();
    let (_, v) = cs.extras.iter().find(|(k, _)| k == b"key").unwrap();
    assert_eq!(v, b"O\n");
}

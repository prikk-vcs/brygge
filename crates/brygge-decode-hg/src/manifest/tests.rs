//! Tests for manifest parsing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::parse;
use crate::Error;

const NODE: &str = "0123456789abcdef0123456789abcdef01234567";

fn line(path: &[u8], flag: &[u8]) -> Vec<u8> {
    let mut l = path.to_vec();
    l.push(0);
    l.extend_from_slice(NODE.as_bytes());
    l.extend_from_slice(flag);
    l.push(b'\n');
    l
}

#[test]
fn a_utf8_manifest_parses_with_modes() {
    let mut text = line(b"a.txt", b"");
    text.extend(line("dir/\u{e9}.sh".as_bytes(), b"x"));
    let m = parse(&text).unwrap();
    assert_eq!(m.len(), 2);
    assert!(m.contains_key("dir/\u{e9}.sh"));
}

#[test]
fn a_non_utf8_path_is_a_named_refusal_showing_the_invalid_bytes_as_hex_escapes() {
    // release-prep §2: exit-20 refusal (`non-utf8-path`), like Git and CVS — not a generic read error.
    let text = line(b"ok/\xff\xfe-name", b"");
    match parse(&text) {
        Err(Error::FloorRefusal { feature, reason }) => {
            assert_eq!(feature, crate::floor::NON_UTF8_PATH);
            assert_eq!(feature, "non-utf8-path");
            assert!(
                reason.contains("ok/\\xff\\xfe-name"),
                "reason was {reason:?}"
            );
            assert!(
                !reason.contains('\u{fffd}'),
                "no lossy substitution: {reason:?}"
            );
        }
        other => panic!("expected FloorRefusal(non-utf8-path), got {other:?}"),
    }
}

//! Unit tests for the dumpstream reader: framing, property blocks, and the delta/format refusals.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{NodeAction, NodeKind, PropBlock, PropChange, TextBody, parse_dump};

/// Build a property block body (`K <len>\n<key>\nV <len>\n<value>\n...PROPS-END\n`).
fn props_block(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in pairs {
        b.extend(format!("K {}\n", k.len()).into_bytes());
        b.extend(k.as_bytes());
        b.push(b'\n');
        b.extend(format!("V {}\n", v.len()).into_bytes());
        b.extend(v.as_bytes());
        b.push(b'\n');
    }
    b.extend(b"PROPS-END\n");
    b
}

fn header() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(b"SVN-fs-dump-format-version: 2\n\n");
    b.extend(b"UUID: 11111111-2222-3333-4444-555555555555\n\n");
    b
}

fn revision(num: u64, props: &[(&str, &str)]) -> Vec<u8> {
    let pblock = props_block(props);
    let mut b = Vec::new();
    b.extend(format!("Revision-number: {num}\n").into_bytes());
    b.extend(format!("Prop-content-length: {}\n", pblock.len()).into_bytes());
    b.extend(format!("Content-length: {}\n", pblock.len()).into_bytes());
    b.push(b'\n');
    b.extend(pblock);
    b.extend(b"\n");
    b
}

fn add_file(path: &str, content: &[u8]) -> Vec<u8> {
    let pblock = props_block(&[]);
    let mut b = Vec::new();
    b.extend(format!("Node-path: {path}\n").into_bytes());
    b.extend(b"Node-kind: file\nNode-action: add\n");
    b.extend(format!("Prop-content-length: {}\n", pblock.len()).into_bytes());
    b.extend(format!("Text-content-length: {}\n", content.len()).into_bytes());
    b.extend(format!("Content-length: {}\n", pblock.len() + content.len()).into_bytes());
    b.push(b'\n');
    b.extend(pblock);
    b.extend(content);
    b.extend(b"\n\n");
    b
}

#[test]
fn parses_a_minimal_dump() {
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(
        1,
        &[
            ("svn:author", "alice"),
            ("svn:date", "2024-01-02T00:00:00.000000Z"),
            ("svn:log", "first"),
        ],
    ));
    d.extend(add_file("file.txt", b"hello\n"));

    let dump = parse_dump(&d).unwrap();
    assert_eq!(
        dump.uuid.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
    assert_eq!(dump.revisions.len(), 2);
    let r1 = &dump.revisions[1];
    assert_eq!(r1.number, 1);
    // the node attached to revision 1.
    assert_eq!(r1.nodes.len(), 1);
    let n = &r1.nodes[0];
    assert_eq!(n.path, "file.txt");
    assert_eq!(n.kind, Some(NodeKind::File));
    assert_eq!(n.action, NodeAction::Add);
    assert_eq!(n.text, Some(TextBody::Full(b"hello\n".to_vec())));
    // a present-but-empty property block is Some(empty), not None.
    assert_eq!(n.props, Some(PropBlock::Full(Vec::new())));
}

#[test]
fn reads_author_date_log_from_revision_props() {
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(1, &[("svn:author", "bob"), ("svn:log", "msg")]));
    let dump = parse_dump(&d).unwrap();
    let props = &dump.revisions[1].props;
    let author = props.iter().find(|(k, _)| k == "svn:author").unwrap();
    assert_eq!(author.1, b"bob");
}

#[test]
fn refuses_an_unknown_format_version() {
    let d = b"SVN-fs-dump-format-version: 9\n\n".to_vec();
    let err = parse_dump(&d).unwrap_err();
    assert!(matches!(err, crate::Error::UnsupportedFormat { .. }));
}

#[test]
fn a_delta_node_is_kept_as_the_dump_states_it() {
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(1, &[("svn:log", "x")]));
    // svndiff version 0 with no windows (an empty target): the header alone is a valid delta.
    d.extend(
        b"Node-path: f\nNode-kind: file\nNode-action: add\nText-delta: true\nProp-delta: true\n",
    );
    d.extend(b"Prop-content-length: 10\nText-content-length: 4\nContent-length: 14\n\nPROPS-END\nSVN\0\n\n");
    let dump = parse_dump(&d).unwrap();
    let n = &dump.revisions[1].nodes[0];
    assert_eq!(n.text, Some(TextBody::Delta(b"SVN\0".to_vec())));
    assert_eq!(n.props, Some(PropBlock::Delta(Vec::new())));
}

#[test]
fn a_property_delta_keeps_its_sets_and_deletes_in_order_and_a_full_block_refuses_a_deletion() {
    let block = b"K 1\na\nV 1\n1\nD 1\nb\nK 1\nc\nV 0\n\nPROPS-END\n";
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(1, &[("svn:log", "x")]));
    let node = |delta: bool| {
        let mut n = b"Node-path: f\nNode-kind: file\nNode-action: change\n".to_vec();
        if delta {
            n.extend(b"Prop-delta: true\n");
        }
        n.extend(
            format!(
                "Prop-content-length: {0}\nContent-length: {0}\n\n",
                block.len()
            )
            .into_bytes(),
        );
        n.extend(block);
        n.extend(b"\n\n");
        n
    };
    let mut with_delta = d.clone();
    with_delta.extend(node(true));
    let parsed = parse_dump(&with_delta).unwrap();
    assert_eq!(
        parsed.revisions[1].nodes[0].props,
        Some(PropBlock::Delta(vec![
            PropChange::Set("a".into(), b"1".to_vec()),
            PropChange::Delete("b".into()),
            PropChange::Set("c".into(), Vec::new()),
        ]))
    );
    let mut without = d;
    without.extend(node(false));
    assert!(matches!(parse_dump(&without), Err(crate::Error::Read(_))));
}

#[test]
fn svndiff_version_1_and_2_are_refused_by_name_before_any_body_is_used() {
    for (v, name) in [(1u8, "version 1 (zlib)"), (2u8, "version 2 (lz4)")] {
        let mut d = header();
        d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
        d.extend(revision(1, &[("svn:log", "x")]));
        d.extend(b"Node-path: f\nNode-kind: file\nNode-action: add\nText-delta: true\n");
        d.extend(b"Text-content-length: 4\nContent-length: 4\n\nSVN");
        d.push(v);
        d.extend(b"\n\n");
        match parse_dump(&d).unwrap_err() {
            crate::Error::UnsupportedFormat { what, reason } => {
                assert!(what.contains(name), "{what}");
                assert!(reason.contains("svnrdump"), "{reason}");
            }
            other => panic!("expected UnsupportedFormat, got {other:?}"),
        }
    }
}

#[test]
fn the_checksum_headers_are_kept() {
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(1, &[("svn:log", "x")]));
    d.extend(b"Node-path: f\nNode-kind: file\nNode-action: add\nText-content-md5: aa\nText-content-sha1: bb\n");
    d.extend(b"Text-delta-base-md5: cc\nText-delta-base-sha1: dd\nText-copy-source-md5: ee\nText-copy-source-sha1: ff\n");
    d.extend(b"Text-content-length: 1\nContent-length: 1\n\nx\n\n");
    let c = &parse_dump(&d).unwrap().revisions[1].nodes[0].checksums;
    assert_eq!(c.content_md5.as_deref(), Some("aa"));
    assert_eq!(c.content_sha1.as_deref(), Some("bb"));
    assert_eq!(c.base_md5.as_deref(), Some("cc"));
    assert_eq!(c.base_sha1.as_deref(), Some("dd"));
    assert_eq!(c.copy_md5.as_deref(), Some("ee"));
    assert_eq!(c.copy_sha1.as_deref(), Some("ff"));
}

#[test]
fn a_declared_length_past_the_end_is_a_typed_refusal_not_a_panic() {
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(1, &[("svn:log", "x")]));
    // Content-length claims more bytes than are present.
    d.extend(b"Node-path: f\nNode-kind: file\nNode-action: add\nText-content-length: 9999\nContent-length: 9999\n\nshort");
    let err = parse_dump(&d).unwrap_err();
    assert!(matches!(err, crate::Error::Read(_)));
}

#[test]
fn a_non_utf8_node_path_is_a_read_error_that_shows_the_bytes_escaped() {
    // A dumpstream's paths are UTF-8 by the format's own definition, so this is a malformed dump (`Read`,
    // not a repository-shape refusal) — but the message must show the offending bytes losslessly.
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(1, &[("svn:log", "x")]));
    d.extend(b"Node-path: caf\xE9/\xFFx\nNode-kind: file\nNode-action: add\n\n");
    let err = parse_dump(&d).unwrap_err();
    match err {
        crate::Error::Read(m) => {
            assert!(m.contains("caf\\xE9/\\xFFx"), "escaped bytes shown: {m}");
            assert!(!m.contains('\u{FFFD}'), "no lossy substitution: {m}");
        }
        other => panic!("expected Read, got {other:?}"),
    }
}

#[test]
fn escaping_keeps_valid_utf8_verbatim_and_escapes_only_invalid_bytes() {
    use super::escape_invalid_utf8;
    assert_eq!(escape_invalid_utf8("café/日本".as_bytes()), "café/日本");
    assert_eq!(escape_invalid_utf8(b"a\xC3"), "a\\xC3");
    assert_eq!(escape_invalid_utf8(b"\xFF\xFEok"), "\\xFF\\xFEok");
}

#[test]
fn an_overlong_offending_line_is_truncated_in_the_message() {
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(1, &[("svn:log", "x")]));
    d.extend(b"Node-path: ");
    d.extend(std::iter::repeat_n(0xFFu8, 100_000));
    d.extend(b"\nNode-kind: file\nNode-action: add\n\n");
    match parse_dump(&d).unwrap_err() {
        crate::Error::Read(m) => {
            assert!(m.len() < 2_000, "message stays bounded: {} bytes", m.len())
        }
        other => panic!("expected Read, got {other:?}"),
    }
}

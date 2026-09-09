//! Unit tests for the dumpstream reader: framing, property blocks, and the delta/format refusals.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{NodeAction, NodeKind, parse_dump};

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
    assert_eq!(dump.format_version, 2);
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
    assert_eq!(n.text.as_deref(), Some(&b"hello\n"[..]));
    // a present-but-empty property block is Some(empty), not None.
    assert_eq!(n.props.as_deref(), Some(&[][..]));
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
fn refuses_a_delta_node() {
    let mut d = header();
    d.extend(revision(0, &[("svn:date", "2024-01-01T00:00:00.000000Z")]));
    d.extend(revision(1, &[("svn:log", "x")]));
    // a node declaring Text-delta: true is refused before its body is read.
    d.extend(b"Node-path: f\nNode-kind: file\nNode-action: add\nText-delta: true\n");
    d.extend(b"Text-content-length: 4\nContent-length: 4\n\nDATA\n\n");
    let err = parse_dump(&d).unwrap_err();
    assert!(matches!(err, crate::Error::UnsupportedFormat { .. }));
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

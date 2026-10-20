//! Mutation tests for the dumpstream reader (RFC 014 D-1): `parse_dump_with` must never panic on a
//! mutated dump, and must never reconstruct content wildly larger than the mutated input could
//! plausibly have produced — the "bounds-checked against the bytes actually present" property the
//! module's own doc already claims, now tested against format-aware boundary writes at the length
//! headers (`Content-length`, `Prop-content-length`, `Text-content-length`, the `K`/`V` property lengths)
//! rather than only by hand-written malformed-input tests.
//!
//! Review 061 F-1: the parser alone is not the whole untrusted-input surface here — "the base lookup, the
//! checksum checks, the `svn:special` toggle and the budget all live in the tree application, not the
//! parser." A second test below runs `decode_with` (not only `parse_dump_with`) over a five-revision dump
//! that exercises a `Text-delta` node, a copy, a replace, and an `svnrdump`-style `Prop-delta` with a `D`
//! entry — the shapes the first, parser-only seed never had a reason to include.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};
use brygge_mutate::ops::{Encoding, InterestingPosition};

use super::parse_dump_with;
use crate::source::Limits;

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

/// Small ceilings, so a boundary value near the ceiling is reachable without gigabyte fixtures.
fn small_limits() -> Limits {
    Limits {
        max_dump_bytes: 64 * 1024,
        max_node_text_bytes: 4096,
        max_stderr_bytes: 1024,
        max_svnadmin_version_bytes: 1024,
    }
}

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

/// A minimal valid dump: the format header, one revision (with a property), and one file-add node (with
/// a property and text) — enough surface for every one of the interesting positions below to land inside
/// a real header or a real property's length field.
fn seed() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend(b"SVN-fs-dump-format-version: 2\n\n");
    d.extend(b"UUID: 11111111-2222-3333-4444-555555555555\n\n");

    let rprops = props_block(&[("svn:log", "first")]);
    d.extend(b"Revision-number: 1\n");
    d.extend(format!("Prop-content-length: {}\n", rprops.len()).into_bytes());
    d.extend(format!("Content-length: {}\n", rprops.len()).into_bytes());
    d.push(b'\n');
    d.extend(&rprops);
    d.push(b'\n');

    let nprops = props_block(&[("svn:mime-type", "text/plain")]);
    let content = b"hello\n";
    d.extend(b"Node-path: file.txt\n");
    d.extend(b"Node-kind: file\nNode-action: add\n");
    d.extend(format!("Prop-content-length: {}\n", nprops.len()).into_bytes());
    d.extend(format!("Text-content-length: {}\n", content.len()).into_bytes());
    d.extend(format!("Content-length: {}\n", nprops.len() + content.len()).into_bytes());
    d.push(b'\n');
    d.extend(&nprops);
    d.extend(content);
    d.extend(b"\n\n");
    d
}

/// The byte offset and digit-run length of the `nth` (0-based) occurrence of `marker` followed by ASCII
/// decimal digits, searching from `from`.
fn find_decimal_after(data: &[u8], marker: &[u8], from: usize, nth: usize) -> (usize, usize) {
    let mut search_from = from;
    let mut at = 0;
    for _ in 0..=nth {
        let rel = data
            .get(search_from..)
            .unwrap_or(&[])
            .windows(marker.len())
            .position(|w| w == marker)
            .unwrap_or_else(|| {
                panic!(
                    "fixture marker {:?} not found",
                    String::from_utf8_lossy(marker)
                )
            });
        at = search_from + rel + marker.len();
        search_from = at;
    }
    let len = data
        .get(at..)
        .unwrap_or(&[])
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    (at, len)
}

fn position(data: &[u8], marker: &[u8], nth: usize, ceiling: Option<u64>) -> InterestingPosition {
    let (offset, len) = find_decimal_after(data, marker, 0, nth);
    InterestingPosition {
        offset,
        encoding: Encoding::Decimal { len },
        ceiling,
    }
}

fn interesting_positions(data: &[u8], limits: &Limits) -> Vec<InterestingPosition> {
    vec![
        // The revision's Content-length/Prop-content-length (1st occurrence of each) and the node's
        // (2nd occurrence) — the dump has exactly these two Content-length-bearing blocks.
        position(data, b"Content-length: ", 0, None),
        position(data, b"Content-length: ", 1, None),
        position(data, b"Prop-content-length: ", 0, None),
        position(data, b"Prop-content-length: ", 1, None),
        // The node's Text-content-length, checked against max_node_text_bytes for a fulltext.
        position(
            data,
            b"Text-content-length: ",
            0,
            Some(limits.max_node_text_bytes as u64),
        ),
        // The two K/V lengths in the node's property block ("svn:mime-type" / "text/plain").
        position(data, b"K 13\n", 0, None), // "svn:mime-type".len() == 13
        position(data, b"V 10\n", 0, None), // "text/plain".len() == 10
    ]
}

/// A generous amplification bound, as the other parsers' mutation modules use: the operators grow a seed
/// by at most a few dozen bytes per call, so a factor of 64 has ample margin before it would ever fire on
/// legitimate growth, while still catching a genuine amplification bug.
fn amplification_ceiling(input_len: usize) -> usize {
    input_len.saturating_mul(64).max(4096)
}

fn check(limits: &Limits, bytes: &[u8]) -> Outcome {
    let ceiling = amplification_ceiling(bytes.len());
    match parse_dump_with(bytes, limits) {
        Ok(dump) => {
            let total: usize = dump
                .revisions
                .iter()
                .flat_map(|r| &r.nodes)
                .filter_map(|n| match &n.text {
                    Some(super::TextBody::Full(t)) => Some(t.len()),
                    Some(super::TextBody::Delta(t)) => Some(t.len()),
                    None => None,
                })
                .sum();
            if total > ceiling {
                return Outcome::Violation(format!(
                    "decoded {total} bytes of node text from a {}-byte dump (ceiling {ceiling})",
                    bytes.len()
                ));
            }
            Outcome::Ok
        }
        Err(_) => Outcome::Ok,
    }
}

#[test]
fn parsing_a_mutated_dumpstream_never_panics_and_never_amplifies() {
    let data = seed();
    let limits = small_limits();
    let positions = interesting_positions(&data, &limits);
    let seeds = [Seed::new(&data, &positions)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "parsing_a_mutated_dumpstream_never_panics_and_never_amplifies",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check(&limits, bytes),
    );
}

// ---- decode_with: the tree-application layer, not only the parser (review 061 F-1) -------------------

/// A minimal svndiff (version 0) delta: one window, no source copy, that writes `new_text` as new data —
/// always valid regardless of the base, which is all a "Text-delta: true" node needs to be structurally
/// legitimate (the handoff's interesting positions, not round-trip fidelity, are the point here).
fn minimal_svndiff(new_text: &[u8]) -> Vec<u8> {
    fn varint(mut n: u64) -> Vec<u8> {
        let mut groups = vec![(n & 0x7f) as u8];
        n >>= 7;
        while n > 0 {
            groups.push((n & 0x7f) as u8 | 0x80);
            n >>= 7;
        }
        groups.reverse();
        groups
    }
    let mut instructions = vec![2 << 6]; // opcode "new data", long form (length follows as a varint)
    instructions.extend(varint(new_text.len() as u64));
    let mut d = b"SVN\0".to_vec();
    for n in [
        0u64,
        0,
        new_text.len() as u64,
        instructions.len() as u64,
        new_text.len() as u64,
    ] {
        d.extend(varint(n));
    }
    d.extend(&instructions);
    d.extend(new_text);
    d
}

/// A node's body: a property block (when `props` is `Some`) and/or text, with the right
/// `Content-length`/`Prop-content-length`/`Text-content-length` headers. Returns the header+body bytes
/// to append after the node's own `Node-*` lines, and — when `text` is `Some` — the text's byte offset
/// within the returned bytes (so callers needing to probe inside an svndiff delta can find it).
fn node_body(props: Option<&[u8]>, text: Option<&[u8]>) -> (Vec<u8>, Option<usize>) {
    let prop_len = props.map_or(0, <[u8]>::len);
    let text_len = text.map_or(0, <[u8]>::len);
    let mut b = Vec::new();
    if props.is_some() {
        b.extend(format!("Prop-content-length: {prop_len}\n").into_bytes());
    }
    if text.is_some() {
        b.extend(format!("Text-content-length: {text_len}\n").into_bytes());
    }
    if props.is_some() || text.is_some() {
        b.extend(format!("Content-length: {}\n", prop_len + text_len).into_bytes());
    }
    b.push(b'\n');
    if let Some(p) = props {
        b.extend(p);
    }
    let text_offset = text.map(|_| b.len());
    if let Some(t) = text {
        b.extend(t);
    }
    b.extend(b"\n\n");
    (b, text_offset)
}

/// A five-revision dump: rev 1 adds `a.txt` (fulltext, one property); rev 2 changes it via a
/// `Text-delta: true` node; rev 3 adds `b.txt` as a pure copy of `a.txt@2`; rev 4 replaces `a.txt`
/// (fulltext); rev 5 changes `a.txt`'s properties via an `svnrdump`-style `Prop-delta: true` block with a
/// `D` entry (deleting the property rev 1 set). Returns the dump bytes and the svndiff delta's byte
/// offset within them (rev 2's node body).
fn rich_seed() -> (Vec<u8>, usize) {
    let mut d = Vec::new();
    d.extend(b"SVN-fs-dump-format-version: 2\n\n");
    d.extend(b"UUID: 11111111-2222-3333-4444-555555555555\n\n");

    let rev1_props = props_block(&[("svn:keywords", "Id")]);
    let text1 = b"line one\nline two\n".to_vec();
    d.extend(b"Revision-number: 1\n");
    d.extend(b"Prop-content-length: 10\nContent-length: 10\n\n");
    d.extend(b"PROPS-END\n\n"); // the revision's own (empty) props
    d.extend(b"Node-path: a.txt\nNode-kind: file\nNode-action: add\n");
    let (body1, _) = node_body(Some(&rev1_props), Some(&text1));
    d.extend(body1);

    let text2 = b"line one\nline TWO\n".to_vec();
    let delta2 = minimal_svndiff(&text2);
    d.extend(b"Revision-number: 2\n");
    d.extend(b"Prop-content-length: 10\nContent-length: 10\n\n");
    d.extend(b"PROPS-END\n\n");
    d.extend(b"Node-path: a.txt\nNode-kind: file\nNode-action: change\nText-delta: true\n");
    let (body2, delta_offset_in_body2) = node_body(None, Some(&delta2));
    let delta_offset = d.len() + delta_offset_in_body2.expect("text present");
    d.extend(&body2);

    d.extend(b"Revision-number: 3\n");
    d.extend(b"Prop-content-length: 10\nContent-length: 10\n\n");
    d.extend(b"PROPS-END\n\n");
    d.extend(b"Node-path: b.txt\nNode-kind: file\nNode-action: add\n");
    d.extend(b"Node-copyfrom-rev: 2\nNode-copyfrom-path: a.txt\n");
    d.push(b'\n'); // no Content-length at all: a pure copy, no own content

    let text4 = b"replaced content\n".to_vec();
    d.extend(b"Revision-number: 4\n");
    d.extend(b"Prop-content-length: 10\nContent-length: 10\n\n");
    d.extend(b"PROPS-END\n\n");
    d.extend(b"Node-path: a.txt\nNode-kind: file\nNode-action: replace\n");
    let (body4, _) = node_body(None, Some(&text4));
    d.extend(body4);

    d.extend(b"Revision-number: 5\n");
    d.extend(b"Prop-content-length: 10\nContent-length: 10\n\n");
    d.extend(b"PROPS-END\n\n");
    d.extend(b"Node-path: a.txt\nNode-kind: file\nNode-action: change\nProp-delta: true\n");
    let prop_delta = {
        let mut p = Vec::new();
        p.extend(b"D 12\nsvn:keywords\n"); // delete the property rev 1 set
        p.extend(b"PROPS-END\n");
        p
    };
    let (body5, _) = node_body(Some(&prop_delta), None);
    d.extend(body5);

    (d, delta_offset)
}

fn rich_interesting_positions(delta_offset: usize) -> Vec<InterestingPosition> {
    // The embedded svndiff delta's five window-header varints (review 061 F-1: "positions on Text-delta's
    // svndiff window integers inside the node body"). All five are single-byte minimal varints here
    // (small values), in order: source offset, source length, target length, instructions length, new
    // data length.
    let start = delta_offset + 4; // past the "SVN\0" header
    (start..start + 5)
        .map(|offset| InterestingPosition {
            offset,
            encoding: Encoding::BigEndianVarint { len: 1 },
            ceiling: None,
        })
        .collect()
}

fn decode_check(path: &std::path::Path, limits: &crate::source::Limits, bytes: &[u8]) -> Outcome {
    if std::fs::write(path, bytes).is_err() {
        return Outcome::Ok; // a filesystem hiccup is not a finding
    }
    let source = crate::Source::DumpFile(path.to_path_buf());
    let _ = crate::decode::decode_with(
        &source,
        &crate::Options::default(),
        limits,
        &brygge_ir::Storage::Memory,
    );
    Outcome::Ok
}

#[test]
fn decoding_a_mutated_dumpstream_never_panics() {
    let (data, delta_offset) = rich_seed();
    let limits = small_limits();
    let positions = rich_interesting_positions(delta_offset);
    let seeds = [Seed::new(&data, &positions)];
    let dir = std::env::temp_dir().join(format!(
        "brygge-mutate-svn-dumpstream-{}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let path = dir.clone();
    // Confirm the unmutated seed decodes cleanly before fuzzing it, so a finding is known to come from a
    // mutation, not from the fixture being wrong.
    assert!(
        crate::decode::decode_with(
            &crate::Source::DumpFile({
                std::fs::write(&path, &data).expect("write seed dump");
                path.clone()
            }),
            &crate::Options::default(),
            &limits,
            &brygge_ir::Storage::Memory,
        )
        .is_ok(),
        "the unmutated rich seed must decode cleanly"
    );
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "decoding_a_mutated_dumpstream_never_panics",
        &seeds,
        // decode_with needs a real file per call; reused (overwritten) sequentially, never raced, but
        // still real I/O, so this runs at a smaller multiple than the parser-only test above.
        50,
        40,
        TIME_BUDGET,
        |_seed_index, bytes| decode_check(&path, &limits, bytes),
    );
    let _ = std::fs::remove_file(&path);
}

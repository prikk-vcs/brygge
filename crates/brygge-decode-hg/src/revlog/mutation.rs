//! Mutation tests for the revlog reader (RFC 014 D-1): `Revlog::from_bytes`, then `revision_with` over
//! every revision, must never panic on a mutated store, and must never reconstruct a revision larger
//! than its declared ceiling (`limits.max_revision_bytes`) — the property the decompression bomb fix
//! (RUSTSEC-2026-0344) exists to guarantee.
//!
//! Review 061 F-1: a first version of this module seeded only raw (`'u'`-marked) chunks, so `decompress`'s
//! `b'x'` (zlib) and `0x28` (zstd) arms — RUSTSEC-2026-0344's class — were never fed a single mutated
//! byte. Three seeds now: the original raw snapshot-plus-delta, a real zlib chunk, and a real zstd chunk
//! (a hand-built frame, `Compressed_Block` with a literals section and no sequences — the simplest block
//! shape that still has every field RFC 014 D-1's table names: a frame descriptor, a window descriptor, a
//! block header with its own type and size, and a literals-section size). The zstd seed gets dedicated
//! boundary positions on exactly those fields; all three seeds get the same index-entry positions
//! (`comp_len`, `uncomp_len`, `base_rev`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Write as _;
use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};
use brygge_mutate::ops::{Encoding, InterestingPosition};
use sha1_checked::{CollisionResult, Digest, Sha1};

use super::{ENTRY_LEN, FLAG_INLINE, Limits, Revlog};

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

fn small_limits() -> Limits {
    Limits {
        max_revision_bytes: 4096,
    }
}

/// Mercurial's own hash (`storageutil.hashrevisionsha1`): the two parent nodes in ascending order, then
/// the text.
fn node_of(p1: [u8; 20], p2: [u8; 20], text: &[u8]) -> [u8; 20] {
    let (lo, hi) = if p1 <= p2 { (p1, p2) } else { (p2, p1) };
    let mut hasher = Sha1::new();
    Digest::update(&mut hasher, lo);
    Digest::update(&mut hasher, hi);
    Digest::update(&mut hasher, text);
    let CollisionResult::Ok(digest) = hasher.try_finalize() else {
        panic!("unexpected SHA-1 collision in a hand-built seed");
    };
    digest.as_slice().try_into().expect("sha1 is 20 bytes")
}

struct EntryFields {
    offset_flags: u64,
    comp_len: u32,
    uncomp_len: u32,
    base_rev: i32,
    link_rev: i32,
    p1: i32,
    p2: i32,
    node: [u8; 20],
}

fn entry_bytes(f: EntryFields) -> [u8; ENTRY_LEN] {
    let mut e = [0u8; ENTRY_LEN];
    e[0..8].copy_from_slice(&f.offset_flags.to_be_bytes());
    e[8..12].copy_from_slice(&f.comp_len.to_be_bytes());
    e[12..16].copy_from_slice(&f.uncomp_len.to_be_bytes());
    e[16..20].copy_from_slice(&f.base_rev.to_be_bytes());
    e[20..24].copy_from_slice(&f.link_rev.to_be_bytes());
    e[24..28].copy_from_slice(&f.p1.to_be_bytes());
    e[28..32].copy_from_slice(&f.p2.to_be_bytes());
    e[32..52].copy_from_slice(&f.node);
    e
}

/// A `mpatch` hunk: replace `base[start..end]` with `data`.
fn hunk(start: u32, end: u32, data: &[u8]) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend(start.to_be_bytes());
    h.extend(end.to_be_bytes());
    h.extend((data.len() as u32).to_be_bytes());
    h.extend(data);
    h
}

/// A single-revision inline revlog around one already-compressed `chunk` (its first byte is the
/// compression marker `decompress` switches on), with the index-entry positions every seed shares
/// (`comp_len`, `uncomp_len`, `base_rev`, each relative to this one entry's start, i.e. `0`).
fn single_rev_seed(chunk: &[u8], uncomp_len: u32) -> (Vec<u8>, [usize; 3]) {
    let version_and_flags = u64::from(1u32 | FLAG_INLINE) << 32;
    let entry = entry_bytes(EntryFields {
        offset_flags: version_and_flags,
        comp_len: chunk.len() as u32,
        uncomp_len,
        base_rev: 0,
        link_rev: 0,
        p1: -1,
        p2: -1,
        node: [0; 20], // placeholder: the caller overwrites it with `with_node` once the real text is known
    });
    let mut buf = entry.to_vec();
    buf.extend(chunk);
    (buf, [8, 12, 16]) // comp_len, uncomp_len, base_rev offsets within the entry
}

/// The original seed: rev 0 a raw (`'u'`-marked) snapshot, rev 1 a raw `mpatch` delta against it — both
/// the decompression markers and the delta-apply path in one buffer.
fn raw_snapshot_and_delta_seed() -> (Vec<u8>, Vec<InterestingPosition>) {
    let text0 = b"hello\n".to_vec();
    let node0 = node_of([0; 20], [0; 20], &text0);
    let chunk0 = {
        let mut c = vec![b'u'];
        c.extend(&text0);
        c
    };
    let version_and_flags = u64::from(1u32 | FLAG_INLINE) << 32;
    let entry0 = entry_bytes(EntryFields {
        offset_flags: version_and_flags,
        comp_len: chunk0.len() as u32,
        uncomp_len: text0.len() as u32,
        base_rev: 0,
        link_rev: 0,
        p1: -1,
        p2: -1,
        node: node0,
    });

    let text1 = b"hello, world\n".to_vec();
    let delta1 = hunk(0, text0.len() as u32, &text1);
    let node1 = node_of(node0, [0; 20], &text1);
    let chunk1 = {
        let mut c = vec![b'u'];
        c.extend(&delta1);
        c
    };
    let entry1 = entry_bytes(EntryFields {
        offset_flags: 0,
        comp_len: chunk1.len() as u32,
        uncomp_len: text1.len() as u32,
        base_rev: 0,
        link_rev: 1,
        p1: 0,
        p2: -1,
        node: node1,
    });

    let mut buf = Vec::new();
    buf.extend(entry0);
    buf.extend(&chunk0);
    buf.extend(entry1);
    buf.extend(&chunk1);

    let mut positions = Vec::new();
    for entry_start in [0usize, ENTRY_LEN + chunk0.len()] {
        positions.push(index_position(entry_start + 8, None)); // comp_len
        positions.push(index_position(
            entry_start + 12,
            Some(small_limits().max_revision_bytes),
        )); // uncomp_len
        positions.push(index_position(entry_start + 16, None)); // base_rev
    }
    (buf, positions)
}

fn index_position(offset: usize, ceiling: Option<u64>) -> InterestingPosition {
    InterestingPosition {
        offset,
        encoding: Encoding::Binary {
            width: 4,
            big_endian: true,
        },
        ceiling,
    }
}

/// A real zlib chunk (`flate2`, matching the `b'x'` marker `decompress` switches on).
fn zlib_chunk_seed() -> (Vec<u8>, Vec<InterestingPosition>) {
    let text = b"zlib snapshot text, long enough to compress meaningfully, repeated a bit: zlib zlib zlib\n".to_vec();
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&text).expect("zlib encode");
    let chunk = enc.finish().expect("zlib finish");
    assert_eq!(chunk.first(), Some(&b'x'), "zlib marker byte");
    let (buf, offsets) = single_rev_seed(&chunk, text.len() as u32);
    let node = node_of([0; 20], [0; 20], &text);
    let buf = with_node(buf, node);
    let positions = vec![
        index_position(offsets[0], None),
        index_position(offsets[1], Some(small_limits().max_revision_bytes)),
        index_position(offsets[2], None),
    ];
    (buf, positions)
}

/// A real zstd chunk: a hand-built frame, a `Compressed_Block` with a literals section and **no**
/// sequences (the simplest block that still has a frame descriptor, a window descriptor, a block header
/// with its own type-and-size, and a literals-section size field — every field RFC 014 D-1's table
/// names). Verified to round-trip before being used as a seed.
fn zstd_chunk_seed() -> (Vec<u8>, Vec<InterestingPosition>) {
    let literal = b"zstd text!!!".to_vec(); // 12 bytes: fits the 5-bit Raw_Literals size field (max 31)
    let lit_header = (literal.len() as u8) << 3; // Literals_Block_Type = Raw (00), Size_Format = 00
    let mut block = vec![lit_header];
    block.extend(&literal);
    block.push(0x00); // Number_of_Sequences = 0: no sequences section at all

    let mut chunk = Vec::new();
    chunk.extend(0xFD2F_B528u32.to_le_bytes()); // magic
    let frame_descriptor_offset = chunk.len();
    chunk.push(2 << 6); // Frame_Content_Size is 4 bytes; Single_Segment NOT set (a Window_Descriptor follows)
    let window_descriptor_offset = chunk.len();
    chunk.push(0u8); // Window_Descriptor: exponent 0, mantissa 0 (the smallest window)
    chunk.extend((literal.len() as u32).to_le_bytes()); // Frame_Content_Size
    let block_header_offset = chunk.len();
    chunk.extend(zstd_block_header(true, 2, block.len() as u32)); // last, Compressed_Block
    let lit_header_offset = chunk.len();
    chunk.extend(&block);
    assert_eq!(chunk.first(), Some(&0x28), "zstd frame magic's first byte");

    let (buf, offsets) = single_rev_seed(&chunk, literal.len() as u32);
    let node = node_of([0; 20], [0; 20], &literal);
    let buf = with_node(buf, node);

    let mut positions = vec![
        index_position(offsets[0], None),
        index_position(offsets[1], Some(small_limits().max_revision_bytes)),
        index_position(offsets[2], None),
    ];
    // The zstd-internal positions review 061 F-1 asks for, each offset by the entry's length (the chunk
    // starts right after the 64-byte index entry).
    let base = ENTRY_LEN;
    positions.push(InterestingPosition {
        offset: base + frame_descriptor_offset,
        encoding: Encoding::Binary {
            width: 1,
            big_endian: true,
        },
        ceiling: None,
    });
    positions.push(InterestingPosition {
        offset: base + window_descriptor_offset,
        encoding: Encoding::Binary {
            width: 1,
            big_endian: true,
        },
        ceiling: None,
    });
    positions.push(InterestingPosition {
        offset: base + block_header_offset,
        encoding: Encoding::Binary {
            width: 3,
            big_endian: true,
        },
        ceiling: None,
    });
    positions.push(InterestingPosition {
        offset: base + lit_header_offset,
        encoding: Encoding::Binary {
            width: 1,
            big_endian: true,
        },
        ceiling: None,
    });
    (buf, positions)
}

fn zstd_block_header(last: bool, block_type: u8, size21: u32) -> [u8; 3] {
    [
        u8::from(last) | (block_type << 1) | (((size21 & 0x1f) as u8) << 3),
        ((size21 >> 5) & 0xff) as u8,
        ((size21 >> 13) & 0xff) as u8,
    ]
}

/// Overwrite `single_rev_seed`'s placeholder node (computed before the real text was known) with the
/// correct one, so the seed round-trips and verifies.
fn with_node(mut buf: Vec<u8>, node: [u8; 20]) -> Vec<u8> {
    buf[32..52].copy_from_slice(&node);
    buf
}

fn check(limits: &Limits, bytes: &[u8]) -> Outcome {
    let Ok(revlog) = Revlog::from_bytes(bytes.to_vec(), Vec::new()) else {
        return Outcome::Ok;
    };
    for rev in 0..revlog.len() {
        match revlog.revision_with(rev, limits) {
            Ok(text) if text.len() as u64 > limits.max_revision_bytes => {
                return Outcome::Violation(format!(
                    "revision_with({rev}) reconstructed {} bytes, past the {}-byte ceiling",
                    text.len(),
                    limits.max_revision_bytes
                ));
            }
            Ok(_) | Err(_) => {}
        }
    }
    Outcome::Ok
}

#[test]
fn reading_a_mutated_revlog_never_panics_and_never_exceeds_its_ceiling() {
    let limits = small_limits();
    let (raw_data, raw_positions) = raw_snapshot_and_delta_seed();
    let (zlib_data, zlib_positions) = zlib_chunk_seed();
    let (zstd_data, zstd_positions) = zstd_chunk_seed();
    let seeds = [
        Seed::new(&raw_data, &raw_positions),
        Seed::new(&zlib_data, &zlib_positions),
        Seed::new(&zstd_data, &zstd_positions),
    ];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "reading_a_mutated_revlog_never_panics_and_never_exceeds_its_ceiling",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check(&limits, bytes),
    );
}

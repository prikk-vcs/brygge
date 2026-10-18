//! Mutation tests for the svndiff (version 0) reader (RFC 014 D-1): `apply` must never panic on a
//! mutated delta, and must never produce more target bytes than the ceiling it was given.
//!
//! Review 061 F-1: a single one-window seed left D-2's other shapes — several windows with sliding
//! source views, an overlapping target copy (a run), and an empty target — never mutated. Four seeds
//! now, each with its own base and its own window-header positions (both source fields' ceiling is that
//! seed's own base length).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};
use brygge_mutate::ops::{Encoding, InterestingPosition};

use super::apply;

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);
const MAX_TARGET: usize = 64;

const HEADER: &[u8] = b"SVN\0";

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

/// One instruction: opcode `op` (0 source, 1 target, 2 new), a length, and for a copy an offset.
fn ins(op: u8, len: usize, offset: Option<u64>) -> Vec<u8> {
    let mut i = Vec::new();
    if (1..64).contains(&len) {
        i.push((op << 6) | u8::try_from(len).unwrap());
    } else {
        i.push(op << 6);
        i.extend(varint(len as u64));
    }
    if let Some(o) = offset {
        i.extend(varint(o));
    }
    i
}

/// One window, appended to `delta`; records each of its five header varints' byte offset in `positions`
/// (source offset and length get `ceiling`, the other three get none).
fn push_window(
    delta: &mut Vec<u8>,
    positions: &mut Vec<InterestingPosition>,
    source: (u64, u64),
    target_len: u64,
    instructions: &[u8],
    new_data: &[u8],
    ceiling: Option<u64>,
) {
    let fields = [
        source.0,
        source.1,
        target_len,
        instructions.len() as u64,
        new_data.len() as u64,
    ];
    let field_ceilings = [ceiling, ceiling, Some(MAX_TARGET as u64), None, None];
    for (n, c) in fields.into_iter().zip(field_ceilings) {
        let offset = delta.len();
        let varint_bytes = varint(n);
        let len = varint_bytes.len();
        delta.extend(varint_bytes);
        positions.push(InterestingPosition {
            offset,
            encoding: Encoding::BigEndianVarint { len },
            ceiling: c,
        });
    }
    delta.extend(instructions);
    delta.extend(new_data);
}

/// One window: copies "hello" from the source view, appends " world" as new data — reproducing `base`
/// exactly.
fn one_window_seed() -> (Vec<u8>, Vec<InterestingPosition>, Vec<u8>) {
    let base = b"hello world".to_vec();
    let mut delta = HEADER.to_vec();
    let mut positions = Vec::new();
    let instructions = {
        let mut i = ins(0, 5, Some(0));
        i.extend(ins(2, 6, None));
        i
    };
    push_window(
        &mut delta,
        &mut positions,
        (0, 5),
        11,
        &instructions,
        b" world",
        Some(base.len() as u64),
    );
    (delta, positions, base)
}

/// Several windows, each with a different ("sliding") source view, together reproducing `base` exactly:
/// one pure source-copy window per 3-or-4-byte slice of it.
fn several_windows_sliding_source_seed() -> (Vec<u8>, Vec<InterestingPosition>, Vec<u8>) {
    let base = b"abcdefghij".to_vec();
    let mut delta = HEADER.to_vec();
    let mut positions = Vec::new();
    for (offset, len) in [(0u64, 3u64), (3, 3), (6, 4)] {
        push_window(
            &mut delta,
            &mut positions,
            (offset, len),
            len,
            &ins(0, len as usize, Some(0)),
            &[],
            Some(base.len() as u64),
        );
    }
    (delta, positions, base)
}

/// One window whose target copy overlaps what it is still writing (a run): one new byte, then a target
/// copy at offset 0 (back into what this window itself just produced) that reads past what existed when
/// the copy began — the "a target copy may overlap what it writes" shape (`svndiff::tests`).
fn overlapping_target_copy_seed() -> (Vec<u8>, Vec<InterestingPosition>, Vec<u8>) {
    let base = b"irrelevant".to_vec(); // this window's source view is empty; base content is unused
    let mut delta = HEADER.to_vec();
    let mut positions = Vec::new();
    let instructions = {
        let mut i = ins(2, 1, None); // new data: one byte
        i.extend(ins(1, 5, Some(0))); // target copy: offset 0, len 5 -> extends the one byte into a run
        i
    };
    push_window(
        &mut delta,
        &mut positions,
        (0, 0),
        6,
        &instructions,
        b"A",
        Some(base.len() as u64),
    );
    (delta, positions, base)
}

/// One window declaring an empty target: no instructions, no new data, an empty source view — the
/// smallest legal window (`svndiff::tests::an_empty_target_is_the_header_alone_or_a_window_of_nothing`'s
/// shape).
fn empty_target_seed() -> (Vec<u8>, Vec<InterestingPosition>, Vec<u8>) {
    let base = b"irrelevant".to_vec();
    let mut delta = HEADER.to_vec();
    let mut positions = Vec::new();
    push_window(
        &mut delta,
        &mut positions,
        (0, 0),
        0,
        &[],
        &[],
        Some(base.len() as u64),
    );
    (delta, positions, base)
}

fn check(base: &[u8], bytes: &[u8]) -> Outcome {
    match apply(bytes, base, MAX_TARGET) {
        Ok(target) if target.len() > MAX_TARGET => Outcome::Violation(format!(
            "apply produced {} bytes, past the {MAX_TARGET}-byte ceiling it was given",
            target.len()
        )),
        Ok(_) | Err(_) => Outcome::Ok,
    }
}

#[test]
fn applying_a_mutated_svndiff_delta_never_panics_and_never_exceeds_its_ceiling() {
    let (d0, p0, b0) = one_window_seed();
    let (d1, p1, b1) = several_windows_sliding_source_seed();
    let (d2, p2, b2) = overlapping_target_copy_seed();
    let (d3, p3, b3) = empty_target_seed();
    let bases = [b0, b1, b2, b3];
    let seeds = [
        Seed::new(&d0, &p0),
        Seed::new(&d1, &p1),
        Seed::new(&d2, &p2),
        Seed::new(&d3, &p3),
    ];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "applying_a_mutated_svndiff_delta_never_panics_and_never_exceeds_its_ceiling",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |seed_index, bytes| {
            let base = bases
                .get(seed_index)
                .expect("the harness never passes an out-of-range seed index");
            check(base, bytes)
        },
    );
}

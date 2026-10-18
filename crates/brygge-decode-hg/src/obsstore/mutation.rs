//! Mutation tests for the obsstore reader (RFC 014 D-1): `parse_bytes` must never panic on a mutated
//! store, and (since this reader extracts only a `HashSet` of precursor nodes, with no reconstructed
//! text to measure) must never walk past the file it was given — every record's declared `size` and
//! metadata table are bounds-checked against the bytes actually present, never pre-allocated from them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};
use brygge_mutate::ops::{Encoding, InterestingPosition};

use super::{Limits, parse_bytes};

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

fn small_limits() -> Limits {
    Limits {
        max_obsstore_bytes: 4096,
    }
}

/// One obsstore record: a precursor, its successors, optionally its parents (`None` writes the
/// `numpar == 3` "not recorded" sentinel), and metadata pairs.
fn record(
    precursor: [u8; 20],
    successors: &[[u8; 20]],
    parents: Option<&[[u8; 20]]>,
    meta: &[(&[u8], &[u8])],
) -> Vec<u8> {
    let (numpar, parent_bytes): (u8, Vec<u8>) = match parents {
        None => (3, Vec::new()),
        Some(ps) => (
            u8::try_from(ps.len()).unwrap(),
            ps.iter().flatten().copied().collect(),
        ),
    };
    let meta_table: Vec<u8> = meta
        .iter()
        .flat_map(|(k, v)| {
            [
                u8::try_from(k.len()).unwrap(),
                u8::try_from(v.len()).unwrap(),
            ]
        })
        .collect();
    let meta_bytes: Vec<u8> = meta.iter().flat_map(|(k, v)| [*k, *v].concat()).collect();

    let mut body = Vec::new();
    body.extend([0u8; 8]); // date (f64), not interpreted
    body.extend([0u8; 2]); // tz
    body.extend([0u8; 2]); // flags (0: 20-byte nodes)
    body.push(u8::try_from(successors.len()).unwrap());
    body.push(numpar);
    body.push(u8::try_from(meta.len()).unwrap());
    body.extend(precursor);
    body.extend(successors.iter().flatten());
    body.extend(parent_bytes);
    body.extend(meta_table);
    body.extend(meta_bytes);

    let size = 4 + body.len();
    let mut out = (size as u32).to_be_bytes().to_vec();
    out.extend(body);
    out
}

/// The real format version byte, then one amend-shaped record (a precursor, one successor, parents not
/// recorded, two metadata pairs) and one prune-shaped record (a precursor, no successor, one recorded
/// parent, one metadata pair) — between them, every structural branch `parse_bytes` has.
fn seed() -> Vec<u8> {
    let mut data = vec![1u8]; // FORMAT_VERSION
    data.extend(record(
        [0xAA; 20],
        &[[0xBB; 20]],
        None,
        &[(b"operation", b"amend"), (b"user", b"a@example.com")],
    ));
    data.extend(record(
        [0xCC; 20],
        &[],
        Some(&[[0u8; 20]]),
        &[(b"user", b"a@example.com")],
    ));
    data
}

fn interesting_positions() -> Vec<InterestingPosition> {
    // Offsets within the first record: size (u32 BE, at the record's own start, offset 1 in the file),
    // numsuc/numpar/nummeta (u8 each, right after the fixed 12-byte date/tz/flags prefix).
    vec![
        InterestingPosition {
            offset: 1,
            encoding: Encoding::Binary {
                width: 4,
                big_endian: true,
            },
            ceiling: None,
        },
        InterestingPosition {
            offset: 1 + 13, // numsuc
            encoding: Encoding::Binary {
                width: 1,
                big_endian: true,
            },
            ceiling: None,
        },
        InterestingPosition {
            offset: 1 + 14, // numpar
            encoding: Encoding::Binary {
                width: 1,
                big_endian: true,
            },
            ceiling: Some(3), // the NUMPAR_UNKNOWN sentinel
        },
        InterestingPosition {
            offset: 1 + 15, // nummeta
            encoding: Encoding::Binary {
                width: 1,
                big_endian: true,
            },
            ceiling: None,
        },
    ]
}

fn check(limits: &Limits, bytes: &[u8]) -> Outcome {
    // Nothing reconstructed to bound here (the reader extracts only a `HashSet` of 20-byte nodes, never
    // more of them than the file could hold) — a panic is the only thing worth catching.
    let _ = parse_bytes(bytes, limits);
    Outcome::Ok
}

#[test]
fn parsing_a_mutated_obsstore_never_panics() {
    let data = seed();
    let limits = small_limits();
    let positions = interesting_positions();
    let seeds = [Seed::new(&data, &positions)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "parsing_a_mutated_obsstore_never_panics",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check(&limits, bytes),
    );
}

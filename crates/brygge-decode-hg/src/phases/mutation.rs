//! Mutation tests for the phaseroots text parser (RFC 014 D-1, review 061 F-2): `parse_phaseroots` must
//! never panic on a mutated `phaseroots` file. Every field is a split of the input, like `changelog`'s
//! and `manifest`'s modules — no counted or length-prefixed field to target, so generic byte operators
//! carry this module. (`compute`'s inheritance pass, which needs a real changelog, is not re-exercised
//! here; the untrusted-bytes surface is the parse, which this does fuzz.)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};

use super::parse_phaseroots;

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

fn seed() -> Vec<u8> {
    let mut t = Vec::new();
    t.extend(b"1 0123456789abcdef0123456789abcdef01234567\n"); // draft
    t.extend(b"32 fedcba9876543210fedcba9876543210fedcba98\n"); // archived
    t
}

fn check(bytes: &[u8]) -> Outcome {
    let _ = parse_phaseroots(bytes);
    Outcome::Ok
}

#[test]
fn parsing_a_mutated_phaseroots_file_never_panics() {
    let data = seed();
    let seeds = [Seed::generic(&data)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "parsing_a_mutated_phaseroots_file_never_panics",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check(bytes),
    );
}

//! Mutation tests for the manifest text parser (RFC 014 D-1): `parse` must never panic on a mutated
//! manifest text. As `changelog`'s module: every field is a split of the input with no counted or
//! length-prefixed field to target, so generic byte operators carry this module.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};

use super::parse;

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

fn seed() -> Vec<u8> {
    let mut t = Vec::new();
    t.extend(b"dir/exe.sh\x000123456789abcdef0123456789abcdef01234567x\n");
    t.extend(b"dir/link\x000123456789abcdef0123456789abcdef01234567l\n");
    t.extend(b"plain.txt\x000123456789abcdef0123456789abcdef01234567\n");
    t
}

fn check(bytes: &[u8]) -> Outcome {
    let _ = parse(bytes);
    Outcome::Ok
}

#[test]
fn parsing_a_mutated_manifest_entry_never_panics() {
    let data = seed();
    let seeds = [Seed::generic(&data)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "parsing_a_mutated_manifest_entry_never_panics",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check(bytes),
    );
}

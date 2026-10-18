//! Mutation tests for the bookmarks text parser (RFC 014 D-1, review 061 F-2): `parse_bookmarks` must
//! never panic on a mutated `.hg/bookmarks` file. As `changelog`/`manifest`/`phases`'s modules: a split
//! of the input with no counted or length-prefixed field, so generic byte operators carry this module.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};

use super::parse_bookmarks;

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

fn seed() -> Vec<u8> {
    let mut t = Vec::new();
    t.extend(b"0123456789abcdef0123456789abcdef01234567 main\n");
    t.extend(b"fedcba9876543210fedcba9876543210fedcba98 feature/x\n");
    t
}

fn check(bytes: &[u8]) -> Outcome {
    let _ = parse_bookmarks(bytes);
    Outcome::Ok
}

#[test]
fn parsing_a_mutated_bookmarks_file_never_panics() {
    let data = seed();
    let seeds = [Seed::generic(&data)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "parsing_a_mutated_bookmarks_file_never_panics",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check(bytes),
    );
}

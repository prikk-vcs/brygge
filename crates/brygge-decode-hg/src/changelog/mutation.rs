//! Mutation tests for the changelog text parser (RFC 014 D-1): `parse` must never panic on a mutated
//! changeset text. Every field here is a substring or raw-byte split of the input — there is no
//! length-prefixed or counted field to probe with boundary values (unlike the binary formats this batch
//! also covers), so generic byte operators (the harness's default) are the whole of this module's job:
//! flips, truncation, splices and byte-run edits against the real line/`\0`/blank-line structure.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};

use super::parse;

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

fn seed() -> Vec<u8> {
    let mut t = Vec::new();
    t.extend(b"0123456789abcdef0123456789abcdef01234567\n"); // manifest-node-hex (40 hex)
    t.extend(b"Alice <alice@example.com>\n");
    t.extend(b"1700000000 0 branch:default\n");
    t.extend(b"file.txt\n");
    t.extend(b"\n");
    t.extend(b"a commit message\nwith a second line\n");
    t
}

fn check(bytes: &[u8]) -> Outcome {
    let _ = parse(bytes);
    Outcome::Ok
}

#[test]
fn parsing_a_mutated_changelog_entry_never_panics() {
    let data = seed();
    let seeds = [Seed::generic(&data)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "parsing_a_mutated_changelog_entry_never_panics",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check(bytes),
    );
}

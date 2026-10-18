#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use super::{Depth, Outcome, Seed, run};
use crate::ops::{Encoding, InterestingPosition};

#[test]
fn depth_from_env_defaults_to_quick() {
    // The harness must never run the deep multiplier unless explicitly asked: an ordinary `cargo test`
    // run (no env var set) is the common case, and it must stay fast.
    if std::env::var("BRYGGE_MUTATION_DEPTH").is_err() {
        assert_eq!(Depth::from_env(), Depth::Quick);
    }
}

#[test]
fn a_check_that_always_passes_never_panics() {
    let positions = [InterestingPosition {
        offset: 0,
        encoding: Encoding::Binary {
            width: 4,
            big_endian: true,
        },
        ceiling: Some(10),
    }];
    let seeds = [Seed::new(b"0123456789abcdef", &positions)];
    run(
        env!("CARGO_MANIFEST_DIR"),
        "a_check_that_always_passes_never_panics",
        &seeds,
        20,
        2,
        Duration::from_secs(2),
        |_seed_index, _bytes| Outcome::Ok,
    );
}

#[test]
#[should_panic(expected = "mutation finding in a_check_that_always_violates_is_reported")]
fn a_check_that_always_violates_is_reported() {
    let seeds = [Seed::generic(b"seed bytes")];
    run(
        env!("CARGO_MANIFEST_DIR"),
        "a_check_that_always_violates_is_reported",
        &seeds,
        5,
        2,
        Duration::from_secs(2),
        |_seed_index, _bytes| Outcome::Violation("always wrong, on purpose".to_string()),
    );
}

#[test]
#[should_panic(expected = "mutation finding in a_check_that_panics_is_caught_and_reported")]
fn a_check_that_panics_is_caught_and_reported() {
    let seeds = [Seed::generic(b"seed bytes")];
    run(
        env!("CARGO_MANIFEST_DIR"),
        "a_check_that_panics_is_caught_and_reported",
        &seeds,
        5,
        2,
        Duration::from_secs(2),
        |_seed_index, _bytes| panic!("boom"),
    );
}

#[test]
#[should_panic(expected = "exceeded the")]
fn a_check_slower_than_the_time_budget_is_reported() {
    let seeds = [Seed::generic(b"seed bytes")];
    run(
        env!("CARGO_MANIFEST_DIR"),
        "a_check_slower_than_the_time_budget_is_reported",
        &seeds,
        3,
        2,
        Duration::from_millis(1),
        |_seed_index, _bytes| {
            std::thread::sleep(Duration::from_millis(20));
            Outcome::Ok
        },
    );
}

#[test]
fn an_empty_seed_list_runs_nothing() {
    run(
        env!("CARGO_MANIFEST_DIR"),
        "an_empty_seed_list_runs_nothing",
        &[],
        1000,
        1000,
        Duration::from_secs(2),
        |_seed_index, _bytes| Outcome::Violation("should never be called".to_string()),
    );
}

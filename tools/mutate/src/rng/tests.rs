#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{SplitMix64, seed_for};

#[test]
fn the_same_seed_always_produces_the_same_sequence() {
    let mut a = SplitMix64::new(42);
    let mut b = SplitMix64::new(42);
    for _ in 0..100 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
}

#[test]
fn different_seeds_usually_diverge_immediately() {
    let mut a = SplitMix64::new(1);
    let mut b = SplitMix64::new(2);
    assert_ne!(a.next_u64(), b.next_u64());
}

#[test]
fn below_never_reaches_its_bound() {
    let mut r = SplitMix64::new(7);
    for _ in 0..1000 {
        assert!(r.below(5) < 5);
    }
    assert_eq!(r.below(0), 0);
}

#[test]
fn seed_for_is_a_pure_function_of_its_three_inputs() {
    assert_eq!(seed_for("test_a", 0, 0), seed_for("test_a", 0, 0));
    assert_ne!(seed_for("test_a", 0, 0), seed_for("test_b", 0, 0));
    assert_ne!(seed_for("test_a", 0, 0), seed_for("test_a", 1, 0));
    assert_ne!(seed_for("test_a", 0, 0), seed_for("test_a", 0, 1));
}

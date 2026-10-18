//! A small, deterministic mutation-testing harness (RFC 014 D-1/D-2/D-3): a splitmix64 generator and
//! mutation operators ([`ops`]) plus a quick/deep test harness ([`harness`]) that each parser crate uses
//! as a path-only dev-dependency. `publish = false`: this crate ships in no published artifact, adds no
//! dependency to any published crate, and adds no public feature or API that a brygge user can see
//! (RFC 014's non-negotiable constraints).
//!
//! A parser module uses this crate by:
//! 1. picking a handful of its own valid fixtures as seeds;
//! 2. for each, listing the byte offsets it considers worth probing with boundary values (a length, a
//!    count, a window size, …) as [`ops::InterestingPosition`]s;
//! 3. calling [`harness::run`] with a closure that feeds the mutated bytes through its own parser and
//!    reports [`harness::Outcome::Violation`] for anything other than a typed error or an in-ceiling `Ok`.
//!
//! See `rfcs/handoffs/014-hardening/mutation-tests-handoff-v1.md` for the full specification.

pub mod harness;
pub mod ops;
pub mod rng;

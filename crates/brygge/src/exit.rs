//! The CL-08 exit-code taxonomy: a CI gate distinguishes outcomes without parsing prose.
//!
//! `0` clean · `10` recorded loss (advisory/other drops) · `20` floor refusal (`FA-3`) ·
//! `30` convention violation (`FA-2`, reserved for SVN) · `40` partial/interrupted (`FA-1`, reserved) ·
//! `50` verify failed · `1` failure (bad args, unreadable input, I/O).

/// Completed cleanly; at most representation-class drops (the benign Git baseline).
pub const CLEAN: i32 = 0;
/// Generic failure: bad arguments, unreadable input, I/O.
pub const FAILURE: i32 = 1;
/// Completed, but a non-representation (advisory/other) drop was recorded — the honest, non-silent signal.
pub const RECORDED_LOSS: i32 = 10;
/// A source feature below the floor was refused (`FA-3`).
pub const FLOOR_REFUSAL: i32 = 20;
/// A source violated its own conventions and was not resolved (`FA-2`) — svn ref reconstruction found no
/// trunk/branches/tags layout (RFC 006 OQ-B); not reached by Git.
pub const CONVENTION_VIOLATION: i32 = 30;
/// A partial/interrupted import (`FA-1`) — reserved.
#[allow(
    dead_code,
    reason = "reserved CL-08 outcome class for partial/interrupted imports (FA-1)"
)]
pub const PARTIAL: i32 = 40;
/// A `verify` check did not hold.
pub const VERIFY_FAILED: i32 = 50;

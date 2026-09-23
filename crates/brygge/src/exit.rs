//! The CL-08 exit-code taxonomy: a CI gate distinguishes outcomes without parsing prose.
//!
//! `0` clean · `10` recorded loss (advisory/other drops) · `20` floor refusal (`FA-3`) or a resource
//! ceiling hit (CR-10/CR-16 handoff) · `30` convention/confidence violation (`FA-2`) · `40`
//! partial/interrupted (`FA-1`, reserved) · `50` verify failed · `1` runtime failure (unreadable input,
//! I/O, an internal decoder fault) · `2` usage error (bad arguments, an inapplicable option).

/// Completed cleanly; at most representation-class drops (the benign Git baseline).
pub const CLEAN: i32 = 0;
/// Runtime failure: unreadable input, I/O, or an internal decoder fault (the CR-16 panic boundary).
/// Never a usage problem — see [`USAGE`].
pub const FAILURE: i32 = 1;
/// Completed, but a non-representation (advisory/other) drop was recorded — the honest, non-silent signal.
pub const RECORDED_LOSS: i32 = 10;
/// A source feature below the floor was refused (`FA-3`), or a resource ceiling was hit.
pub const FLOOR_REFUSAL: i32 = 20;
/// A source violated its own conventions and was not resolved (`FA-2`), or fell below a confidence floor
/// — svn ref reconstruction found no trunk/branches/tags layout (RFC 006 OQ-B), or a CVS reconstruction
/// scored under the confidence floor (RFC 007 D-8); not reached by Git or Mercurial.
pub const CONVENTION_VIOLATION: i32 = 30;
/// A partial/interrupted import (`FA-1`) — reserved.
#[allow(
    dead_code,
    reason = "reserved CL-08 outcome class for partial/interrupted imports (FA-1)"
)]
pub const PARTIAL: i32 = 40;
/// A `verify` check did not hold.
pub const VERIFY_FAILED: i32 = 50;
/// A usage error: bad arguments, an unknown or missing flag, a missing value, a missing positional, or an
/// option given to a source kind it does not apply to. Never a runtime condition.
pub const USAGE: i32 = 2;

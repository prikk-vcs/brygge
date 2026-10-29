//! The IR contract version and its read gate (RFC 011 D-3, requirement `IX-07`).
//!
//! The contract version is **independent of the brygge tool version** and travels in every artifact's
//! container header (RFC 011 §2.3). RFC 011 re-cut the contract to **0.2.0**, superseding the 1.0.0
//! freeze recorded in RFC 003 D-7: the wire format changed too much (tagged records, a critical bit, a
//! strict canonical form) to keep calling it 1.0. While major stays `0`, RFC 011 D-3's rule applies: a
//! breaking change (a critical field or variant added or changed) bumps `0.y`; a purely additive,
//! non-critical-only change bumps `0.y.z`. A reader accepts only its own exact `0.y` (skipping unknown
//! non-critical fields within it) and refuses any other `0.y` with a stated message. Once major reaches
//! `1` (declared with brygge 1.0), the familiar rule resumes: major is breaking, minor is additive.

use crate::Error;

/// A semantic version of the IR contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContractVersion {
    /// Major — while `0`, any breaking change bumps this (RFC 011 D-3); from `1`, a breaking change.
    pub major: u32,
    /// Minor — while major is `0`, a reader accepts only its own exact minor; from major `1`, additive.
    pub minor: u32,
    /// Patch — non-critical-only additions while major is `0`; otherwise unused by the read gate.
    pub patch: u32,
}

impl ContractVersion {
    /// Construct a contract version.
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// True when a build supporting [`CURRENT`] may read an artifact declaring `self` (RFC 011 D-3,
    /// RFC 018 D-7): while major `0`, only a `0.y` listed in [`READABLE`] is accepted (any patch); from
    /// major `1`, any minor within this build's major is accepted (additive-only forward compatibility).
    #[must_use]
    pub fn accepts(self) -> bool {
        if CURRENT.major == 0 {
            self.major == 0 && READABLE.iter().any(|r| r.minor == self.minor)
        } else {
            self.major == CURRENT.major
        }
    }
}

impl std::fmt::Display for ContractVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The IR contract version this build **writes** (RFC 018 D-1, compact artifacts; re-cuts RFC 011's
/// 0.2.0). [`READABLE`] is what it **reads** — a strict superset while this build also keeps reading the
/// prior minor (RFC 018 D-7, OQ-2).
pub const CURRENT: ContractVersion = ContractVersion::new(0, 3, 0);

/// Every contract minor this build reads (any patch), oldest first. [`write_to`](crate::artifact::write_to)
/// only ever writes [`CURRENT`] — this list is the read gate's alone.
pub const READABLE: &[ContractVersion] = &[ContractVersion::new(0, 2, 0), CURRENT];

/// Check that an artifact's declared contract version is readable, else [`Error::UnsupportedContract`].
///
/// # Errors
/// Returns [`Error::UnsupportedContract`] when `found` is not [`ContractVersion::accepts`]-ed by this
/// build.
pub fn ensure_readable(found: ContractVersion) -> Result<(), Error> {
    if found.accepts() {
        Ok(())
    } else {
        Err(Error::UnsupportedContract {
            found,
            supported: CURRENT,
        })
    }
}

/// A one-line rendering of [`READABLE`] for an error message ("reads 0.2, 0.3").
pub(crate) fn readable_summary() -> String {
    READABLE
        .iter()
        .map(|v| format!("{}.{}", v.major, v.minor))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;

//! The IR contract version and its read gate (RFC 003 D-7, requirement `IX-07`).
//!
//! The contract version is **independent of the brygge tool version** and travels in every artifact's
//! manifest. A reader **refuses an unknown major** rather than misread it — the same discipline as
//! prikk's format gates. Pre-1.0 the contract may change with a minor bump; after the freeze (once Git
//! and Mercurial have exercised it — ROADMAP) it is additive-only within a major.

use crate::Error;

/// A semantic version of the IR contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContractVersion {
    /// Major — a change here is breaking; a reader refuses a major it does not know.
    pub major: u32,
    /// Minor — additive within a major after the freeze.
    pub minor: u32,
    /// Patch.
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

    /// True when a build supporting up to [`CURRENT`] may read an artifact declaring `self`
    /// (RFC 003 D-7): the major must be known. A newer minor/patch within a known major is readable
    /// (additive-only forward compatibility).
    #[must_use]
    #[allow(
        clippy::absurd_extreme_comparisons,
        reason = "CURRENT.major is 0 today, so this reads as `major <= 0`, a type-extreme \
                  comparison clippy flags. The `<=` is deliberate forward-compatibility: it keeps \
                  admitting known (lower-or-equal) majors once CURRENT advances, and must not be \
                  narrowed to `==`."
    )]
    pub const fn is_readable(self) -> bool {
        self.major <= CURRENT.major
    }
}

impl std::fmt::Display for ContractVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The IR contract version this build writes and reads. Pre-freeze (`0.x`): may change with a minor
/// bump and a stated migration (RFC 003 D-7).
pub const CURRENT: ContractVersion = ContractVersion::new(0, 1, 0);

/// Check that an artifact's declared contract version is readable, else [`Error::UnsupportedContractMajor`].
///
/// # Errors
/// Returns [`Error::UnsupportedContractMajor`] when `found.major` exceeds this build's.
pub fn ensure_readable(found: ContractVersion) -> Result<(), Error> {
    if found.is_readable() {
        Ok(())
    } else {
        Err(Error::UnsupportedContractMajor {
            found: found.major,
            supported: CURRENT.major,
        })
    }
}

#[cfg(test)]
mod tests;

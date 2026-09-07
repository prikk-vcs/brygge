//! The IR contract version and its read gate (RFC 003 D-7, requirement `IX-07`).
//!
//! The contract version is **independent of the brygge tool version** and travels in every artifact's
//! manifest. A reader **refuses an unknown major** rather than misread it — the same discipline as
//! prikk's format gates. **Frozen at 1.0 (RFC 003 D-7, 2026-09-08)** — Git (M1) and Mercurial (M2)
//! exercised the contract with no change, so it is now **additive-only within major 1**: new optional
//! fields and new versioned enum variants only, never a field removed or repurposed. A breaking change
//! would be a deliberate, rare contract 2.0 shipped with a converter.

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
    /// (additive-only forward compatibility), and a pre-freeze major-0 artifact stays readable under the
    /// frozen major 1.
    #[must_use]
    pub const fn is_readable(self) -> bool {
        self.major <= CURRENT.major
    }
}

impl std::fmt::Display for ContractVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The IR contract version this build writes and reads. **Frozen at 1.0.0** (RFC 003 D-7, 2026-09-08):
/// additive-only within major 1 from here; a breaking change is a deliberate contract 2.0 with a converter.
pub const CURRENT: ContractVersion = ContractVersion::new(1, 0, 0);

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

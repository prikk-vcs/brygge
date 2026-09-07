//! brygge's **Mercurial source decoder** (RFC 005): read a local Mercurial repository by parsing its
//! revlog store **directly** — no `hg` binary, no subprocess (RFC 005 D-1, Tier 2) — and produce a
//! [`brygge_ir::Ir`], mostly *Stated*, **including source-recorded renames** carried as `Stated`
//! (the point of M2, SRC-H2).
//!
//! This is the only crate that reads hg (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none
//! of it. It reads an untrusted store, so every parser is bounds-checked and panic-free.
//!
//! **Status: foundation increment.** This increment lands the crate, the error/option types, and the
//! **format-safety gate** ([`requires`]) — the "refuse rather than misread" spine (RFC 005 §1): a
//! repository whose `.hg/requires` names a format this build does not implement is refused, never
//! guessed. The revlog reader and `decode()` follow, built against real Mercurial fixtures.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod requires;

mod options;
pub use options::Options;

/// The low-level Mercurial revlog reader (index + delta chains + decompression). Public so the decode
/// layer and integration tests can drive it; most callers want `decode()` instead.
pub mod revlog;

/// The decoder id and version recorded into IR provenance (`PR-6`) and every derivation (`HO-1`).
#[must_use]
pub fn decoder_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Everything the Mercurial decoder can fail with. A refused feature or unreadable format is a **typed
/// outcome**, never a panic and never an approximation (`FA-3`, RFC 005 D-1/D-4).
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The path is not a readable Mercurial repository.
    Open(String),
    /// An object or store file could not be read or decoded (a malformed or unreadable repository).
    Read(String),
    /// The repository declares a format requirement this build does not implement (RFC 005 §1). Refused
    /// rather than misread — the format-level safety gate.
    UnsupportedFormat {
        /// The `.hg/requires` entry that is not implemented.
        requirement: String,
        /// Why it is refused rather than read.
        reason: String,
    },
    /// A source feature below the floor was hit; refused with a named reason (RFC 005 D-4, OQ-3, `FA-3`).
    FloorRefusal {
        /// The refused feature (e.g. `"largefiles"`, `"subrepo"`).
        feature: String,
        /// Why it is refused rather than approximated.
        reason: String,
    },
    /// The assembled IR violated a `brygge-ir` invariant (a decoder bug, not bad input).
    Ir(brygge_ir::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(m) => write!(f, "cannot open Mercurial repository: {m}"),
            Self::Read(m) => write!(f, "cannot read Mercurial repository: {m}"),
            Self::UnsupportedFormat {
                requirement,
                reason,
            } => write!(
                f,
                "unsupported Mercurial format requirement '{requirement}': {reason}"
            ),
            Self::FloorRefusal { feature, reason } => {
                write!(
                    f,
                    "refused Mercurial feature '{feature}' below the floor: {reason}"
                )
            }
            Self::Ir(e) => write!(f, "IR assembly failed: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ir(e) => Some(e),
            _ => None,
        }
    }
}

impl From<brygge_ir::Error> for Error {
    fn from(e: brygge_ir::Error) -> Self {
        Self::Ir(e)
    }
}

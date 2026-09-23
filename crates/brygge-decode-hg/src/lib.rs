//! brygge's **Mercurial source decoder** (RFC 005, milestone M2): read a local Mercurial repository by
//! parsing its revlog store **directly** — no `hg` binary, no subprocess (RFC 005 D-1, Tier 2) — and
//! produce a [`brygge_ir::Ir`], mostly *Stated*, **including source-recorded renames** carried as
//! `Stated` (SRC-H2).
//!
//! This is the only crate that reads hg (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none
//! of it. It reads an untrusted store, so every parser is bounds-checked and panic-free. The
//! **format-safety gate** (the crate-private `requires` module) — the "refuse rather than misread" spine
//! (RFC 005 §1) — refuses a repository whose `.hg/requires` names a format this build does not
//! implement, before a single revlog byte is parsed.
//!
//! **Public API (CR-20, minimal by design):** [`decode`], [`Options`], [`Error`], and
//! [`decoder_version`] — the revlog reader and the format gate are crate-private; `decode()` and this
//! crate's own tests are their only callers.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod changelog;
mod decode;
mod filelog;
mod floor;
mod fncache;
mod manifest;
/// The `.hg/store/obsstore` reader (RFC 005 corrections handoff §1.2); crate-internal, reached only
/// from `published`.
mod obsstore;
mod options;
/// Computes the published view (RFC 005 corrections handoff §1, D-4): phases, obsolescence, and the
/// pinned set; crate-internal, reached only from `decode()`.
mod phases;
/// Computes which changesets `hg clone` would transfer (RFC 005 corrections handoff §1, D-4);
/// crate-internal, reached only from `decode()`.
mod published;
/// The format-safety gate (RFC 005 §1): crate-internal (CR-20), reached only from `decode()`.
mod requires;
/// The low-level Mercurial revlog reader (index + delta chains + decompression); crate-internal (CR-20)
/// — `decode()` and this crate's own tests drive it, no caller outside the crate needs it.
mod revlog;
mod util;
pub use decode::decode;
pub use options::Options;

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
    /// A resource ceiling was hit (a malformed or hostile revlog); refused rather than exhausting the
    /// host (RFC 010, `brygge-03` T-8/C-2d).
    ResourceLimit {
        /// A noun phrase for what exceeded its ceiling (e.g. `"a decompressed revision"`).
        what: String,
        /// The ceiling's value, with its unit (e.g. `"1073741824 bytes"`).
        ceiling: String,
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
            Self::ResourceLimit { what, ceiling } => {
                write!(f, "refused: {what} exceeds brygge's ceiling ({ceiling})")
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

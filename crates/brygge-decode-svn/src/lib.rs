//! brygge's **Subversion source decoder** (RFC 006, milestone M3): read an SVN history from a
//! **dumpstream** — a user-supplied dumpfile, or a read-only local `svnadmin dump` brygge invokes
//! (RFC 006 D-1, Tier D) — and produce a [`brygge_ir::Ir`]: a `Stated` linear revision spine with
//! `Stated` copies, plus an **opt-in** `Derived` branch/tag layer reconstructed by layout convention.
//!
//! This is the only crate that reads SVN (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none
//! of it. There is **no linked SVN library**: the dumpstream is uncompressed plaintext parsed in pure
//! Rust, and the one external touch is running `svnadmin dump` as a producer subprocess (skipped entirely
//! when a dumpfile is supplied). It reads untrusted input, so every parser is bounds-checked and
//! panic-free (RFC 006 security review, `brygge-03` T-2/INV-2).
//!
//! Entry point: [`decode`]. Sources are described by [`Source`]. Behaviour is tuned by [`Options`]
//! (branch/tag reconstruction is **off by default**).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod decode;
mod dumpstream;
mod layout;
mod options;
mod props;
mod source;
mod tree;

pub use decode::decode;
pub use options::{LayoutPolicy, Options};
pub use source::Source;

/// The parsed-dumpstream types, public so integration tests and the decode layer can drive the reader;
/// most callers want [`decode`] instead.
pub mod dump {
    pub use crate::dumpstream::{
        Dump, NodeAction, NodeKind, NodeRecord, RevisionRecord, parse_dump,
    };
}

/// The decoder id and version recorded into IR provenance (`PR-6`) and every derivation (`HO-1`).
#[must_use]
pub fn decoder_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The decoder id recorded into IR provenance and derivations.
pub const DECODER: &str = "brygge-decode-svn";

/// Everything the Subversion decoder can fail with. A refused feature, an unreadable format, or a
/// resource ceiling is a **typed outcome**, never a panic and never an approximation (`FA-3`, RFC 006).
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The source is not a readable SVN dumpstream or repository (bad path, `svnadmin` failure, empty).
    Open(String),
    /// The dumpstream could not be read or decoded (malformed or truncated).
    Read(String),
    /// The dumpstream declares a format version, or uses a form (e.g. deltas), this build does not
    /// implement (RFC 006 §4). Refused rather than misread — the format-level safety gate.
    UnsupportedFormat {
        /// What is not implemented (e.g. `"dump format version 5"`, `"delta dump"`).
        what: String,
        /// Why it is refused rather than read.
        reason: String,
    },
    /// A source feature below the floor was hit; refused with a named reason (RFC 006 D-4/OQ-B, `FA-3`).
    FloorRefusal {
        /// The refused feature (e.g. `"svn:externals"`, `"remote-source"`).
        feature: String,
        /// Why it is refused rather than approximated.
        reason: String,
    },
    /// A resource ceiling was hit (a malformed or hostile dump); refused rather than exhausting the host
    /// (RFC 006 security review, `brygge-03` T-8/C-2d).
    ResourceLimit {
        /// Which limit was exceeded.
        limit: String,
    },
    /// The assembled IR violated a `brygge-ir` invariant (a decoder bug, not bad input).
    Ir(brygge_ir::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(m) => write!(f, "cannot open SVN source: {m}"),
            Self::Read(m) => write!(f, "cannot read SVN dumpstream: {m}"),
            Self::UnsupportedFormat { what, reason } => {
                write!(f, "unsupported SVN dump form '{what}': {reason}")
            }
            Self::FloorRefusal { feature, reason } => {
                write!(
                    f,
                    "refused SVN feature '{feature}' below the floor: {reason}"
                )
            }
            Self::ResourceLimit { limit } => {
                write!(
                    f,
                    "SVN dump exceeded a resource limit ({limit}); refused rather than exhaust the host"
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

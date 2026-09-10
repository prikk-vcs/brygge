//! brygge's **CVS source decoder** (RFC 007, milestone M4 — the gradient's last source): read a CVS
//! repository's RCS `,v` files directly (RFC 007 D-1, Tier R) and **reconstruct changesets** by clustering
//! per-file revisions on (author, log, time window), producing a [`brygge_ir::Ir`].
//!
//! CVS has **no atomic commit**, so the changeset is brygge's construction: **every [`ChangeAtom`] is
//! `Derived(ReconstructedChangeset)`** (SRC-C1/IR-2), carrying its clustering parameters and a confidence —
//! the honest lossy-but-labelled verdict (SRC-C3). Per-file content and history are carried faithfully.
//!
//! This is the only crate that reads CVS (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none of
//! it. There is **no linked CVS/RCS library and no subprocess**: RCS `,v` files are uncompressed text
//! parsed in pure Rust. It reads untrusted input, so every parser is bounds-checked and panic-free
//! (RFC 007 security review, `brygge-03` T-2/INV-2).
//!
//! Entry point: [`decode`]. The source is a local CVS repository ([`Source`]); behaviour is tuned by
//! [`Options`] (the clustering window and confidence floor; ref reconstruction is **off by default**).
//!
//! [`ChangeAtom`]: brygge_ir::ChangeAtom

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cluster;
mod decode;
mod options;
mod rcs;
mod scan;
mod source;
mod symbols;

pub use decode::decode;
pub use options::Options;
pub use source::Source;

/// The RCS `,v` reader types, public so integration tests and the decode layer can drive the reader;
/// most callers want [`decode`] instead.
pub mod rcsfile {
    pub use crate::rcs::{RcsFile, RevNum, Revision, parse_rcs};
}

/// The decoder id and version recorded into IR provenance (`PR-6`) and every derivation (`HO-1`).
#[must_use]
pub fn decoder_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The decoder id recorded into IR provenance and derivations.
pub const DECODER: &str = "brygge-decode-cvs";

/// The `what` of the loss record written when the whole import is under the confidence floor (RFC 007
/// OQ-B). A CLI maps its presence to the CL-08 convention/confidence exit class.
pub const UNDER_FLOOR: &str = "reconstruction confidence below the floor";

/// Everything the CVS decoder can fail with. A refused feature, an unreadable `,v`, or a below-floor
/// reconstruction is a **typed outcome**, never a panic and never an approximation (`FA-3`, RFC 007).
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The source is not a readable local CVS repository (bad path, remote source, empty).
    Open(String),
    /// An RCS `,v` file could not be read or decoded (malformed or truncated).
    Read(String),
    /// A source feature below the floor was hit; refused with a named reason (RFC 007 D-8/OQ-B, `FA-3`).
    FloorRefusal {
        /// The refused feature (e.g. `"remote-source"`, `"whole-import-under-confidence-floor"`).
        feature: String,
        /// Why it is refused rather than approximated.
        reason: String,
    },
    /// A resource ceiling was hit (a malformed or hostile `,v`); refused rather than exhausting the host
    /// (RFC 007 security review, `brygge-03` T-8/C-2d).
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
            Self::Open(m) => write!(f, "cannot open CVS repository: {m}"),
            Self::Read(m) => write!(f, "cannot read CVS repository: {m}"),
            Self::FloorRefusal { feature, reason } => {
                write!(
                    f,
                    "refused CVS feature '{feature}' below the floor: {reason}"
                )
            }
            Self::ResourceLimit { limit } => write!(
                f,
                "CVS repository exceeded a resource limit ({limit}); refused rather than exhaust the host"
            ),
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

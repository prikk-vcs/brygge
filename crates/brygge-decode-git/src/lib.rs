//! brygge's **Git source decoder** (RFC 004): read a local Git object database and produce a
//! [`brygge_ir::Ir`] — content, ancestry, and messages carried faithfully **as claims**, entirely
//! *Stated* except opt-in, explicitly-marked *Derived* rename hints (which never replace the literal
//! delete+create Git recorded).
//!
//! This is the **only** crate that links `gix` (RFC 009 D-1): [`brygge_ir`], the encoders, and the
//! internal-verify path link none of it, so a target checks a brygge import on its own surface. gix is
//! used with **no network feature** (INV-3) and the decoder executes **no source-provided code** — no
//! hooks, filters/smudge, or submodule fetch (RFC 009 D-4). Output is byte-deterministic for the same
//! repository + brygge version + options (`VF-1`), independent of physical packing.
//!
//! ```no_run
//! let ir = brygge_decode_git::decode(
//!     std::path::Path::new("/path/to/repo/.git"),
//!     &brygge_decode_git::Options::default(),
//! )?;
//! println!("{}", brygge_ir::honesty::summary(&ir).render_human());
//! # Ok::<(), brygge_decode_git::Error>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod decode;
mod open;
mod options;

pub use decode::decode;
pub use options::Options;

/// The decoder id and version recorded into IR provenance (`PR-6`) and every derivation (`HO-1`).
#[must_use]
pub fn decoder_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Everything the Git decoder can fail with. A refused source feature is a **typed outcome**
/// ([`Error::FloorRefusal`]), not a panic and not an approximation (`FA-3`).
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The path is not a readable Git repository.
    Open(String),
    /// An object could not be read or decoded (a malformed or unreadable repository).
    Read(String),
    /// A source feature below the floor was hit; it is refused with a named reason (RFC 004 D-4, `FA-3`).
    FloorRefusal {
        /// The refused feature (e.g. `"submodule"`, `"shallow clone"`).
        feature: String,
        /// Why it is refused rather than approximated.
        reason: String,
    },
    /// The assembled IR violated a `brygge-ir` invariant (signals a decoder bug, not bad input).
    Ir(brygge_ir::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(m) => write!(f, "cannot open Git repository: {m}"),
            Self::Read(m) => write!(f, "cannot read Git repository: {m}"),
            Self::FloorRefusal { feature, reason } => {
                write!(
                    f,
                    "refused Git feature '{feature}' below the floor: {reason}"
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

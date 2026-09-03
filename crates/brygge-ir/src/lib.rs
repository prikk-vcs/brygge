//! brygge's intermediate representation — the light core (RFC 001/002/003, under RFC 009).
//!
//! This crate is the durable product boundary of brygge (requirement PU-3): decoders write an [`Ir`],
//! `inspect`/`verify` read it, encoders consume it, and a foreign encoder for another target depends on
//! this crate and nothing heavier. It **links no source-decoder dependency** (RFC 009 D-1): the whole
//! honesty and verification path runs here, so a target can check a brygge import on its own surface.
//!
//! The one idea it turns on (RFC 001): **faithfulness-with-provenance, not neutrality.** The IR records
//! what each source *literally guaranteed*, plus a *marked* place for what a decoder or encoder
//! *derived* — and it carries **evidence for** identity (stated and inferred renames), never identity
//! itself, leaving a node-identity encoder to author identity visibly. Honesty is a property of the
//! model, not a convention: an inference has no representation except a [`status::Derived`] record, and
//! there is no field in which to store target identity.
//!
//! Modules: [`version`] the IR contract version + read gate · [`canon`] the hand-rolled canonical codec ·
//! [`content`] the content-addressed blob store · [`status`] the epistemic-status type + taxonomy ·
//! [`model`] the IR types · [`builder`] deterministic construction · [`artifact`] the single-file,
//! digested, versioned container · [`honesty`] the loss boundary + the recoverable fidelity report.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod artifact;
pub mod builder;
pub mod canon;
pub mod content;
pub mod honesty;
pub mod model;
pub mod status;
pub mod version;

pub use artifact::{from_bytes, to_bytes};
pub use builder::{AtomDraft, IrBuilder};
pub use content::{BlobId, ContentStore};
pub use honesty::{FidelityReport, summary};
pub use model::{
    AtomId, ChangeAtom, DropRecord, Identity, ImportProvenance, Ir, LossBoundary, LossClass,
    MetadataClaims, PathOp, RefKind, RefRecord, RenameHint, SourceIdentity, SourceKind,
};
pub use status::{Derivation, DerivationKind, EpistemicStatus};
pub use version::ContractVersion;

/// The result type for fallible `brygge-ir` operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything `brygge-ir` can fail with. Reading an artifact never panics on malformed bytes — a bad
/// input is a typed error (brygge parses untrusted history; the core must be unshakeable).
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The artifact's bytes ended or were malformed while decoding.
    Decode(String),
    /// The artifact's integrity digest did not match its contents (tamper or truncation — RFC 003 D-3).
    DigestMismatch,
    /// The artifact declares an IR-contract major version this build does not know how to read
    /// (RFC 003 D-7): stikk-style refuse-rather-than-misread.
    UnsupportedContractMajor {
        /// The contract major found in the artifact.
        found: u32,
        /// The highest contract major this build supports.
        supported: u32,
    },
    /// A structural invariant was violated while building or writing (e.g. a ref targets an unknown
    /// atom). Signals a bug in a caller, never malformed *input*.
    Invariant(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(m) => write!(f, "malformed brygge-ir artifact: {m}"),
            Self::DigestMismatch => {
                write!(
                    f,
                    "brygge-ir integrity digest mismatch (tampered or truncated)"
                )
            }
            Self::UnsupportedContractMajor { found, supported } => write!(
                f,
                "brygge-ir contract major {found} is newer than this build supports ({supported}); \
                 refusing to misread it"
            ),
            Self::Invariant(m) => write!(f, "brygge-ir invariant violated: {m}"),
        }
    }
}

impl std::error::Error for Error {}

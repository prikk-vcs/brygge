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
pub(crate) mod canon;
pub mod content;
pub mod honesty;
pub mod model;
pub mod status;
pub mod version;

pub use artifact::{Decoded, from_bytes, to_bytes};
pub use builder::{AtomDraft, IrBuilder};
pub use content::{BlobId, ContentStore};
pub use honesty::{FidelityReport, summary};
pub use model::{
    Annotation, AtomId, ChangeAtom, CopyRecord, DropRecord, Extra, Flag, FlagKind, Identity,
    ImportProvenance, Ir, LossBoundary, LossClass, MetadataClaims, PathOp, RefKind, RefRecord,
    Signature, SourceIdentity, SourceKind, Text, Time,
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
    /// The artifact's bytes ended or were malformed while decoding (a truncation, an invalid UTF-8
    /// byte in a `text` field, a declared length exceeding the input).
    Decode(String),
    /// The artifact's integrity digest did not match its contents (tamper or truncation — RFC 003 D-3).
    DigestMismatch,
    /// The artifact's container format byte is `1` (`PreReleaseArtifact`): a pre-contract-0.2.0
    /// artifact, from before RFC 011. Its metadata is never parsed — the wire shape underneath changed
    /// too much to read safely. Re-decode the source.
    PreReleaseArtifact,
    /// The artifact declares an IR-contract version this build does not read (RFC 011 D-3's "0.y"
    /// rule): while major 0, a reader accepts only its own exact `0.y`; from major 1, it accepts its
    /// own major with any minor.
    UnsupportedContract {
        /// The contract version found in the artifact.
        found: version::ContractVersion,
        /// The contract version this build supports.
        supported: version::ContractVersion,
    },
    /// A record carried a field with an unrecognized id and its critical bit set (RFC 011 §2.2 rule
    /// 9): this build cannot safely ignore it, since the field may change the record's meaning.
    UnknownCriticalField {
        /// The record type the field was found in (e.g. `"PathOp::Add"`).
        record: &'static str,
        /// The unrecognized field's id (not the raw tag — the critical bit already stripped).
        tag: u64,
    },
    /// The bytes decode but violate a canonical-form rule (RFC 011 §2.2): an out-of-order or
    /// duplicate tag, a non-minimal varint, an omittable field encoded anyway, an unsorted list or
    /// map, or trailing bytes. Accepting non-canonical input would let two different byte strings mean
    /// the same value, defeating content-addressing and the integrity digest.
    NonCanonical(String),
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
            Self::PreReleaseArtifact => write!(
                f,
                "this artifact predates brygge-ir contract 0.2.0 (a pre-release format); re-decode the \
                 source"
            ),
            Self::UnsupportedContract { found, supported } => write!(
                f,
                "artifact contract {found} is not supported by this build ({supported})"
            ),
            Self::UnknownCriticalField { record, tag } => write!(
                f,
                "{record}: unrecognized critical field {tag}; this build cannot safely ignore it"
            ),
            Self::NonCanonical(m) => write!(f, "brygge-ir artifact is not in canonical form: {m}"),
            Self::Invariant(m) => write!(f, "brygge-ir invariant violated: {m}"),
        }
    }
}

impl std::error::Error for Error {}

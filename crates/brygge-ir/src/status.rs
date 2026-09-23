//! The epistemic-status type and the shared derivation taxonomy (RFC 001 D-2/D-3, RFC 002 D-1, RFC 011
//! §2.4).
//!
//! Every assertion in the IR is either [`EpistemicStatus::Stated`] (the source recorded it) or
//! [`EpistemicStatus::Derived`] (a decoder/encoder inferred it, carrying *why* and *how*). A reader
//! tells judgment from fact without re-running any heuristic (`HO-1`, prikk RFC 113 §4.2). The
//! derivation `kind` is drawn from a **closed, shared taxonomy** so a Git import and a CVS import are
//! comparable (`IR-5/IX-06`).

use std::collections::BTreeMap;

use crate::Error;
use crate::canon::{CanonReader, CanonWriter, RecordWriter};

/// Whether an assertion was stated by the source or derived by brygge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpistemicStatus {
    /// The source recorded this.
    Stated,
    /// A decoder or encoder inferred this; the record says how.
    Derived(Derivation),
}

/// The record of an inference: what kind, by whom, under which parameters, with what confidence
/// (`PR-5`, `HO-1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivation {
    /// The kind of derivation (shared taxonomy).
    pub kind: DerivationKind,
    /// The decoder/encoder that made it (e.g. `"brygge-decode-git"`).
    pub by: String,
    /// Its version — a different version may derive differently, so it is recorded.
    pub decoder_version: String,
    /// The exact parameters that governed the inference (canonical, sorted by key). See
    /// [`DerivationKind`] for which keys each kind requires.
    pub params: BTreeMap<String, String>,
    /// An optional confidence in `0..=100` (percent); `None` when the decoder has none to offer.
    pub confidence: Option<u8>,
}

/// The closed, versioned taxonomy of derivations (RFC 002 D-1). New kinds are added additively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivationKind {
    /// A rename inferred from similarity (Git). Required params: `rename_algorithm`,
    /// `rename_threshold`.
    InferredRename,
    /// A changeset reconstructed from per-file revisions (CVS). Required params: `window_secs`,
    /// `cluster_keys`, `date_rule`.
    ReconstructedChangeset,
    /// A branch reconstructed by convention (SVN path copies). Required params: `layout` or `source`.
    ReconstructedBranch,
    /// A merge relationship inferred rather than stated.
    InferredMerge,
    /// Metadata normalized/canonicalized by the decoder.
    NormalizedMetadata,
    /// A kind this build does not model by name; the note preserves the decoder's own label.
    Other(String),
}

impl EpistemicStatus {
    /// True for a derived assertion.
    #[must_use]
    pub fn is_derived(&self) -> bool {
        matches!(self, Self::Derived(_))
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        match self {
            Self::Stated => {
                w.uvarint(0);
                RecordWriter::new().finish_into(&mut w);
            }
            Self::Derived(d) => {
                w.uvarint(1);
                w.raw(&d.encode());
            }
        }
        w.into_bytes()
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        match r.uvarint()? {
            0 => {
                r.record_fields("EpistemicStatus::Stated", |_, _, _| Ok(false))?;
                Ok(Self::Stated)
            }
            1 => Ok(Self::Derived(Derivation::decode(r)?)),
            other => Err(Error::Decode(format!(
                "unknown EpistemicStatus variant {other}"
            ))),
        }
    }
}

impl Derivation {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.kind.encode());
        rw.field(2, true, self.by.as_bytes().to_vec());
        rw.field(3, true, self.decoder_version.as_bytes().to_vec());
        if !self.params.is_empty() {
            rw.field(
                4,
                true,
                crate::canon::map_value(self.params.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
            );
        }
        if let Some(c) = self.confidence {
            rw.field(5, true, crate::canon::uvarint_value(u64::from(c)));
        }
        rw.into_bytes()
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut kind = None;
        let mut by = None;
        let mut decoder_version = None;
        let mut params = BTreeMap::new();
        let mut confidence = None;
        r.record_fields("Derivation", |r, id, len| match id {
            1 => {
                kind = Some(DerivationKind::decode(r)?);
                Ok(true)
            }
            2 => {
                by = Some(r.text(len)?);
                Ok(true)
            }
            3 => {
                decoder_version = Some(r.text(len)?);
                Ok(true)
            }
            4 => {
                params = r.map()?;
                if params.is_empty() {
                    return Err(Error::NonCanonical(
                        "Derivation.params encoded but empty".to_string(),
                    ));
                }
                Ok(true)
            }
            5 => {
                let c = r.uvarint()?;
                let c = u8::try_from(c)
                    .map_err(|_| Error::NonCanonical("confidence exceeds 100".to_string()))?;
                if c > 100 {
                    return Err(Error::NonCanonical("confidence exceeds 100".to_string()));
                }
                confidence = Some(c);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            kind: kind.ok_or_else(|| {
                Error::NonCanonical("Derivation missing field 1 (kind)".to_string())
            })?,
            by: by.ok_or_else(|| {
                Error::NonCanonical("Derivation missing field 2 (by)".to_string())
            })?,
            decoder_version: decoder_version.ok_or_else(|| {
                Error::NonCanonical("Derivation missing field 3 (decoder_version)".to_string())
            })?,
            params,
            confidence,
        })
    }
}

impl DerivationKind {
    /// A stable, human/report label.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::InferredRename => "inferred-rename",
            Self::ReconstructedChangeset => "reconstructed-changeset",
            Self::ReconstructedBranch => "reconstructed-branch",
            Self::InferredMerge => "inferred-merge",
            Self::NormalizedMetadata => "normalized-metadata",
            Self::Other(_) => "other",
        }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        let variant = match self {
            Self::InferredRename => 0,
            Self::ReconstructedChangeset => 1,
            Self::ReconstructedBranch => 2,
            Self::InferredMerge => 3,
            Self::NormalizedMetadata => 4,
            Self::Other(_) => 5,
        };
        w.uvarint(variant);
        let mut rw = RecordWriter::new();
        if let Self::Other(note) = self {
            rw.field(1, true, note.as_bytes().to_vec());
        }
        rw.finish_into(&mut w);
        w.into_bytes()
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let variant = r.uvarint()?;
        match variant {
            0..=4 => {
                r.record_fields("DerivationKind", |_, _, _| Ok(false))?;
                Ok(match variant {
                    0 => Self::InferredRename,
                    1 => Self::ReconstructedChangeset,
                    2 => Self::ReconstructedBranch,
                    3 => Self::InferredMerge,
                    _ => Self::NormalizedMetadata,
                })
            }
            5 => {
                let mut note = None;
                r.record_fields("DerivationKind::Other", |r, id, len| {
                    if id == 1 {
                        note = Some(r.text(len)?);
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })?;
                Ok(Self::Other(note.ok_or_else(|| {
                    Error::NonCanonical("DerivationKind::Other missing field 1 (note)".to_string())
                })?))
            }
            other => Err(Error::Decode(format!(
                "unknown DerivationKind variant {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests;

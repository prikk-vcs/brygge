//! The epistemic-status type and the shared derivation taxonomy (RFC 001 D-2/D-3, RFC 002 D-1).
//!
//! Every assertion in the IR is either [`EpistemicStatus::Stated`] (the source recorded it) or
//! [`EpistemicStatus::Derived`] (a decoder/encoder inferred it, carrying *why* and *how*). A reader
//! tells judgment from fact without re-running any heuristic (`HO-1`, prikk RFC 113 §4.2). The
//! derivation `kind` is drawn from a **closed, shared taxonomy** so a Git import and a CVS import are
//! comparable (`IR-5/IX-06`).

use std::collections::BTreeMap;

use crate::Error;
use crate::canon::{CanonReader, CanonWriter};

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
    /// The exact parameters that governed the inference (canonical, sorted by key).
    pub params: BTreeMap<String, String>,
    /// An optional confidence in `0..=100` (percent).
    pub confidence: Option<u8>,
}

/// The closed, versioned taxonomy of derivations (RFC 002 D-1). New kinds are added additively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivationKind {
    /// A rename inferred from similarity (Git). Params must include the algorithm and threshold.
    InferredRename,
    /// A changeset reconstructed from per-file revisions (CVS). Params must include the clustering keys.
    ReconstructedChangeset,
    /// A branch reconstructed by convention (SVN path copies). Params must include the convention.
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

    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        match self {
            Self::Stated => w.u8(0),
            Self::Derived(d) => {
                w.u8(1);
                d.encode(w);
            }
        }
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        match r.u8()? {
            0 => Ok(Self::Stated),
            1 => Ok(Self::Derived(Derivation::decode(r)?)),
            other => Err(Error::Decode(format!("bad epistemic-status tag {other}"))),
        }
    }
}

impl Derivation {
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        self.kind.encode(w);
        w.str(&self.by);
        w.str(&self.decoder_version);
        w.uvarint(self.params.len() as u64);
        for (k, v) in &self.params {
            // BTreeMap iterates in sorted key order — canonical.
            w.str(k);
            w.str(v);
        }
        match self.confidence {
            None => w.u8(0),
            Some(c) => {
                w.u8(1);
                w.u8(c);
            }
        }
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let kind = DerivationKind::decode(r)?;
        let by = r.str()?;
        let decoder_version = r.str()?;
        let n = r.uvarint()?;
        let mut params = BTreeMap::new();
        for _ in 0..n {
            let k = r.str()?;
            let v = r.str()?;
            params.insert(k, v);
        }
        let confidence = match r.u8()? {
            0 => None,
            1 => Some(r.u8()?),
            other => return Err(Error::Decode(format!("bad confidence tag {other}"))),
        };
        Ok(Self {
            kind,
            by,
            decoder_version,
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

    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        match self {
            Self::InferredRename => w.u8(0),
            Self::ReconstructedChangeset => w.u8(1),
            Self::ReconstructedBranch => w.u8(2),
            Self::InferredMerge => w.u8(3),
            Self::NormalizedMetadata => w.u8(4),
            Self::Other(note) => {
                w.u8(255);
                w.str(note);
            }
        }
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        Ok(match r.u8()? {
            0 => Self::InferredRename,
            1 => Self::ReconstructedChangeset,
            2 => Self::ReconstructedBranch,
            3 => Self::InferredMerge,
            4 => Self::NormalizedMetadata,
            255 => Self::Other(r.str()?),
            other => return Err(Error::Decode(format!("unknown derivation kind {other}"))),
        })
    }
}

#[cfg(test)]
mod tests;

//! The IR types (RFC 001 D-1/D-3/D-4, re-cut by RFC 011 §2.4).
//!
//! A content store (in [`crate::content`]) plus a DAG of [`ChangeAtom`]s plus per-import
//! [`ImportProvenance`] and [`LossBoundary`]. Each atom holds the source's **literal** path operations
//! and — separately — marked [`CopyRecord`]s, so a rename never hides the delete+create the source
//! actually recorded (D-3). There is **no node-identity type**: the IR carries evidence for identity,
//! never identity (D-4). Every type has one canonical, tagged-record encoding (RFC 011 §2.4), used for
//! the artifact and for [`AtomId`] computation.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::Error;
use crate::canon::{CanonReader, CanonWriter, RecordWriter, list_value, map_value};
use crate::content::{ContentStore, to_hex};
use crate::status::EpistemicStatus;

pub use crate::content::BlobId;

/// Text carried as the bytes the source gave, with an optional declared encoding (RFC 011 D-2): a
/// commit message or identity name/email is **not** assumed to be UTF-8 just because most sources are.
/// `encoding` is `None` unless the source declared one; the bytes are carried unchanged either way.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Text {
    /// The raw bytes, exactly as the source gave them (may be empty).
    pub bytes: Vec<u8>,
    /// The encoding the source declared, if any (e.g. Git's `encoding` commit header). `None` does not
    /// mean UTF-8 — it means the source did not say.
    pub encoding: Option<String>,
}

impl Text {
    /// A `Text` from a Rust string, with no declared encoding.
    #[must_use]
    pub fn utf8(s: &str) -> Self {
        Self {
            bytes: s.as_bytes().to_vec(),
            encoding: None,
        }
    }

    /// The bytes decoded as UTF-8, if they are valid UTF-8 (regardless of `encoding`).
    #[must_use]
    pub fn as_utf8(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

/// A point in time as the source stated it: seconds since the Unix epoch, plus the source's own UTC
/// offset if it gave one (RFC 011 D-2). The offset is carried, never normalized away — two sources'
/// times are comparable only in `seconds`, not "local time".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Time {
    /// Seconds since the Unix epoch, as the source stated (may be negative).
    pub seconds: i64,
    /// The source's own UTC offset in minutes, if it recorded one; `None` when the source gave no
    /// offset (e.g. a bare epoch timestamp).
    pub offset_minutes: Option<i16>,
}

/// Which source system an identity came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// Git.
    Git,
    /// Mercurial.
    Hg,
    /// Subversion.
    Svn,
    /// CVS.
    Cvs,
    /// A source this build does not name; the label is preserved.
    Other(String),
}

/// One opaque cryptographic signature the source carried over its own object (e.g. Git's `gpgsig`),
/// labelled so a reader knows what it signs without brygge interpreting it (RFC 011 D-6). Never
/// verified by brygge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// What the signature is (e.g. `"gpgsig"`, `"gpgsig-sha256"`).
    pub label: String,
    /// The signature's raw bytes, exactly as the source stored them.
    pub bytes: Vec<u8>,
}

/// One opaque, source-specific datum brygge does not model by name, carried rather than dropped (RFC
/// 011 D-6): a labelled bag for source data with no home elsewhere in the IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra {
    /// What the datum is, in the source's own terms (e.g. `"mergetag"`).
    pub label: String,
    /// Its raw bytes, exactly as the source stored them.
    pub bytes: Vec<u8>,
}

/// The source's own opaque identifiers, signatures and unclassified extras (`PR-4/IR-3`) — the only
/// cryptographic link back to the original. Carried unchanged; they verify nothing in any target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdentity {
    /// The source system.
    pub kind: SourceKind,
    /// Opaque repository identity (e.g. a root commit id, a UUID).
    pub repo_id: Vec<u8>,
    /// Opaque atom identity (e.g. a Git commit SHA, an SVN revision, a CVS revision tag).
    pub atom_id: Vec<u8>,
    /// Opaque signatures, in source order.
    pub signatures: Vec<Signature>,
    /// Opaque, unclassified extras, in source order.
    pub extras: Vec<Extra>,
}

/// One path operation within an atom, each carrying its own epistemic status (D-2/D-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathOp {
    /// A path created with the given content and mode.
    Add {
        /// Repo-relative path.
        path: String,
        /// Content address.
        blob: BlobId,
        /// Unix-style file mode.
        mode: u32,
        /// Stated or derived.
        status: EpistemicStatus,
    },
    /// A path's content (and/or mode) changed.
    Modify {
        /// Repo-relative path.
        path: String,
        /// New content address.
        blob: BlobId,
        /// File mode.
        mode: u32,
        /// Stated or derived.
        status: EpistemicStatus,
    },
    /// A path removed.
    Delete {
        /// Repo-relative path.
        path: String,
        /// Stated or derived.
        status: EpistemicStatus,
    },
    /// A path replaced in place — a source that draws a distinction between "modify" and "replace"
    /// (e.g. a symlink swapped for a regular file at the same path) states it here rather than being
    /// folded into `Modify` (RFC 011 D-7).
    Replace {
        /// Repo-relative path.
        path: String,
        /// New content address.
        blob: BlobId,
        /// New file mode.
        mode: u32,
        /// Stated or derived.
        status: EpistemicStatus,
    },
}

/// A marked record that one path is a copy of another at a specific earlier atom (D-3, RFC 011 D-5).
/// `Stated` when the source recorded it (e.g. `hg mv`, an SVN copy); `Derived` when brygge inferred it
/// (e.g. Git similarity). Sits *beside* the literal delete+create/add ops, never replacing them.
///
/// A **move** is exactly: `from_atom` is the atom's first parent **and** the atom's own ops delete
/// `from` — see [`ChangeAtom::is_move`]. Anything else (a copy from a non-parent atom, or a copy whose
/// source path survives) is a copy that is not a move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyRecord {
    /// The source path, as it existed in `from_atom`.
    pub from: String,
    /// The atom `from` is copied from — always an atom earlier in canonical order than the one this
    /// record belongs to.
    pub from_atom: AtomId,
    /// The destination path, in this atom.
    pub to: String,
    /// Stated or derived.
    pub status: EpistemicStatus,
}

/// An author/committer identity, carried as a *claim* (`PR-3`), never verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Claimed name, as the source's raw bytes.
    pub name: Text,
    /// Claimed email, as the source's raw bytes, if the source gave one.
    pub email: Option<Text>,
}

/// Message and authorship metadata, as claims not verified facts (`PR-3`). Times are source-stated and
/// identity-bearing (they are part of what the source recorded).
///
/// **The one-claim rule:** `author`/`author_time` hold what the source attributes the change to —
/// brygge's best single claim of authorship. `committer`/`commit_time` hold only a **distinct**
/// recording identity or time the source itself states (e.g. Git's separate committer); a source with
/// no such distinction leaves them `None` rather than duplicating `author`/`author_time`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataClaims {
    /// Claimed author.
    pub author: Option<Identity>,
    /// Claimed author time.
    pub author_time: Option<Time>,
    /// Claimed committer — only when the source states one distinct from the author.
    pub committer: Option<Identity>,
    /// Claimed commit time — only when the source states one distinct from the author time.
    pub commit_time: Option<Time>,
    /// Claimed message.
    pub message: Option<Text>,
}

impl MetadataClaims {
    /// True when every claim is absent — the condition under which `ChangeAtom` omits its `metadata`
    /// field entirely (RFC 011 §2.4).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.author.is_none()
            && self.author_time.is_none()
            && self.committer.is_none()
            && self.commit_time.is_none()
            && self.message.is_none()
    }
}

/// A change atom's IR-local identity: SHA-256 over its canonical record encoding, excluding field 1
/// (D-4, RFC 011 §2.4). **Not** a source id and **not** a target node id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AtomId(pub [u8; 32]);

impl AtomId {
    /// A lowercase-hex rendering.
    #[must_use]
    pub fn to_hex(&self) -> String {
        to_hex(&self.0)
    }
}

impl std::fmt::Display for AtomId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// One atom of history (a commit / revision / reconstructed changeset), with an atom-level epistemic
/// status (`Stated` for git/hg/svn, `Derived` for a reconstructed CVS changeset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeAtom {
    /// The computed IR-local id.
    pub id: AtomId,
    /// Parent atom ids, order significant (`PR-2`).
    pub parents: Vec<AtomId>,
    /// The source's literal path operations, in canonical path order.
    pub ops: Vec<PathOp>,
    /// Marked copy records, in canonical order.
    pub copies: Vec<CopyRecord>,
    /// Message/authorship claims; absent when the source gave none.
    pub metadata: MetadataClaims,
    /// The source's opaque identity for this atom.
    pub source: SourceIdentity,
    /// Whether the atom itself is stated or derived.
    pub status: EpistemicStatus,
}

impl ChangeAtom {
    /// True when `copy` represents this atom moving `copy.from` to `copy.to`: `copy.from_atom` is this
    /// atom's first parent, **and** this atom's own ops delete `copy.from` (RFC 011 D-5). A copy that
    /// does not meet both conditions is a copy, not a move.
    #[must_use]
    pub fn is_move(&self, copy: &CopyRecord) -> bool {
        self.parents.first() == Some(&copy.from_atom)
            && self
                .ops
                .iter()
                .any(|op| matches!(op, PathOp::Delete { path, .. } if path == &copy.from))
    }
}

/// The kind of a ref pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefKind {
    /// A branch.
    Branch,
    /// A tag.
    Tag,
    /// A Mercurial-style bookmark.
    Bookmark,
    /// A Mercurial-style named branch.
    NamedBranch,
    /// A kind this build does not name.
    Other(String),
}

/// A tag/ref's own annotation, when the source carries one distinct from the ref pointer itself (e.g.
/// an annotated Git tag object) — RFC 011 D-6.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Annotation {
    /// Who made the annotation, if the source states one.
    pub tagger: Option<Identity>,
    /// When it was made, if the source states one.
    pub time: Option<Time>,
    /// The annotation's message, if the source states one.
    pub message: Option<Text>,
}

/// A ref pointer into the atom DAG (`PR-2`). A reconstructed SVN branch is `Derived`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRecord {
    /// The ref name.
    pub name: String,
    /// Its kind.
    pub kind: RefKind,
    /// The atom it points at.
    pub target: AtomId,
    /// Stated or derived.
    pub status: EpistemicStatus,
    /// The source's opaque identity for the ref, if any.
    pub source: Option<SourceIdentity>,
    /// The ref's own annotation, if the source carries one distinct from the pointer.
    pub annotation: Option<Annotation>,
}

/// The class of a dropped datum (RFC 002 D-2). Dropping is permitted only for the first two; the
/// never-silently-omit class (`PR-9`) must be carried or explicitly noted, never a silent drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossClass {
    /// Representation rather than assertion (packfiles, deltas, index, reflogs).
    Representation,
    /// Advisory data known to be unreliable (SVN mergeinfo, hg obsmarkers).
    AdvisoryUnreliable,
    /// Any other explicitly-recorded drop.
    Other,
}

/// One recorded drop: what class, what was dropped, and why (`HO-2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropRecord {
    /// The class.
    pub class: LossClass,
    /// What was dropped.
    pub what: String,
    /// Why it was safe to drop.
    pub reason: String,
}

/// The boundary of loss for an import (`HO-2/IR-4`). The *derived* side of the boundary is computed from
/// the atoms by [`crate::honesty::summary`]; this records the *dropped* side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LossBoundary {
    /// Everything the decoder dropped, by class, with reasons.
    pub dropped: Vec<DropRecord>,
}

/// Which kind of attention-worthy condition a [`Flag`] records (RFC 011 D-8): a typed replacement for
/// the old magic-string exit-30 detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    /// A source convention the decoder expected did not hold (e.g. an SVN branch layout it could not
    /// resolve).
    ConventionViolation,
    /// A confidence value fell below the decoder's own floor for the class of inference involved.
    BelowConfidenceFloor,
}

/// One flagged condition surfaced to the caller — drives CLI exit 30 (RFC 011 D-8). Unlike a
/// [`DropRecord`], nothing here was dropped; a flag says "this import needs your attention", not "this
/// datum is missing".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    /// The kind of condition.
    pub kind: FlagKind,
    /// What the flag concerns (a path, a revision range, a layout description).
    pub what: String,
    /// How many occurrences this flag summarizes (at least 1).
    pub count: u64,
    /// Why this was flagged rather than silently accepted.
    pub reason: String,
}

/// Per-import provenance (`PR-6`): what it was made from and by what. Separable from the history it
/// describes (`PX-02`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportProvenance {
    /// The source repository identity.
    pub source: SourceIdentity,
    /// The brygge version that produced this import.
    pub brygge_version: String,
    /// The decoder id.
    pub decoder: String,
    /// The decoder version.
    pub decoder_version: String,
    /// Every inference parameter that governed the import (`PR-5`), canonical/sorted.
    pub params: BTreeMap<String, String>,
}

/// The whole intermediate representation. The contract version this artifact conforms to is not a field
/// here — it travels in the container header (RFC 011 §2.3), not the metadata record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ir {
    /// The atoms, in canonical (topological, tiebroken) order.
    pub atoms: Vec<ChangeAtom>,
    /// The refs.
    pub refs: Vec<RefRecord>,
    /// The import provenance.
    pub provenance: ImportProvenance,
    /// The loss boundary.
    pub loss: LossBoundary,
    /// Flagged conditions (RFC 011 D-8).
    pub flags: Vec<Flag>,
    /// The content store.
    pub content: ContentStore,
}

// ---- canonical encode/decode (RFC 011 §2.1/§2.2/§2.4) --------------------------------------------

impl SourceKind {
    fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        let variant = match self {
            Self::Git => 0,
            Self::Hg => 1,
            Self::Svn => 2,
            Self::Cvs => 3,
            Self::Other(_) => 4,
        };
        w.uvarint(variant);
        let mut rw = RecordWriter::new();
        if let Self::Other(label) = self {
            rw.field(1, true, label.as_bytes().to_vec());
        }
        rw.finish_into(&mut w);
        w.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        match r.uvarint()? {
            v @ 0..=3 => {
                r.record_fields("SourceKind", |_, _, _| Ok(false))?;
                Ok(match v {
                    0 => Self::Git,
                    1 => Self::Hg,
                    2 => Self::Svn,
                    _ => Self::Cvs,
                })
            }
            4 => {
                let mut label = None;
                r.record_fields("SourceKind::Other", |r, id, len| {
                    if id == 1 {
                        label = Some(r.text(len)?);
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })?;
                Ok(Self::Other(label.ok_or_else(|| {
                    Error::NonCanonical("SourceKind::Other missing field 1 (label)".to_string())
                })?))
            }
            o => Err(Error::Decode(format!("unknown SourceKind variant {o}"))),
        }
    }
}

impl Text {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.bytes.clone());
        if let Some(enc) = &self.encoding {
            rw.field(2, true, enc.as_bytes().to_vec());
        }
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut bytes = None;
        let mut encoding = None;
        r.record_fields("Text", |r, id, len| match id {
            1 => {
                bytes = Some(r.bytes_exact(len)?);
                Ok(true)
            }
            2 => {
                encoding = Some(r.text(len)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            bytes: bytes
                .ok_or_else(|| Error::NonCanonical("Text missing field 1 (bytes)".to_string()))?,
            encoding,
        })
    }
}

impl Time {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, crate::canon::svarint_value(self.seconds));
        if let Some(off) = self.offset_minutes {
            rw.field(2, true, crate::canon::svarint_value(i64::from(off)));
        }
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut seconds = None;
        let mut offset_minutes = None;
        r.record_fields("Time", |r, id, _len| match id {
            1 => {
                seconds = Some(r.svarint()?);
                Ok(true)
            }
            2 => {
                let v = r.svarint()?;
                let v = i16::try_from(v).map_err(|_| {
                    Error::NonCanonical("Time.offset_minutes does not fit i16".to_string())
                })?;
                offset_minutes = Some(v);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            seconds: seconds
                .ok_or_else(|| Error::NonCanonical("Time missing field 1 (seconds)".to_string()))?,
            offset_minutes,
        })
    }
}

impl Signature {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.label.as_bytes().to_vec());
        rw.field(2, true, self.bytes.clone());
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut label = None;
        let mut bytes = None;
        r.record_fields("Signature", |r, id, len| match id {
            1 => {
                label = Some(r.text(len)?);
                Ok(true)
            }
            2 => {
                bytes = Some(r.bytes_exact(len)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            label: label.ok_or_else(|| {
                Error::NonCanonical("Signature missing field 1 (label)".to_string())
            })?,
            bytes: bytes.ok_or_else(|| {
                Error::NonCanonical("Signature missing field 2 (bytes)".to_string())
            })?,
        })
    }
}

impl Extra {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.label.as_bytes().to_vec());
        rw.field(2, true, self.bytes.clone());
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut label = None;
        let mut bytes = None;
        r.record_fields("Extra", |r, id, len| match id {
            1 => {
                label = Some(r.text(len)?);
                Ok(true)
            }
            2 => {
                bytes = Some(r.bytes_exact(len)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            label: label
                .ok_or_else(|| Error::NonCanonical("Extra missing field 1 (label)".to_string()))?,
            bytes: bytes
                .ok_or_else(|| Error::NonCanonical("Extra missing field 2 (bytes)".to_string()))?,
        })
    }
}

impl SourceIdentity {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.kind.encode());
        rw.field(2, true, self.repo_id.clone());
        rw.field(3, true, self.atom_id.clone());
        if !self.signatures.is_empty() {
            rw.field(
                4,
                true,
                list_value(self.signatures.iter().map(Signature::encode)),
            );
        }
        if !self.extras.is_empty() {
            rw.field(5, true, list_value(self.extras.iter().map(Extra::encode)));
        }
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut kind = None;
        let mut repo_id = None;
        let mut atom_id = None;
        let mut signatures = Vec::new();
        let mut extras = Vec::new();
        r.record_fields("SourceIdentity", |r, id, len| match id {
            1 => {
                kind = Some(SourceKind::decode(r)?);
                Ok(true)
            }
            2 => {
                repo_id = Some(r.bytes_exact(len)?);
                Ok(true)
            }
            3 => {
                atom_id = Some(r.bytes_exact(len)?);
                Ok(true)
            }
            4 => {
                signatures = r.list(Signature::decode)?;
                non_empty_list("SourceIdentity.signatures", &signatures)?;
                Ok(true)
            }
            5 => {
                extras = r.list(Extra::decode)?;
                non_empty_list("SourceIdentity.extras", &extras)?;
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            kind: kind.ok_or_else(|| {
                Error::NonCanonical("SourceIdentity missing field 1 (kind)".to_string())
            })?,
            repo_id: repo_id.ok_or_else(|| {
                Error::NonCanonical("SourceIdentity missing field 2 (repo_id)".to_string())
            })?,
            atom_id: atom_id.ok_or_else(|| {
                Error::NonCanonical("SourceIdentity missing field 3 (atom_id)".to_string())
            })?,
            signatures,
            extras,
        })
    }
}

fn non_empty_list<T>(field: &'static str, v: &[T]) -> Result<(), Error> {
    if v.is_empty() {
        Err(Error::NonCanonical(format!("{field} encoded but empty")))
    } else {
        Ok(())
    }
}

impl PathOp {
    /// The path this op concerns (its canonical sort key).
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Add { path, .. }
            | Self::Modify { path, .. }
            | Self::Delete { path, .. }
            | Self::Replace { path, .. } => path,
        }
    }
    fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        let (variant, path, blob, mode, status) = match self {
            Self::Add {
                path,
                blob,
                mode,
                status,
            } => (0, path, Some(blob), Some(*mode), status),
            Self::Modify {
                path,
                blob,
                mode,
                status,
            } => (1, path, Some(blob), Some(*mode), status),
            Self::Delete { path, status } => (2, path, None, None, status),
            Self::Replace {
                path,
                blob,
                mode,
                status,
            } => (3, path, Some(blob), Some(*mode), status),
        };
        w.uvarint(variant);
        let mut rw = RecordWriter::new();
        rw.field(1, true, path.as_bytes().to_vec());
        if let Some(blob) = blob {
            rw.field(2, true, blob.as_bytes().to_vec());
        }
        if let Some(mode) = mode {
            rw.field(3, true, crate::canon::uvarint_value(u64::from(mode)));
        }
        rw.field(4, true, status.encode());
        rw.finish_into(&mut w);
        w.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let variant = r.uvarint()?;
        let mut path = None;
        let mut blob = None;
        let mut mode = None;
        let mut status = None;
        r.record_fields("PathOp", |r, id, len| match id {
            1 => {
                path = Some(r.text(len)?);
                Ok(true)
            }
            2 => {
                blob = Some(BlobId(r.raw32()?));
                Ok(true)
            }
            3 => {
                mode = Some(read_u32(r)?);
                Ok(true)
            }
            4 => {
                status = Some(EpistemicStatus::decode(r)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        let path =
            path.ok_or_else(|| Error::NonCanonical("PathOp missing field 1 (path)".to_string()))?;
        let status = status
            .ok_or_else(|| Error::NonCanonical("PathOp missing field 4 (status)".to_string()))?;
        let need_blob_mode = |what: &'static str| {
            let blob = blob.ok_or_else(|| {
                Error::NonCanonical(format!("PathOp::{what} missing field 2 (blob)"))
            })?;
            let mode = mode.ok_or_else(|| {
                Error::NonCanonical(format!("PathOp::{what} missing field 3 (mode)"))
            })?;
            Ok::<_, Error>((blob, mode))
        };
        Ok(match variant {
            0 => {
                let (blob, mode) = need_blob_mode("Add")?;
                Self::Add {
                    path,
                    blob,
                    mode,
                    status,
                }
            }
            1 => {
                let (blob, mode) = need_blob_mode("Modify")?;
                Self::Modify {
                    path,
                    blob,
                    mode,
                    status,
                }
            }
            2 => Self::Delete { path, status },
            3 => {
                let (blob, mode) = need_blob_mode("Replace")?;
                Self::Replace {
                    path,
                    blob,
                    mode,
                    status,
                }
            }
            o => return Err(Error::Decode(format!("unknown PathOp variant {o}"))),
        })
    }
}

impl CopyRecord {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.from.as_bytes().to_vec());
        rw.field(2, true, self.from_atom.0.to_vec());
        rw.field(3, true, self.to.as_bytes().to_vec());
        rw.field(4, true, self.status.encode());
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut from = None;
        let mut from_atom = None;
        let mut to = None;
        let mut status = None;
        r.record_fields("CopyRecord", |r, id, len| match id {
            1 => {
                from = Some(r.text(len)?);
                Ok(true)
            }
            2 => {
                from_atom = Some(AtomId(r.raw32()?));
                Ok(true)
            }
            3 => {
                to = Some(r.text(len)?);
                Ok(true)
            }
            4 => {
                status = Some(EpistemicStatus::decode(r)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            from: from.ok_or_else(|| {
                Error::NonCanonical("CopyRecord missing field 1 (from)".to_string())
            })?,
            from_atom: from_atom.ok_or_else(|| {
                Error::NonCanonical("CopyRecord missing field 2 (from_atom)".to_string())
            })?,
            to: to.ok_or_else(|| {
                Error::NonCanonical("CopyRecord missing field 3 (to)".to_string())
            })?,
            status: status.ok_or_else(|| {
                Error::NonCanonical("CopyRecord missing field 4 (status)".to_string())
            })?,
        })
    }
}

impl Identity {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.name.encode());
        if let Some(email) = &self.email {
            rw.field(2, true, email.encode());
        }
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut name = None;
        let mut email = None;
        r.record_fields("Identity", |r, id, _len| match id {
            1 => {
                name = Some(Text::decode(r)?);
                Ok(true)
            }
            2 => {
                email = Some(Text::decode(r)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            name: name.ok_or_else(|| {
                Error::NonCanonical("Identity missing field 1 (name)".to_string())
            })?,
            email,
        })
    }
}

impl MetadataClaims {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        if let Some(a) = &self.author {
            rw.field(1, true, a.encode());
        }
        if let Some(t) = &self.author_time {
            rw.field(2, true, t.encode());
        }
        if let Some(c) = &self.committer {
            rw.field(3, true, c.encode());
        }
        if let Some(t) = &self.commit_time {
            rw.field(4, true, t.encode());
        }
        if let Some(m) = &self.message {
            rw.field(5, true, m.encode());
        }
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut out = Self::default();
        r.record_fields("MetadataClaims", |r, id, _len| match id {
            1 => {
                out.author = Some(Identity::decode(r)?);
                Ok(true)
            }
            2 => {
                out.author_time = Some(Time::decode(r)?);
                Ok(true)
            }
            3 => {
                out.committer = Some(Identity::decode(r)?);
                Ok(true)
            }
            4 => {
                out.commit_time = Some(Time::decode(r)?);
                Ok(true)
            }
            5 => {
                out.message = Some(Text::decode(r)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(out)
    }
}

fn read_u32(r: &mut CanonReader) -> Result<u32, Error> {
    u32::try_from(r.uvarint()?).map_err(|_| Error::Decode("mode out of range".to_string()))
}

impl ChangeAtom {
    /// Build fields 2–7 (everything but `id`) into `rw`, in ascending order — the shared body for both
    /// the full record encoding and [`Self::compute_id`] (RFC 011 §2.4: "the record without field 1").
    fn encode_body(&self, rw: &mut RecordWriter) {
        if !self.parents.is_empty() {
            rw.field(
                2,
                true,
                list_value(self.parents.iter().map(|id| id.0.to_vec())),
            );
        }
        if !self.ops.is_empty() {
            rw.field(3, true, list_value(self.ops.iter().map(PathOp::encode)));
        }
        if !self.copies.is_empty() {
            rw.field(
                4,
                true,
                list_value(self.copies.iter().map(CopyRecord::encode)),
            );
        }
        if !self.metadata.is_empty() {
            rw.field(5, true, self.metadata.encode());
        }
        rw.field(6, true, self.source.encode());
        rw.field(7, true, self.status.encode());
    }

    /// Compute the [`AtomId`] from the record body (fields 2–7, excluding `id` itself).
    #[must_use]
    pub(crate) fn compute_id(&self) -> AtomId {
        let mut rw = RecordWriter::new();
        self.encode_body(&mut rw);
        let mut hasher = Sha256::new();
        hasher.update(rw.into_bytes());
        AtomId(hasher.finalize().into())
    }

    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.id.0.to_vec());
        self.encode_body(&mut rw);
        rw.into_bytes()
    }

    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut id = None;
        let mut parents = Vec::new();
        let mut ops = Vec::new();
        let mut copies = Vec::new();
        let mut metadata = MetadataClaims::default();
        let mut source = None;
        let mut status = None;
        r.record_fields("ChangeAtom", |r, fid, _len| match fid {
            1 => {
                id = Some(AtomId(r.raw32()?));
                Ok(true)
            }
            2 => {
                parents = r.list(|r| Ok(AtomId(r.raw32()?)))?;
                non_empty_list("ChangeAtom.parents", &parents)?;
                Ok(true)
            }
            3 => {
                ops = r.list(PathOp::decode)?;
                non_empty_list("ChangeAtom.ops", &ops)?;
                Ok(true)
            }
            4 => {
                copies = r.list(CopyRecord::decode)?;
                non_empty_list("ChangeAtom.copies", &copies)?;
                Ok(true)
            }
            5 => {
                metadata = MetadataClaims::decode(r)?;
                if metadata.is_empty() {
                    return Err(Error::NonCanonical(
                        "ChangeAtom.metadata encoded but every claim absent".to_string(),
                    ));
                }
                Ok(true)
            }
            6 => {
                source = Some(SourceIdentity::decode(r)?);
                Ok(true)
            }
            7 => {
                status = Some(EpistemicStatus::decode(r)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        let id =
            id.ok_or_else(|| Error::NonCanonical("ChangeAtom missing field 1 (id)".to_string()))?;
        // Ops must be strictly ascending by path (§2.5): one op per path per atom.
        for (a, b) in ops.iter().zip(ops.iter().skip(1)) {
            if a.path() >= b.path() {
                return Err(Error::NonCanonical(
                    "ChangeAtom.ops are not strictly ascending by path".to_string(),
                ));
            }
        }
        for (a, b) in copies.iter().zip(copies.iter().skip(1)) {
            let ka = (&a.to, &a.from, a.from_atom);
            let kb = (&b.to, &b.from, b.from_atom);
            if ka >= kb {
                return Err(Error::NonCanonical(
                    "ChangeAtom.copies are not strictly ascending by (to, from, from_atom)"
                        .to_string(),
                ));
            }
        }
        let atom = Self {
            id,
            parents,
            ops,
            copies,
            metadata,
            source: source.ok_or_else(|| {
                Error::NonCanonical("ChangeAtom missing field 6 (source)".to_string())
            })?,
            status: status.ok_or_else(|| {
                Error::NonCanonical("ChangeAtom missing field 7 (status)".to_string())
            })?,
        };
        // Integrity: the stored id must match the recomputed one (RFC 011 §2.5).
        if atom.compute_id() != id {
            return Err(Error::Decode(
                "atom id does not match its contents".to_string(),
            ));
        }
        Ok(atom)
    }
}

impl RefKind {
    /// The variant number in the format's kind table: what the encoder writes, and the key the canonical
    /// order of refs is defined on (`ir-artifact-format.md` §6). Never derived from a name.
    pub(crate) fn variant(&self) -> u8 {
        match self {
            Self::Branch => 0,
            Self::Tag => 1,
            Self::Bookmark => 2,
            Self::NamedBranch => 3,
            Self::Other(_) => 4,
        }
    }
    fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.uvarint(u64::from(self.variant()));
        let mut rw = RecordWriter::new();
        if let Self::Other(label) = self {
            rw.field(1, true, label.as_bytes().to_vec());
        }
        rw.finish_into(&mut w);
        w.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        match r.uvarint()? {
            v @ 0..=3 => {
                r.record_fields("RefKind", |_, _, _| Ok(false))?;
                Ok(match v {
                    0 => Self::Branch,
                    1 => Self::Tag,
                    2 => Self::Bookmark,
                    _ => Self::NamedBranch,
                })
            }
            4 => {
                let mut label = None;
                r.record_fields("RefKind::Other", |r, id, len| {
                    if id == 1 {
                        label = Some(r.text(len)?);
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })?;
                Ok(Self::Other(label.ok_or_else(|| {
                    Error::NonCanonical("RefKind::Other missing field 1 (label)".to_string())
                })?))
            }
            o => Err(Error::Decode(format!("unknown RefKind variant {o}"))),
        }
    }
}

impl Annotation {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        if let Some(t) = &self.tagger {
            rw.field(1, true, t.encode());
        }
        if let Some(t) = &self.time {
            rw.field(2, true, t.encode());
        }
        if let Some(m) = &self.message {
            rw.field(3, true, m.encode());
        }
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut out = Self::default();
        r.record_fields("Annotation", |r, id, _len| match id {
            1 => {
                out.tagger = Some(Identity::decode(r)?);
                Ok(true)
            }
            2 => {
                out.time = Some(Time::decode(r)?);
                Ok(true)
            }
            3 => {
                out.message = Some(Text::decode(r)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(out)
    }
}

impl RefRecord {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.name.as_bytes().to_vec());
        rw.field(2, true, self.kind.encode());
        rw.field(3, true, self.target.0.to_vec());
        rw.field(4, true, self.status.encode());
        if let Some(s) = &self.source {
            rw.field(5, true, s.encode());
        }
        if let Some(a) = &self.annotation {
            rw.field(6, true, a.encode());
        }
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut name = None;
        let mut kind = None;
        let mut target = None;
        let mut status = None;
        let mut source = None;
        let mut annotation = None;
        r.record_fields("RefRecord", |r, id, len| match id {
            1 => {
                name = Some(r.text(len)?);
                Ok(true)
            }
            2 => {
                kind = Some(RefKind::decode(r)?);
                Ok(true)
            }
            3 => {
                target = Some(AtomId(r.raw32()?));
                Ok(true)
            }
            4 => {
                status = Some(EpistemicStatus::decode(r)?);
                Ok(true)
            }
            5 => {
                source = Some(SourceIdentity::decode(r)?);
                Ok(true)
            }
            6 => {
                annotation = Some(Annotation::decode(r)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            name: name.ok_or_else(|| {
                Error::NonCanonical("RefRecord missing field 1 (name)".to_string())
            })?,
            kind: kind.ok_or_else(|| {
                Error::NonCanonical("RefRecord missing field 2 (kind)".to_string())
            })?,
            target: target.ok_or_else(|| {
                Error::NonCanonical("RefRecord missing field 3 (target)".to_string())
            })?,
            status: status.ok_or_else(|| {
                Error::NonCanonical("RefRecord missing field 4 (status)".to_string())
            })?,
            source,
            annotation,
        })
    }
}

impl LossClass {
    /// The variant number in the format's class table: what the encoder writes, and the key the canonical
    /// order of drops is defined on (`ir-artifact-format.md` §6). Never derived from a name.
    pub(crate) fn variant(self) -> u8 {
        match self {
            Self::Representation => 0,
            Self::AdvisoryUnreliable => 1,
            Self::Other => 2,
        }
    }
    fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.uvarint(u64::from(self.variant()));
        RecordWriter::new().finish_into(&mut w);
        w.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let variant = r.uvarint()?;
        r.record_fields("LossClass", |_, _, _| Ok(false))?;
        Ok(match variant {
            0 => Self::Representation,
            1 => Self::AdvisoryUnreliable,
            2 => Self::Other,
            o => return Err(Error::Decode(format!("unknown LossClass variant {o}"))),
        })
    }
}

impl DropRecord {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.class.encode());
        rw.field(2, true, self.what.as_bytes().to_vec());
        rw.field(3, true, self.reason.as_bytes().to_vec());
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut class = None;
        let mut what = None;
        let mut reason = None;
        r.record_fields("DropRecord", |r, id, len| match id {
            1 => {
                class = Some(LossClass::decode(r)?);
                Ok(true)
            }
            2 => {
                what = Some(r.text(len)?);
                Ok(true)
            }
            3 => {
                reason = Some(r.text(len)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            class: class.ok_or_else(|| {
                Error::NonCanonical("DropRecord missing field 1 (class)".to_string())
            })?,
            what: what.ok_or_else(|| {
                Error::NonCanonical("DropRecord missing field 2 (what)".to_string())
            })?,
            reason: reason.ok_or_else(|| {
                Error::NonCanonical("DropRecord missing field 3 (reason)".to_string())
            })?,
        })
    }
}

impl FlagKind {
    /// The variant number in the format's kind table: what the encoder writes, and the key the canonical
    /// order of flags is defined on (`ir-artifact-format.md` §6). Never derived from a name.
    pub(crate) fn variant(self) -> u8 {
        match self {
            Self::ConventionViolation => 0,
            Self::BelowConfidenceFloor => 1,
        }
    }
    fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.uvarint(u64::from(self.variant()));
        RecordWriter::new().finish_into(&mut w);
        w.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let variant = r.uvarint()?;
        r.record_fields("FlagKind", |_, _, _| Ok(false))?;
        Ok(match variant {
            0 => Self::ConventionViolation,
            1 => Self::BelowConfidenceFloor,
            o => return Err(Error::Decode(format!("unknown FlagKind variant {o}"))),
        })
    }
}

impl Flag {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.kind.encode());
        rw.field(2, true, self.what.as_bytes().to_vec());
        rw.field(3, true, crate::canon::uvarint_value(self.count));
        rw.field(4, true, self.reason.as_bytes().to_vec());
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut kind = None;
        let mut what = None;
        let mut count = None;
        let mut reason = None;
        r.record_fields("Flag", |r, id, len| match id {
            1 => {
                kind = Some(FlagKind::decode(r)?);
                Ok(true)
            }
            2 => {
                what = Some(r.text(len)?);
                Ok(true)
            }
            3 => {
                count = Some(r.uvarint()?);
                Ok(true)
            }
            4 => {
                reason = Some(r.text(len)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
        let count =
            count.ok_or_else(|| Error::NonCanonical("Flag missing field 3 (count)".to_string()))?;
        if count < 1 {
            return Err(Error::NonCanonical(
                "Flag.count must be at least 1".to_string(),
            ));
        }
        Ok(Self {
            kind: kind
                .ok_or_else(|| Error::NonCanonical("Flag missing field 1 (kind)".to_string()))?,
            what: what
                .ok_or_else(|| Error::NonCanonical("Flag missing field 2 (what)".to_string()))?,
            count,
            reason: reason
                .ok_or_else(|| Error::NonCanonical("Flag missing field 4 (reason)".to_string()))?,
        })
    }
}

impl ImportProvenance {
    fn encode(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        rw.field(1, true, self.source.encode());
        rw.field(2, true, self.brygge_version.as_bytes().to_vec());
        rw.field(3, true, self.decoder.as_bytes().to_vec());
        rw.field(4, true, self.decoder_version.as_bytes().to_vec());
        if !self.params.is_empty() {
            rw.field(
                5,
                true,
                map_value(self.params.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
            );
        }
        rw.into_bytes()
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let mut source = None;
        let mut brygge_version = None;
        let mut decoder = None;
        let mut decoder_version = None;
        let mut params = BTreeMap::new();
        r.record_fields("ImportProvenance", |r, id, len| match id {
            1 => {
                source = Some(SourceIdentity::decode(r)?);
                Ok(true)
            }
            2 => {
                brygge_version = Some(r.text(len)?);
                Ok(true)
            }
            3 => {
                decoder = Some(r.text(len)?);
                Ok(true)
            }
            4 => {
                decoder_version = Some(r.text(len)?);
                Ok(true)
            }
            5 => {
                params = r.map()?;
                if params.is_empty() {
                    return Err(Error::NonCanonical(
                        "ImportProvenance.params encoded but empty".to_string(),
                    ));
                }
                Ok(true)
            }
            _ => Ok(false),
        })?;
        Ok(Self {
            source: source.ok_or_else(|| {
                Error::NonCanonical("ImportProvenance missing field 1 (source)".to_string())
            })?,
            brygge_version: brygge_version.ok_or_else(|| {
                Error::NonCanonical("ImportProvenance missing field 2 (brygge_version)".to_string())
            })?,
            decoder: decoder.ok_or_else(|| {
                Error::NonCanonical("ImportProvenance missing field 3 (decoder)".to_string())
            })?,
            decoder_version: decoder_version.ok_or_else(|| {
                Error::NonCanonical(
                    "ImportProvenance missing field 4 (decoder_version)".to_string(),
                )
            })?,
            params,
        })
    }
}

impl Ir {
    /// Encode the metadata record (everything but the blob bytes) — the container's `metadata` section
    /// (RFC 011 §2.3).
    pub(crate) fn encode_metadata(&self) -> Vec<u8> {
        let mut rw = RecordWriter::new();
        if !self.atoms.is_empty() {
            rw.field(
                1,
                true,
                list_value(self.atoms.iter().map(ChangeAtom::encode)),
            );
        }
        if !self.refs.is_empty() {
            rw.field(2, true, list_value(self.refs.iter().map(RefRecord::encode)));
        }
        rw.field(3, true, self.provenance.encode());
        if !self.loss.dropped.is_empty() {
            rw.field(
                4,
                true,
                list_value(self.loss.dropped.iter().map(DropRecord::encode)),
            );
        }
        if !self.flags.is_empty() {
            rw.field(5, true, list_value(self.flags.iter().map(Flag::encode)));
        }
        rw.into_bytes()
    }

    /// Decode the metadata record (blobs are read separately by [`crate::artifact`]).
    pub(crate) fn decode_metadata(r: &mut CanonReader) -> Result<Self, Error> {
        let mut atoms = Vec::new();
        let mut refs = Vec::new();
        let mut provenance = None;
        let mut dropped = Vec::new();
        let mut flags = Vec::new();
        r.record_fields("Ir", |r, id, _len| match id {
            1 => {
                atoms = r.list(ChangeAtom::decode)?;
                non_empty_list("Ir.atoms", &atoms)?;
                Ok(true)
            }
            2 => {
                refs = r.list(RefRecord::decode)?;
                non_empty_list("Ir.refs", &refs)?;
                Ok(true)
            }
            3 => {
                provenance = Some(ImportProvenance::decode(r)?);
                Ok(true)
            }
            4 => {
                dropped = r.list(DropRecord::decode)?;
                non_empty_list("Ir.dropped", &dropped)?;
                Ok(true)
            }
            5 => {
                flags = r.list(Flag::decode)?;
                non_empty_list("Ir.flags", &flags)?;
                Ok(true)
            }
            _ => Ok(false),
        })?;
        for (a, b) in refs.iter().zip(refs.iter().skip(1)) {
            let ka = (&a.name, a.kind.variant());
            let kb = (&b.name, b.kind.variant());
            if ka >= kb {
                return Err(Error::NonCanonical(
                    "Ir.refs are not strictly ascending by (name, kind variant)".to_string(),
                ));
            }
        }
        for (a, b) in dropped.iter().zip(dropped.iter().skip(1)) {
            let ka = (a.class.variant(), &a.what);
            let kb = (b.class.variant(), &b.what);
            if ka >= kb {
                return Err(Error::NonCanonical(
                    "Ir.dropped is not strictly ascending by (class variant, what)".to_string(),
                ));
            }
        }
        for (a, b) in flags.iter().zip(flags.iter().skip(1)) {
            let ka = (a.kind.variant(), &a.what);
            let kb = (b.kind.variant(), &b.what);
            if ka >= kb {
                return Err(Error::NonCanonical(
                    "Ir.flags are not strictly ascending by (kind variant, what)".to_string(),
                ));
            }
        }
        Ok(Self {
            atoms,
            refs,
            provenance: provenance.ok_or_else(|| {
                Error::NonCanonical("Ir missing field 3 (provenance)".to_string())
            })?,
            loss: LossBoundary { dropped },
            flags,
            content: ContentStore::new(),
        })
    }
}

#[cfg(test)]
mod tests;

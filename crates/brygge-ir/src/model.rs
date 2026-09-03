//! The IR types (RFC 001 D-1/D-3/D-4).
//!
//! A content store (in [`crate::content`]) plus a DAG of [`ChangeAtom`]s plus per-import
//! [`ImportProvenance`] and [`LossBoundary`]. Each atom holds the source's **literal** path operations
//! and — separately — marked [`RenameHint`]s, so a rename never hides the delete+create the source
//! actually recorded (D-3). There is **no node-identity type**: the IR carries evidence for identity,
//! never identity (D-4). Every type has one canonical encoding (RFC 003 D-1), used for the artifact and
//! for [`AtomId`] computation.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::Error;
use crate::canon::{CanonReader, CanonWriter};
use crate::content::{ContentStore, to_hex};
use crate::status::EpistemicStatus;
use crate::version::ContractVersion;

pub use crate::content::BlobId;

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

/// The source's own opaque identifiers and signatures (`PR-4/IR-3`) — the only cryptographic link back
/// to the original. Carried unchanged; they verify nothing in any target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdentity {
    /// The source system.
    pub kind: SourceKind,
    /// Opaque repository identity (e.g. a root commit id, a UUID).
    pub repo_id: Vec<u8>,
    /// Opaque atom identity (e.g. a Git commit SHA, an SVN revision, a CVS revision tag).
    pub atom_id: Vec<u8>,
    /// Opaque signatures (e.g. a GPG signature over the source's object).
    pub signatures: Vec<Vec<u8>>,
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
}

/// A marked hint that one path is a rename/copy of another (D-3). `Stated` when the source recorded it
/// (e.g. `hg mv`); `Derived` when brygge inferred it (e.g. Git similarity). Sits *beside* the literal
/// delete+create ops, never replacing them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameHint {
    /// The source path.
    pub from: String,
    /// The destination path.
    pub to: String,
    /// Stated or derived.
    pub status: EpistemicStatus,
}

/// An author/committer identity, carried as a *claim* (`PR-3`), never verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Claimed name.
    pub name: String,
    /// Claimed email.
    pub email: String,
}

/// Message and authorship metadata, as claims not verified facts (`PR-3`). Times are source-stated and
/// identity-bearing (they are part of what the source recorded).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataClaims {
    /// Claimed author.
    pub author: Option<Identity>,
    /// Claimed committer.
    pub committer: Option<Identity>,
    /// Claimed message.
    pub message: Option<String>,
    /// Claimed author time (source epoch seconds).
    pub author_time: Option<i64>,
    /// Claimed commit time (source epoch seconds).
    pub commit_time: Option<i64>,
}

/// A change atom's IR-local identity: SHA-256 over its canonical identity bytes (D-4, RFC 003 D-3).
/// **Not** a source id and **not** a target node id.
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
    /// Marked rename hints, in canonical order.
    pub rename_hints: Vec<RenameHint>,
    /// Message/authorship claims.
    pub metadata: MetadataClaims,
    /// The source's opaque identity for this atom.
    pub source: SourceIdentity,
    /// Whether the atom itself is stated or derived.
    pub status: EpistemicStatus,
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

/// Per-import provenance (`PR-6`): what it was made from and by what. Separable from the history it
/// describes (`PX-02`). `import_time` is **provenance-only** — excluded from the integrity digest and
/// from all ids (`ID-4`, RFC 003 D-4).
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
    /// When the import ran (source epoch seconds) — provenance-only, never identity-bearing.
    pub import_time: Option<i64>,
}

/// The whole intermediate representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ir {
    /// The IR contract version this artifact conforms to.
    pub contract_version: ContractVersion,
    /// The atoms, in canonical (topological, tiebroken) order.
    pub atoms: Vec<ChangeAtom>,
    /// The refs.
    pub refs: Vec<RefRecord>,
    /// The import provenance.
    pub provenance: ImportProvenance,
    /// The loss boundary.
    pub loss: LossBoundary,
    /// The content store.
    pub content: ContentStore,
}

// ---- canonical encode/decode ---------------------------------------------------------------------

impl SourceKind {
    fn encode(&self, w: &mut CanonWriter) {
        match self {
            Self::Git => w.u8(0),
            Self::Hg => w.u8(1),
            Self::Svn => w.u8(2),
            Self::Cvs => w.u8(3),
            Self::Other(s) => {
                w.u8(255);
                w.str(s);
            }
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        Ok(match r.u8()? {
            0 => Self::Git,
            1 => Self::Hg,
            2 => Self::Svn,
            3 => Self::Cvs,
            255 => Self::Other(r.str()?),
            o => return Err(Error::Decode(format!("unknown source kind {o}"))),
        })
    }
}

impl SourceIdentity {
    fn encode(&self, w: &mut CanonWriter) {
        self.kind.encode(w);
        w.bytes(&self.repo_id);
        w.bytes(&self.atom_id);
        w.uvarint(self.signatures.len() as u64);
        for s in &self.signatures {
            w.bytes(s);
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let kind = SourceKind::decode(r)?;
        let repo_id = r.bytes()?;
        let atom_id = r.bytes()?;
        let n = r.uvarint()?;
        let mut signatures = Vec::new();
        for _ in 0..n {
            signatures.push(r.bytes()?);
        }
        Ok(Self {
            kind,
            repo_id,
            atom_id,
            signatures,
        })
    }
}

impl PathOp {
    /// The path this op concerns (its canonical sort key).
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Add { path, .. } | Self::Modify { path, .. } | Self::Delete { path, .. } => path,
        }
    }
    fn encode(&self, w: &mut CanonWriter) {
        match self {
            Self::Add {
                path,
                blob,
                mode,
                status,
            } => {
                w.u8(0);
                w.str(path);
                w.raw32(blob.as_bytes());
                w.uvarint(u64::from(*mode));
                status.encode(w);
            }
            Self::Modify {
                path,
                blob,
                mode,
                status,
            } => {
                w.u8(1);
                w.str(path);
                w.raw32(blob.as_bytes());
                w.uvarint(u64::from(*mode));
                status.encode(w);
            }
            Self::Delete { path, status } => {
                w.u8(2);
                w.str(path);
                status.encode(w);
            }
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        Ok(match r.u8()? {
            0 => Self::Add {
                path: r.str()?,
                blob: BlobId(r.raw32()?),
                mode: read_u32(r)?,
                status: EpistemicStatus::decode(r)?,
            },
            1 => Self::Modify {
                path: r.str()?,
                blob: BlobId(r.raw32()?),
                mode: read_u32(r)?,
                status: EpistemicStatus::decode(r)?,
            },
            2 => Self::Delete {
                path: r.str()?,
                status: EpistemicStatus::decode(r)?,
            },
            o => return Err(Error::Decode(format!("bad path-op tag {o}"))),
        })
    }
}

impl RenameHint {
    fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.from);
        w.str(&self.to);
        self.status.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        Ok(Self {
            from: r.str()?,
            to: r.str()?,
            status: EpistemicStatus::decode(r)?,
        })
    }
}

impl Identity {
    fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.name);
        w.str(&self.email);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        Ok(Self {
            name: r.str()?,
            email: r.str()?,
        })
    }
}

fn encode_opt_identity(o: &Option<Identity>, w: &mut CanonWriter) {
    match o {
        None => w.u8(0),
        Some(i) => {
            w.u8(1);
            i.encode(w);
        }
    }
}
fn decode_opt_identity(r: &mut CanonReader) -> Result<Option<Identity>, Error> {
    Ok(match r.u8()? {
        0 => None,
        1 => Some(Identity::decode(r)?),
        o => return Err(Error::Decode(format!("bad option tag {o}"))),
    })
}
fn encode_opt_str(o: &Option<String>, w: &mut CanonWriter) {
    match o {
        None => w.u8(0),
        Some(s) => {
            w.u8(1);
            w.str(s);
        }
    }
}
fn decode_opt_str(r: &mut CanonReader) -> Result<Option<String>, Error> {
    Ok(match r.u8()? {
        0 => None,
        1 => Some(r.str()?),
        o => return Err(Error::Decode(format!("bad option tag {o}"))),
    })
}
fn encode_opt_i64(o: Option<i64>, w: &mut CanonWriter) {
    match o {
        None => w.u8(0),
        Some(v) => {
            w.u8(1);
            w.ivarint(v);
        }
    }
}
fn decode_opt_i64(r: &mut CanonReader) -> Result<Option<i64>, Error> {
    Ok(match r.u8()? {
        0 => None,
        1 => Some(r.ivarint()?),
        o => return Err(Error::Decode(format!("bad option tag {o}"))),
    })
}
fn read_u32(r: &mut CanonReader) -> Result<u32, Error> {
    u32::try_from(r.uvarint()?).map_err(|_| Error::Decode("mode out of range".to_string()))
}

impl MetadataClaims {
    fn encode(&self, w: &mut CanonWriter) {
        encode_opt_identity(&self.author, w);
        encode_opt_identity(&self.committer, w);
        encode_opt_str(&self.message, w);
        encode_opt_i64(self.author_time, w);
        encode_opt_i64(self.commit_time, w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        Ok(Self {
            author: decode_opt_identity(r)?,
            committer: decode_opt_identity(r)?,
            message: decode_opt_str(r)?,
            author_time: decode_opt_i64(r)?,
            commit_time: decode_opt_i64(r)?,
        })
    }
}

impl ChangeAtom {
    /// Encode everything the atom asserts *except* its id — the input to [`AtomId`] and the canonical
    /// per-atom bytes. Assumes `ops`/`rename_hints` are already in canonical order (the builder ensures).
    pub(crate) fn encode_identity(&self, w: &mut CanonWriter) {
        w.uvarint(self.parents.len() as u64);
        for p in &self.parents {
            w.raw32(&p.0);
        }
        w.uvarint(self.ops.len() as u64);
        for op in &self.ops {
            op.encode(w);
        }
        w.uvarint(self.rename_hints.len() as u64);
        for h in &self.rename_hints {
            h.encode(w);
        }
        self.metadata.encode(w);
        self.source.encode(w);
        self.status.encode(w);
    }

    /// Compute the [`AtomId`] from the identity bytes.
    #[must_use]
    pub(crate) fn compute_id(&self) -> AtomId {
        let mut w = CanonWriter::new();
        self.encode_identity(&mut w);
        let mut hasher = Sha256::new();
        hasher.update(w.as_bytes());
        AtomId(hasher.finalize().into())
    }

    fn encode(&self, w: &mut CanonWriter) {
        w.raw32(&self.id.0);
        self.encode_identity(w);
    }

    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let id = AtomId(r.raw32()?);
        let np = r.uvarint()?;
        let mut parents = Vec::new();
        for _ in 0..np {
            parents.push(AtomId(r.raw32()?));
        }
        let no = r.uvarint()?;
        let mut ops = Vec::new();
        for _ in 0..no {
            ops.push(PathOp::decode(r)?);
        }
        let nh = r.uvarint()?;
        let mut rename_hints = Vec::new();
        for _ in 0..nh {
            rename_hints.push(RenameHint::decode(r)?);
        }
        let metadata = MetadataClaims::decode(r)?;
        let source = SourceIdentity::decode(r)?;
        let status = EpistemicStatus::decode(r)?;
        let atom = Self {
            id,
            parents,
            ops,
            rename_hints,
            metadata,
            source,
            status,
        };
        // Integrity: the stored id must match the recomputed one.
        if atom.compute_id() != id {
            return Err(Error::Decode(
                "atom id does not match its contents".to_string(),
            ));
        }
        Ok(atom)
    }
}

impl RefKind {
    fn encode(&self, w: &mut CanonWriter) {
        match self {
            Self::Branch => w.u8(0),
            Self::Tag => w.u8(1),
            Self::Bookmark => w.u8(2),
            Self::NamedBranch => w.u8(3),
            Self::Other(s) => {
                w.u8(255);
                w.str(s);
            }
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        Ok(match r.u8()? {
            0 => Self::Branch,
            1 => Self::Tag,
            2 => Self::Bookmark,
            3 => Self::NamedBranch,
            255 => Self::Other(r.str()?),
            o => return Err(Error::Decode(format!("unknown ref kind {o}"))),
        })
    }
}

impl RefRecord {
    fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.name);
        self.kind.encode(w);
        w.raw32(&self.target.0);
        self.status.encode(w);
        match &self.source {
            None => w.u8(0),
            Some(s) => {
                w.u8(1);
                s.encode(w);
            }
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let name = r.str()?;
        let kind = RefKind::decode(r)?;
        let target = AtomId(r.raw32()?);
        let status = EpistemicStatus::decode(r)?;
        let source = match r.u8()? {
            0 => None,
            1 => Some(SourceIdentity::decode(r)?),
            o => return Err(Error::Decode(format!("bad option tag {o}"))),
        };
        Ok(Self {
            name,
            kind,
            target,
            status,
            source,
        })
    }
}

impl LossClass {
    fn encode(&self, w: &mut CanonWriter) {
        w.u8(match self {
            Self::Representation => 0,
            Self::AdvisoryUnreliable => 1,
            Self::Other => 2,
        });
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        Ok(match r.u8()? {
            0 => Self::Representation,
            1 => Self::AdvisoryUnreliable,
            2 => Self::Other,
            o => return Err(Error::Decode(format!("bad loss class {o}"))),
        })
    }
}

impl LossBoundary {
    fn encode(&self, w: &mut CanonWriter) {
        w.uvarint(self.dropped.len() as u64);
        for d in &self.dropped {
            d.class.encode(w);
            w.str(&d.what);
            w.str(&d.reason);
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, Error> {
        let n = r.uvarint()?;
        let mut dropped = Vec::new();
        for _ in 0..n {
            let class = LossClass::decode(r)?;
            let what = r.str()?;
            let reason = r.str()?;
            dropped.push(DropRecord {
                class,
                what,
                reason,
            });
        }
        Ok(Self { dropped })
    }
}

impl ImportProvenance {
    /// Encode provenance. `include_import_time` is false for the identity/digest encoding (`ID-4`).
    fn encode(&self, w: &mut CanonWriter, include_import_time: bool) {
        self.source.encode(w);
        w.str(&self.brygge_version);
        w.str(&self.decoder);
        w.str(&self.decoder_version);
        w.uvarint(self.params.len() as u64);
        for (k, v) in &self.params {
            w.str(k);
            w.str(v);
        }
        if include_import_time {
            encode_opt_i64(self.import_time, w);
        }
    }
    fn decode(r: &mut CanonReader, include_import_time: bool) -> Result<Self, Error> {
        let source = SourceIdentity::decode(r)?;
        let brygge_version = r.str()?;
        let decoder = r.str()?;
        let decoder_version = r.str()?;
        let n = r.uvarint()?;
        let mut params = BTreeMap::new();
        for _ in 0..n {
            let k = r.str()?;
            let v = r.str()?;
            params.insert(k, v);
        }
        let import_time = if include_import_time {
            decode_opt_i64(r)?
        } else {
            None
        };
        Ok(Self {
            source,
            brygge_version,
            decoder,
            decoder_version,
            params,
            import_time,
        })
    }
}

impl Ir {
    /// Encode the metadata section (everything but the blob bytes). `include_import_time` is false for
    /// the digest/identity encoding (RFC 003 D-4) and true for the stored form.
    pub(crate) fn encode_metadata(&self, w: &mut CanonWriter, include_import_time: bool) {
        w.uvarint(u64::from(self.contract_version.major));
        w.uvarint(u64::from(self.contract_version.minor));
        w.uvarint(u64::from(self.contract_version.patch));
        w.uvarint(self.atoms.len() as u64);
        for a in &self.atoms {
            a.encode(w);
        }
        w.uvarint(self.refs.len() as u64);
        for rf in &self.refs {
            rf.encode(w);
        }
        self.provenance.encode(w, include_import_time);
        self.loss.encode(w);
    }

    /// Decode the metadata section (blobs are read separately by [`crate::artifact`]).
    pub(crate) fn decode_metadata(r: &mut CanonReader) -> Result<Self, Error> {
        let contract_version = ContractVersion::new(read_u32(r)?, read_u32(r)?, read_u32(r)?);
        let na = r.uvarint()?;
        let mut atoms = Vec::new();
        for _ in 0..na {
            atoms.push(ChangeAtom::decode(r)?);
        }
        let nr = r.uvarint()?;
        let mut refs = Vec::new();
        for _ in 0..nr {
            refs.push(RefRecord::decode(r)?);
        }
        let provenance = ImportProvenance::decode(r, true)?;
        let loss = LossBoundary::decode(r)?;
        Ok(Self {
            contract_version,
            atoms,
            refs,
            provenance,
            loss,
            content: ContentStore::new(),
        })
    }
}

#[cfg(test)]
mod tests;

//! The content-addressed blob store (RFC 001 D-1, RFC 003 D-2/D-3; the file backing: RFC 015 D-3).
//!
//! File bytes are stored once per [`BlobId`] (SHA-256 of the bytes) — dedup, integrity, and determinism
//! for free. Path operations in the model reference blobs by id; the store owns the bytes.
//!
//! **Two backings, one surface.** The default, [`ContentStore::new`], keeps every blob in memory, as
//! before. [`ContentStore::spilling`] keeps them in a private spill file instead, holding only an
//! index (`BlobId` → offset, length) in memory. Both answer the same calls — `get`/`iter_sorted` read
//! **owned, fallible** bytes either way (RFC 015 D-3: a file-backed blob cannot be lent as `&[u8]`), so
//! no caller branches on which backing it has. A blob already present (the same id) is never written
//! twice, in either backing.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{Error, Result};

mod spill;

/// A content address: the SHA-256 of a blob's bytes (RFC 003 D-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobId(pub [u8; 32]);

impl BlobId {
    /// Compute the id of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// The 32 raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// A lowercase hex rendering, for inspection and reports.
    #[must_use]
    pub fn to_hex(&self) -> String {
        to_hex(&self.0)
    }
}

impl std::fmt::Display for BlobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Where a blob's bytes live: in memory, or at an offset/length in the spill file.
#[derive(Debug)]
enum Backing {
    Memory(BTreeMap<BlobId, Vec<u8>>),
    /// `RefCell`, not `&mut`: `get`/`iter_sorted` read (seek and read on the shared file handle)
    /// through `&self`, so a caller never needs mutable access just to read a blob back — the same
    /// shape the memory backing already offers. Single-threaded use only (brygge-ir makes no
    /// cross-thread claim for `Ir`/`ContentStore`); a borrow conflict would be a decoder bug, not
    /// input-dependent, so a panic on a held borrow is acceptable here.
    File {
        file: RefCell<std::fs::File>,
        index: BTreeMap<BlobId, (u64, u64)>,
    },
}

/// A set of blobs keyed by content address, in one of two backings (above). **Equality** is the set of
/// blob ids, not a byte comparison: a `BlobId` is the SHA-256 of its bytes, so two stores with the same
/// id set hold the same content (collision-resistant hashing; `Ir`'s own `PartialEq`, used by
/// `verify --against-source`, relies on exactly this). Iterates in sorted `BlobId` order (deterministic
/// — `VF-1`).
#[derive(Debug)]
pub struct ContentStore {
    backing: Backing,
}

impl Default for ContentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for ContentStore {
    fn eq(&self, other: &Self) -> bool {
        self.ids().eq(other.ids())
    }
}
impl Eq for ContentStore {}

impl ContentStore {
    /// An empty, in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            backing: Backing::Memory(BTreeMap::new()),
        }
    }

    /// An empty store backed by a private spill file created fresh in `dir` (RFC 015 D-3/D-3a). The
    /// file is removed from `dir` immediately after opening (Unix) or opened with delete-on-close
    /// (Windows): it is never left behind in `dir`, however the process ends. `dir` must already exist.
    ///
    /// # Errors
    /// [`Error::Io`] if `dir` does not exist or is not a directory, or the spill file cannot be
    /// created.
    pub fn spilling(dir: &Path) -> Result<Self> {
        let file = spill::create(dir)?;
        Ok(Self {
            backing: Backing::File {
                file: RefCell::new(file),
                index: BTreeMap::new(),
            },
        })
    }

    /// Insert `bytes`, returning its content address. Inserting identical bytes twice is idempotent —
    /// the second insert neither writes nor allocates again.
    ///
    /// # Errors
    /// [`Error::Io`] if the blob cannot be appended to the spill file (for example, the disk is full).
    pub fn insert(&mut self, bytes: Vec<u8>) -> Result<BlobId> {
        let id = BlobId::of(&bytes);
        match &mut self.backing {
            Backing::Memory(blobs) => {
                blobs.entry(id).or_insert(bytes);
            }
            Backing::File { file, index } => {
                if let std::collections::btree_map::Entry::Vacant(e) = index.entry(id) {
                    let mut f = file.borrow_mut();
                    let offset = f.seek(SeekFrom::End(0)).map_err(Error::io)?;
                    f.write_all(&bytes).map_err(Error::io)?;
                    e.insert((offset, bytes.len() as u64));
                }
            }
        }
        Ok(id)
    }

    /// The bytes for `id`, if present.
    ///
    /// # Errors
    /// [`Error::Io`] if `id` is present but its bytes could not be read back from the spill file (for
    /// example, a truncated file).
    pub fn get(&self, id: &BlobId) -> Result<Option<Vec<u8>>> {
        match &self.backing {
            Backing::Memory(blobs) => Ok(blobs.get(id).cloned()),
            Backing::File { file, index } => {
                let Some(&(offset, len)) = index.get(id) else {
                    return Ok(None);
                };
                let mut f = file.borrow_mut();
                f.seek(SeekFrom::Start(offset)).map_err(Error::io)?;
                let mut buf =
                    vec![0u8; usize::try_from(len).map_err(|e| Error::io_msg(e.to_string()))?];
                f.read_exact(&mut buf).map_err(Error::io)?;
                Ok(Some(buf))
            }
        }
    }

    /// True when `id` is present.
    #[must_use]
    pub fn contains(&self, id: &BlobId) -> bool {
        match &self.backing {
            Backing::Memory(blobs) => blobs.contains_key(id),
            Backing::File { index, .. } => index.contains_key(id),
        }
    }

    /// The number of distinct blobs.
    #[must_use]
    pub fn len(&self) -> usize {
        match &self.backing {
            Backing::Memory(blobs) => blobs.len(),
            Backing::File { index, .. } => index.len(),
        }
    }

    /// True when the store holds no blobs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total content bytes across all blobs (for the fidelity report). Known from the index alone —
    /// reading no blob back.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        match &self.backing {
            Backing::Memory(blobs) => blobs.values().map(|b| b.len() as u64).sum(),
            Backing::File { index, .. } => index.values().map(|&(_, len)| len).sum(),
        }
    }

    /// Every blob id, in sorted order — the canonical order for serialization and the digest. Reading
    /// a blob's bytes is a separate [`ContentStore::get`] per id (RFC 015 D-3: at most one blob is ever
    /// in memory at a time this way).
    pub fn ids(&self) -> impl Iterator<Item = &BlobId> {
        match &self.backing {
            Backing::Memory(blobs) => Either::Left(blobs.keys()),
            Backing::File { index, .. } => Either::Right(index.keys()),
        }
    }

    /// Keep only the blobs `keep` accepts, dropping the rest from the **index** — no blob already
    /// written is copied, re-hashed, or moved (RFC 015 D-2: `IrBuilder::finish` used to prune by
    /// building a second store with `insert`, a full copy of every referenced blob while the old store
    /// was still alive). For a file-backed store, the unreferenced bytes stay physically in the spill
    /// file — harmless, since the whole file is temporary and never read except by id through the
    /// index — but are no longer indexed, so `write_to` never streams them.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&BlobId) -> bool) {
        match &mut self.backing {
            Backing::Memory(blobs) => blobs.retain(|id, _| keep(id)),
            Backing::File { index, .. } => index.retain(|id, _| keep(id)),
        }
    }
}

/// Two iterator shapes with one static type, so [`ContentStore::ids`] can return either backing's
/// `keys()` without boxing (both yield `&BlobId` in the same sorted order; a `BTreeMap`'s `keys()` is
/// already that).
enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<'a, L, R> Iterator for Either<L, R>
where
    L: Iterator<Item = &'a BlobId>,
    R: Iterator<Item = &'a BlobId>,
{
    type Item = &'a BlobId;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Left(l) => l.next(),
            Self::Right(r) => r.next(),
        }
    }
}

/// Lowercase-hex a byte slice without a dependency (and without indexing).
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        // Writing to a String is infallible; the discarded Result carries only `fmt::Error`.
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests;

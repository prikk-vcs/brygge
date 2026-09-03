//! The content-addressed blob store (RFC 001 D-1, RFC 003 D-2/D-3).
//!
//! File bytes are stored once per [`BlobId`] (SHA-256 of the bytes) — dedup, integrity, and determinism
//! for free. Path operations in the model reference blobs by id; the store owns the bytes.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

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

/// A set of blobs keyed by content address. Iterates in sorted `BlobId` order (deterministic — `VF-1`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentStore {
    blobs: BTreeMap<BlobId, Vec<u8>>,
}

impl ContentStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert `bytes`, returning its content address. Inserting identical bytes twice is idempotent.
    pub fn insert(&mut self, bytes: Vec<u8>) -> BlobId {
        let id = BlobId::of(&bytes);
        self.blobs.entry(id).or_insert(bytes);
        id
    }

    /// The bytes for `id`, if present.
    #[must_use]
    pub fn get(&self, id: &BlobId) -> Option<&[u8]> {
        self.blobs.get(id).map(Vec::as_slice)
    }

    /// True when `id` is present.
    #[must_use]
    pub fn contains(&self, id: &BlobId) -> bool {
        self.blobs.contains_key(id)
    }

    /// The number of distinct blobs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blobs.len()
    }

    /// True when the store holds no blobs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blobs.is_empty()
    }

    /// Total content bytes across all blobs (for the fidelity report).
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.blobs.values().map(|b| b.len() as u64).sum()
    }

    /// Iterate `(id, bytes)` in sorted-id order — the canonical order for serialization and the digest.
    pub fn iter_sorted(&self) -> impl Iterator<Item = (&BlobId, &[u8])> {
        self.blobs.iter().map(|(id, bytes)| (id, bytes.as_slice()))
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

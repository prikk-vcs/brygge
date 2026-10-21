//! The byte-level plumbing for [`super::read_from`] (RFC 015 D-4): read forward through `R` exactly
//! once, feeding every byte past the stored digest into the artifact's own integrity hash as it is
//! read, while tracking an absolute byte offset so a blob's position can be recorded without ever
//! seeking backward during the pass. Mirrors [`crate::canon::CanonReader`]'s bounds checks and error
//! wording field-for-field, so a malformed artifact is refused with the same message whether it is
//! read from a slice ([`super::from_bytes`]) or streamed from here — the two must never diverge on a
//! length check, only on how (or whether) they keep a blob's bytes around afterward.
//!
//! No indexing or slicing by a runtime value anywhere here (`indexing_slicing` is denied workspace-
//! wide): a single byte is read into, and taken back out of, a fixed `[u8; 1]`; a chunk of a blob's
//! body is read into a freshly sized `Vec<u8>` each time rather than sliced out of one reused buffer.

use std::io::Read;

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// 64 KiB: large enough that a multi-gigabyte blob is hashed in a few thousand syscalls, not millions;
/// small enough that it is never itself a meaningful fraction of the "under 1 GiB" target.
const CHUNK: usize = 64 * 1024;

/// Reads `r` forward, byte by byte, tracking how many bytes have been consumed against `total` (the
/// source's full length, read via `Seek` before the pass starts) so every declared length can be
/// checked against what is actually left — the streaming equivalent of
/// [`crate::canon::CanonReader::remaining`]. Bytes read through [`Self::hashed`]/[`Self::uvarint_hashed`]
/// /[`Self::blob_body_hash`] are fed into the running digest; bytes read through [`Self::raw`] are not
/// (the magic, the format byte, and the stored digest itself sit outside the hashed region —
/// RFC 011 §2.3).
pub(super) struct SeqReader<R> {
    pub(super) r: R,
    consumed: u64,
    total: u64,
    hasher: Sha256,
}

impl<R: Read> SeqReader<R> {
    pub(super) fn new(r: R, total: u64) -> Self {
        Self {
            r,
            consumed: 0,
            total,
            hasher: Sha256::new(),
        }
    }

    /// Bytes consumed so far — the absolute offset of whatever is read next.
    pub(super) fn position(&self) -> u64 {
        self.consumed
    }

    fn remaining(&self) -> u64 {
        self.total.saturating_sub(self.consumed)
    }

    fn check_len(&self, n: u64, what: &str) -> Result<()> {
        if n > self.remaining() {
            Err(Error::Decode(format!("missing {what}")))
        } else {
            Ok(())
        }
    }

    /// Read exactly `n` bytes, outside the hashed region (the magic, the format byte, the stored
    /// digest). Never fed to the digest.
    pub(super) fn raw(&mut self, n: usize, what: &str) -> Result<Vec<u8>> {
        self.check_len(n as u64, what)?;
        let mut buf = vec![0u8; n];
        self.r.read_exact(&mut buf).map_err(Error::io)?;
        self.consumed += n as u64;
        Ok(buf)
    }

    /// Read a single byte, outside the hashed region.
    pub(super) fn raw_u8(&mut self, what: &str) -> Result<u8> {
        self.check_len(1, what)?;
        let mut buf = [0u8; 1];
        self.r.read_exact(&mut buf).map_err(Error::io)?;
        self.consumed += 1;
        let [byte] = buf;
        Ok(byte)
    }

    /// Read exactly `n` bytes, inside the hashed region. Fed to the digest as read.
    pub(super) fn hashed(&mut self, n: usize, what: &str) -> Result<Vec<u8>> {
        let buf = self.raw(n, what)?;
        self.hasher.update(&buf);
        Ok(buf)
    }

    /// Read a single byte, inside the hashed region.
    fn u8_hashed(&mut self) -> Result<u8> {
        self.check_len(1, "byte")?;
        let mut buf = [0u8; 1];
        self.r.read_exact(&mut buf).map_err(Error::io)?;
        self.hasher.update(buf);
        self.consumed += 1;
        let [byte] = buf;
        Ok(byte)
    }

    /// Read an unsigned LEB128 integer, hashed as it is read (the version, the metadata length, the
    /// blob count, each blob's length — every uvarint in the format sits inside the hashed region).
    /// Mirrors [`crate::canon::CanonReader::uvarint`]'s checks (overlong/overflow) exactly, so the two
    /// readers reject the same malformed varint with the same error.
    pub(super) fn uvarint_hashed(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        let mut nbytes: u32 = 0;
        let last_byte;
        loop {
            let byte = self.u8_hashed()?;
            nbytes += 1;
            if shift >= 64 {
                return Err(Error::Decode("varint too long".to_string()));
            }
            result |= u64::from(byte & 0x7f)
                .checked_shl(shift)
                .ok_or_else(|| Error::Decode("varint overflow".to_string()))?;
            if byte & 0x80 == 0 {
                last_byte = byte;
                break;
            }
            shift += 7;
        }
        if last_byte == 0 && nbytes > 1 {
            return Err(Error::NonCanonical("overlong varint".to_string()));
        }
        Ok(result)
    }

    /// Read a `uvarint` length, hashed, and check it against what is actually left — the streaming
    /// equivalent of [`crate::canon::CanonReader::uvarint_len`].
    pub(super) fn uvarint_len_hashed(&mut self) -> Result<u64> {
        let len = self.uvarint_hashed()?;
        if len > self.remaining() {
            return Err(Error::Decode("declared length exceeds input".to_string()));
        }
        Ok(len)
    }

    /// Stream exactly `len` bytes of a blob's body: fed to the overall artifact digest and, separately,
    /// to a per-blob hasher, in bounded chunks — a fresh, exactly-sized `Vec<u8>` per chunk rather than
    /// a slice of one reused buffer, so nothing here indexes by a runtime value. A blob's bytes are
    /// never collected into one buffer (RFC 015 D-4: at most [`CHUNK`] of any one blob is ever in
    /// memory at a time). Returns the blob's own content address.
    pub(super) fn blob_body_hash(&mut self, len: u64) -> Result<[u8; 32]> {
        let mut blob_hasher = Sha256::new();
        let mut remaining = len;
        while remaining > 0 {
            let want = usize::try_from(remaining.min(CHUNK as u64)).unwrap_or(CHUNK);
            let mut buf = vec![0u8; want];
            self.r.read_exact(&mut buf).map_err(Error::io)?;
            self.hasher.update(&buf);
            blob_hasher.update(&buf);
            self.consumed += want as u64;
            remaining -= want as u64;
        }
        Ok(blob_hasher.finalize().into())
    }

    /// After a structural error, abandon interpretation and read every remaining byte to the true end
    /// of the source, feeding each one to the digest — so the final digest still covers everything the
    /// writer covered, and a tamper that *also* breaks structure is still reported as the digest
    /// mismatch it is (matching [`super::from_bytes`]'s precedence: the digest is checked before the
    /// body is trusted at all).
    pub(super) fn drain_to_end_hashed(&mut self) -> Result<()> {
        loop {
            let want = usize::try_from(self.remaining().min(CHUNK as u64)).unwrap_or(CHUNK);
            if want == 0 {
                return Ok(());
            }
            let mut buf = vec![0u8; want];
            self.r.read_exact(&mut buf).map_err(Error::io)?;
            self.hasher.update(&buf);
            self.consumed += want as u64;
        }
    }

    /// `true` once every byte up to `total` has been consumed.
    pub(super) fn at_end(&self) -> bool {
        self.consumed == self.total
    }

    /// The running digest over every hashed byte read so far.
    pub(super) fn digest(&self) -> [u8; 32] {
        self.hasher.clone().finalize().into()
    }
}

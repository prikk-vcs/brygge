//! Compact artifacts' frame codec (RFC 018 D-1): a frame is a run of raw bytes, stored either as they
//! are (`Codec::Stored`) or DEFLATE-compressed (`Codec::Deflate`, RFC 1951, raw — no zlib header or
//! checksum). The writer picks per frame; the reader accepts either, for any frame. All three pieces
//! that need this live here: the packing plan both the writer and reader compute (`canonical_blob_frame_lens`),
//! the writer's own per-frame encode (`compress_frame`), and the reader's bounded inflate
//! (`inflate_exact`), used both by the in-memory/streaming decoders and by [`crate::content`]'s
//! artifact-backed [`crate::content::ContentStore::get`].

use crate::{Error, Result};

/// 1 MiB: metadata's fixed chunk size, and the cap on a frame packing more than one blob (RFC 018 D-1).
pub(super) const FRAME_CAP: u64 = 1 << 20;

/// A frame's compression codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Codec {
    /// The bytes as they are.
    Stored,
    /// DEFLATE, RFC 1951, raw (no zlib header or checksum).
    Deflate,
}

impl Codec {
    pub(super) fn to_u64(self) -> u64 {
        match self {
            Self::Stored => 0,
            Self::Deflate => 1,
        }
    }

    pub(super) fn from_u64(v: u64) -> Result<Self> {
        match v {
            0 => Ok(Self::Stored),
            1 => Ok(Self::Deflate),
            n => Err(Error::Decode(format!("unknown frame codec {n}"))),
        }
    }
}

/// Where one blob frame's stored bytes live in the artifact, and how to read it back
/// (RFC 018 D-4): used by [`crate::content::ContentStore`]'s `ArtifactCompact` backing, built by
/// [`crate::artifact::read_from`]/[`crate::artifact::from_file`] from the frames their verifying pass
/// already walked once.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameLoc {
    /// The file offset where this frame's *stored* bytes begin (after its codec/raw_len/stored_len
    /// header).
    pub(crate) stored_offset: u64,
    pub(crate) codec: Codec,
    pub(crate) raw_len: u64,
    pub(crate) stored_len: u64,
}

/// Compress `raw` for storage: DEFLATE at level 6, falling back to `Codec::Stored` when DEFLATE is not
/// smaller (RFC 018 D-1's writer rule). Returns the codec chosen and the bytes to write.
pub(super) fn compress_frame(raw: &[u8]) -> (Codec, Vec<u8>) {
    let deflated = miniz_oxide::deflate::compress_to_vec(raw, 6);
    if deflated.len() < raw.len() {
        (Codec::Deflate, deflated)
    } else {
        (Codec::Stored, raw.to_vec())
    }
}

/// Inflate `stored` (bounded, `codec`-aware), checked before and during inflation (RFC 018 D-1's
/// "bounds" rule): a claim that does not match these bounds never reaches an allocation it could abuse,
/// and a frame that does not inflate exactly to `raw_len` — using exactly `stored`'s bytes, no more, no
/// less — is refused before any digest is attempted (the format page's error precedence: "a frame that
/// does not inflate").
///
/// # Errors
/// [`Error::Decode`] on an unknown codec, a codec-0 length mismatch, a codec-1 claim past DEFLATE's
/// maximum expansion (1032×, `+64` for small inputs), or an inflate that over/under-produces or leaves
/// stored bytes unconsumed.
pub(super) fn inflate_exact(codec: Codec, stored: &[u8], raw_len: u64) -> Result<Vec<u8>> {
    match codec {
        Codec::Stored => {
            if stored.len() as u64 != raw_len {
                return Err(Error::Decode(format!(
                    "stored frame: raw_len {raw_len} does not match its {} stored bytes",
                    stored.len()
                )));
            }
            Ok(stored.to_vec())
        }
        Codec::Deflate => {
            let max_raw = 1032u64
                .checked_mul(stored.len() as u64)
                .and_then(|v| v.checked_add(64))
                .ok_or_else(|| {
                    Error::Decode("deflate frame: expansion bound overflow".to_string())
                })?;
            if raw_len > max_raw {
                return Err(Error::Decode(format!(
                    "deflate frame: raw_len {raw_len} exceeds DEFLATE's maximum expansion of its \
                     {} stored bytes",
                    stored.len()
                )));
            }
            let raw_len_usize = usize::try_from(raw_len)
                .map_err(|_| Error::Decode("deflate frame: raw_len too large".to_string()))?;
            let mut out = vec![0u8; raw_len_usize];
            let mut decomp = miniz_oxide::inflate::core::DecompressorOxide::new();
            let flags =
                miniz_oxide::inflate::core::inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
            let (status, in_consumed, out_consumed) =
                miniz_oxide::inflate::core::decompress(&mut decomp, stored, &mut out, 0, flags);
            if status != miniz_oxide::inflate::TINFLStatus::Done {
                return Err(Error::Decode(format!(
                    "deflate frame: failed to inflate ({status:?})"
                )));
            }
            if out_consumed != raw_len_usize {
                return Err(Error::Decode(format!(
                    "deflate frame: inflated to {out_consumed} bytes, declared {raw_len}"
                )));
            }
            if in_consumed != stored.len() {
                return Err(Error::Decode(
                    "deflate frame: stored bytes left unconsumed after inflating".to_string(),
                ));
            }
            Ok(out)
        }
    }
}

/// Inflate the frame at `loc` by seeking to it in `source` (RFC 018 D-4: [`crate::content`]'s
/// `ArtifactCompact` backing, reading a blob back on demand, after the artifact's own verifying pass).
pub(crate) fn inflate_frame<R: std::io::Read + std::io::Seek>(
    source: &mut R,
    loc: &FrameLoc,
) -> Result<Vec<u8>> {
    use std::io::SeekFrom;
    source
        .seek(SeekFrom::Start(loc.stored_offset))
        .map_err(Error::io)?;
    let stored_len = usize::try_from(loc.stored_len)
        .map_err(|_| Error::io_msg("frame stored length too large".to_string()))?;
    let mut stored = vec![0u8; stored_len];
    source.read_exact(&mut stored).map_err(Error::io)?;
    inflate_exact(loc.codec, &stored, loc.raw_len).map_err(|e| {
        Error::ArtifactChanged(format!(
            "a blob frame no longer inflates as the artifact's own verifying pass found it: {e}"
        ))
    })
}

/// The canonical packing plan for a run of raw byte lengths (RFC 018 D-1's blob-frame rule, used for
/// metadata too via fixed chunking — see [`fixed_chunk_lens`]): a length of 1 MiB or less joins the
/// current frame if the frame's running total stays at or under 1 MiB, or starts a new one; a length
/// over 1 MiB is a frame of its own. Pure function of the lengths alone, so the writer and the reader
/// (recomputing it from the blob table to check the actual on-disk frame lengths match) can never
/// diverge.
pub(super) fn canonical_frame_raw_lens(lens: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let mut frames = Vec::new();
    let mut current: Option<u64> = None;
    for len in lens {
        if len > FRAME_CAP {
            if let Some(c) = current.take() {
                frames.push(c);
            }
            frames.push(len);
            continue;
        }
        current = Some(match current {
            Some(c) if c + len <= FRAME_CAP => c + len,
            Some(c) => {
                frames.push(c);
                len
            }
            None => len,
        });
    }
    if let Some(c) = current {
        frames.push(c);
    }
    frames
}

/// The metadata section's fixed chunking (RFC 018 D-1): `total` bytes cut into frames of exactly
/// [`FRAME_CAP`], the last one shorter (never zero frames for a non-zero `total` — an empty metadata
/// record is impossible).
pub(super) fn fixed_chunk_lens(total: u64) -> Vec<u64> {
    let mut v = Vec::new();
    let mut remaining = total;
    while remaining > 0 {
        let n = remaining.min(FRAME_CAP);
        v.push(n);
        remaining -= n;
    }
    v
}

#[cfg(test)]
mod tests;

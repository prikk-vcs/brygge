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

/// The output buffer size for [`inflate_streamed`] (review 099 R-1): every inflate, whatever the
/// frame's `raw_len` claims, allocates at most this much for its own decompression buffer. 64 KiB: large
/// enough that even a multi-GB blob inflates in a few thousand calls, not millions; small enough that it
/// is never itself a meaningful fraction of a sound memory bound.
const STREAM_CHUNK: usize = 64 * 1024;

/// Check `codec`/`stored_len`/`raw_len` against DEFLATE's own bounds, **before any allocation sized by
/// `raw_len`** (review 099 R-1): codec 0 requires `stored_len == raw_len` exactly; codec 1 requires
/// `raw_len ≤ 1032 × stored_len + 64`, DEFLATE's documented maximum expansion. This is the one check
/// that stands between an untrusted `raw_len` claim and an allocation sized by it — call it before
/// reading a single stored byte of a frame whose `raw_len` has not *also* been checked against the
/// packing plan (the caller's job: see `artifact.rs`'s `read_and_inflate_frame*`).
///
/// # Errors
/// [`Error::Decode`] on an unknown codec, or a claim past either bound.
pub(super) fn check_frame_bounds(codec: Codec, stored_len: u64, raw_len: u64) -> Result<()> {
    match codec {
        Codec::Stored => {
            if stored_len != raw_len {
                return Err(Error::Decode(format!(
                    "stored frame: raw_len {raw_len} does not match its {stored_len} stored bytes"
                )));
            }
        }
        Codec::Deflate => {
            let max_raw = 1032u64
                .checked_mul(stored_len)
                .and_then(|v| v.checked_add(64))
                .ok_or_else(|| {
                    Error::Decode("deflate frame: expansion bound overflow".to_string())
                })?;
            if raw_len > max_raw {
                return Err(Error::Decode(format!(
                    "deflate frame: raw_len {raw_len} exceeds DEFLATE's maximum expansion of its \
                     {stored_len} stored bytes"
                )));
            }
        }
    }
    Ok(())
}

/// Inflate `stored` (`codec`-aware, bounds already checked by the caller via
/// [`check_frame_bounds`] — this function trusts `raw_len` only to the extent that bound allows,
/// never allocating more than [`STREAM_CHUNK`] for its own output buffer regardless of what `raw_len`
/// claims), feeding each successive chunk of the frame's raw (decompressed) bytes to `sink`, in order.
/// `sink` decides what becomes of a chunk — hashed and discarded (the verifying pass, RFC 015 D-4's
/// bound carried over compression), or appended to a growing buffer ([`crate::content`]'s `get`, where
/// `raw_len` is a value a prior pass already proved, not fresh untrusted input).
///
/// A frame that does not inflate exactly to `raw_len` — using exactly `stored`'s bytes, no more, no
/// less — is refused before any digest is attempted (the format page's error precedence: "a frame that
/// does not inflate").
///
/// # Errors
/// [`Error::Decode`] on a codec-1 stream that fails to inflate, over/under-produces relative to
/// `raw_len`, or leaves stored bytes unconsumed; whatever `sink` itself returns.
pub(super) fn inflate_streamed(
    codec: Codec,
    stored: &[u8],
    raw_len: u64,
    mut sink: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    match codec {
        Codec::Stored => {
            // `check_frame_bounds` already confirmed `stored.len() as u64 == raw_len`.
            for chunk in stored.chunks(STREAM_CHUNK) {
                sink(chunk)?;
            }
            Ok(())
        }
        Codec::Deflate => inflate_deflate_streamed(stored, raw_len, &mut sink),
    }
}

/// [`inflate_streamed`]'s `Codec::Deflate` path: `miniz_oxide::inflate::stream::inflate`, called
/// repeatedly with a fixed [`STREAM_CHUNK`]-sized output buffer and `MZFlush::None` (never `Finish`,
/// which only `InflateState`'s *first* call may safely use, and only when the whole output fits in one
/// buffer — exactly what a fixed small buffer cannot promise). `MZFlush::None` keeps the 32 KiB window
/// `InflateState` itself owns, so a back-reference into already-flushed output still resolves correctly
/// across calls — no re-implementation of DEFLATE's own windowing here, just repeated calls into it.
fn inflate_deflate_streamed(
    stored: &[u8],
    raw_len: u64,
    sink: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    use miniz_oxide::inflate::stream::{InflateState, inflate as mz_inflate};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};

    let mut state = InflateState::new(DataFormat::Raw);
    let mut out = [0u8; STREAM_CHUNK];
    let mut input = stored;
    let mut produced: u64 = 0;
    // Each call consumes input, produces output, or reaches `StreamEnd`/an error (`inflate_loop`'s own
    // contract) — bounded by `stored.len() + 1` calls at worst, generously (one call can consume more
    // than one byte); the guard exists only so a future miniz_oxide behavior change fails loudly here
    // rather than hanging.
    let guard_max = stored.len().saturating_add(2);
    for _ in 0..guard_max {
        let res = mz_inflate(&mut state, input, &mut out, MZFlush::None);
        input = input.get(res.bytes_consumed..).ok_or_else(|| {
            Error::Decode("deflate frame: consumed past its own input".to_string())
        })?;
        if res.bytes_written > 0 {
            produced = produced
                .checked_add(res.bytes_written as u64)
                .ok_or_else(|| {
                    Error::Decode("deflate frame: output counter overflow".to_string())
                })?;
            if produced > raw_len {
                return Err(Error::Decode(format!(
                    "deflate frame: inflated past its declared raw_len {raw_len}"
                )));
            }
            sink(out.get(..res.bytes_written).ok_or_else(|| {
                Error::Decode("deflate frame: output counter exceeds its own buffer".to_string())
            })?)?;
        }
        match res.status {
            Ok(MZStatus::StreamEnd) => {
                if produced != raw_len {
                    return Err(Error::Decode(format!(
                        "deflate frame: inflated to {produced} bytes, declared {raw_len}"
                    )));
                }
                if !input.is_empty() {
                    return Err(Error::Decode(
                        "deflate frame: stored bytes left unconsumed after inflating".to_string(),
                    ));
                }
                return Ok(());
            }
            Ok(MZStatus::Ok) => continue,
            Ok(MZStatus::NeedDict) | Err(_) => {
                return Err(Error::Decode(format!(
                    "deflate frame: failed to inflate ({:?})",
                    res.status
                )));
            }
        }
    }
    Err(Error::Decode(
        "deflate frame: did not finish inflating within a bounded number of calls".to_string(),
    ))
}

/// Inflate the frame at `loc` by seeking to it in `source` (RFC 018 D-4: [`crate::content`]'s
/// `ArtifactCompact` backing, reading a blob back on demand, after the artifact's own verifying pass).
/// `loc.raw_len` is a value that pass already proved (not fresh untrusted input), so allocating a
/// buffer of exactly that size here, up front, is sound — review 099 R-1's bound is about the *first,
/// untrusted* pass (`artifact.rs`'s `read_from`/`from_bytes`), not this on-demand re-read of a frame a
/// prior pass already verified.
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
    let raw_len_usize = usize::try_from(loc.raw_len)
        .map_err(|_| Error::io_msg("frame raw length too large".to_string()))?;
    let mut out = Vec::with_capacity(raw_len_usize);
    inflate_streamed(loc.codec, &stored, loc.raw_len, |chunk| {
        out.extend_from_slice(chunk);
        Ok(())
    })
    .map_err(|e| {
        Error::ArtifactChanged(format!(
            "a blob frame no longer inflates as the artifact's own verifying pass found it: {e}"
        ))
    })?;
    Ok(out)
}

/// The canonical packing plan for a run of raw byte lengths (RFC 018 D-1's blob-frame rule, used for
/// metadata too via fixed chunking — see [`fixed_chunk_lens`]): a length of 1 MiB or less joins the
/// current frame if the frame's running total stays at or under 1 MiB, or starts a new one; a length
/// over 1 MiB is a frame of its own. Pure function of the lengths alone, so the writer and the reader
/// (recomputing it from the blob table to check the actual on-disk frame lengths match) can never
/// diverge.
pub(super) fn canonical_frame_raw_lens(lens: impl IntoIterator<Item = u64>) -> Vec<u64> {
    canonical_frame_plan(&lens.into_iter().collect::<Vec<_>>())
        .into_iter()
        .map(|(len, _)| len)
        .collect()
}

/// As [`canonical_frame_raw_lens`], but each frame's raw length is paired with the half-open range of
/// `lens`' own indices it covers (review 099 R-1: a caller needs this to know, index by index, which
/// blob(s) a frame's raw bytes belong to — and so where that frame's `raw_len` must land, before it is
/// read, to check it against the plan *before inflating*, not after).
pub(super) fn canonical_frame_plan(lens: &[u64]) -> Vec<(u64, std::ops::Range<usize>)> {
    let mut frames = Vec::new();
    let mut current: Option<(u64, usize)> = None;
    for (i, &len) in lens.iter().enumerate() {
        if len > FRAME_CAP {
            if let Some((c, start)) = current.take() {
                frames.push((c, start..i));
            }
            frames.push((len, i..i + 1));
            continue;
        }
        current = Some(match current {
            Some((c, start)) if c + len <= FRAME_CAP => (c + len, start),
            Some((c, start)) => {
                frames.push((c, start..i));
                (len, i)
            }
            None => (len, i),
        });
    }
    if let Some((c, start)) = current {
        frames.push((c, start..lens.len()));
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

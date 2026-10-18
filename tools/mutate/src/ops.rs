//! Mutation operators (RFC 014 D-1): generic byte-level operators that need no knowledge of the format,
//! and format-aware operators that write a boundary value at a position a parser module declares
//! "interesting" (a length, a count, a varint, a window size, a revision number). The format-aware
//! operators are what finds RUSTSEC-2026-0344's class — random bytes alone very rarely land on an exact
//! ceiling value — so every parser module supplying [`InterestingPosition`]s is load-bearing, not optional.

use crate::rng::SplitMix64;

/// One byte-wide-and-up position in a seed's bytes that a parser module considers worth probing with
/// boundary values: the offset of a length, a count, a varint, a window size, a revision number, or
/// similar — anywhere a crafted value is more interesting than a crafted sea of random bytes.
#[derive(Debug, Clone, Copy)]
pub struct InterestingPosition {
    /// The byte offset in the original seed where the value starts.
    pub offset: usize,
    /// How the value is encoded at that offset.
    pub encoding: Encoding,
    /// The value the parser is documented to refuse at or above (its declared ceiling), when the module
    /// knows one. `None` still exercises zero/one/max; it just skips the ceiling-relative values.
    pub ceiling: Option<u64>,
}

/// How a probed value is written into the byte stream.
#[derive(Debug, Clone, Copy)]
pub enum Encoding {
    /// A fixed-width binary integer, this many bytes (1, 2, 4 or 8), big- or little-endian. Overwritten
    /// in place: the buffer's length never changes.
    Binary {
        /// How many bytes wide the integer is (1, 2, 4 or 8).
        width: u8,
        /// Big-endian when `true`, little-endian when `false`.
        big_endian: bool,
    },
    /// A run of ASCII decimal digits spanning exactly `len` bytes of the original seed (no sign, no
    /// leading zero other than a lone `0`). The replacement may need more or fewer digits than `len`; the
    /// span is spliced out and the new digits spliced in, so the buffer's length can change.
    Decimal {
        /// How many bytes the original decimal span occupies.
        len: usize,
    },
    /// A minimal unsigned LEB128 varint (7 value bits per byte, high bit set on every byte but the
    /// last — brygge-ir's canon format, and common to many binary formats) spanning exactly `len` bytes
    /// of the original seed. As `Decimal`, the span is spliced out and the new varint spliced in.
    Uvarint {
        /// How many bytes the original varint occupies.
        len: usize,
    },
}

/// A boundary value to probe at an [`InterestingPosition`].
#[derive(Debug, Clone, Copy)]
pub enum Boundary {
    /// `0`.
    Zero,
    /// `1`.
    One,
    /// The maximum value the encoding can hold (binary: `2^(8*width) - 1`; decimal/uvarint: `u64::MAX`).
    Max,
    /// `ceiling - 1`; skipped when there is no ceiling or `ceiling == 0`.
    CeilingMinus1,
    /// `ceiling` itself — the first value the parser should refuse.
    Ceiling,
    /// `ceiling + 1`; skipped when there is no ceiling.
    CeilingPlus1,
    /// Near `u32::MAX`: overflows a 32-bit multiply or add by almost any non-trivial second operand.
    Overflow32,
    /// Near `u64::MAX`: as `Overflow32`, for 64-bit arithmetic.
    Overflow64,
}

const ALL_BOUNDARIES_NO_CEILING: &[Boundary] = &[
    Boundary::Zero,
    Boundary::One,
    Boundary::Max,
    Boundary::Overflow32,
    Boundary::Overflow64,
];
const ALL_BOUNDARIES_WITH_CEILING: &[Boundary] = &[
    Boundary::Zero,
    Boundary::One,
    Boundary::Max,
    Boundary::CeilingMinus1,
    Boundary::Ceiling,
    Boundary::CeilingPlus1,
    Boundary::Overflow32,
    Boundary::Overflow64,
];

/// Every boundary value worth probing at `pos` (depends only on whether it carries a ceiling).
#[must_use]
pub fn boundaries_for(pos: &InterestingPosition) -> &'static [Boundary] {
    if pos.ceiling.is_some() {
        ALL_BOUNDARIES_WITH_CEILING
    } else {
        ALL_BOUNDARIES_NO_CEILING
    }
}

fn resolve(boundary: Boundary, ceiling: Option<u64>, max_for_width: u64) -> u64 {
    match boundary {
        Boundary::Zero => 0,
        Boundary::One => 1,
        Boundary::Max => max_for_width,
        Boundary::CeilingMinus1 => ceiling.unwrap_or(0).saturating_sub(1),
        Boundary::Ceiling => ceiling.unwrap_or(0),
        Boundary::CeilingPlus1 => ceiling.unwrap_or(0).saturating_add(1).min(max_for_width),
        Boundary::Overflow32 => u64::from(u32::MAX).min(max_for_width),
        Boundary::Overflow64 => max_for_width,
    }
}

/// Write `boundary` at `pos` in `data`, returning the mutated copy. A position past the end of `data`, or
/// one whose declared span does not fit, leaves `data` unchanged (the caller supplied stale positions for
/// a different seed — not a bug worth panicking over in a fuzz loop).
#[must_use]
pub fn apply_boundary(data: &[u8], pos: &InterestingPosition, boundary: Boundary) -> Vec<u8> {
    match pos.encoding {
        Encoding::Binary { width, big_endian } => {
            let w = usize::from(width);
            if data.get(pos.offset..pos.offset + w).is_none() {
                return data.to_vec();
            }
            let max_for_width = if w >= 8 {
                u64::MAX
            } else {
                (1u64 << (8 * w)) - 1
            };
            let value = resolve(boundary, pos.ceiling, max_for_width);
            let be = value.to_be_bytes();
            let be_tail = be.get(8 - w..).unwrap_or(&[]);
            let mut out = data.to_vec();
            let Some(slot) = out.get_mut(pos.offset..pos.offset + w) else {
                return data.to_vec();
            };
            if big_endian {
                slot.copy_from_slice(be_tail);
            } else {
                let mut le = be_tail.to_vec();
                le.reverse();
                slot.copy_from_slice(&le);
            }
            out
        }
        Encoding::Decimal { len } => {
            if data.get(pos.offset..pos.offset + len).is_none() {
                return data.to_vec();
            }
            let value = resolve(boundary, pos.ceiling, u64::MAX);
            let digits = value.to_string();
            splice(data, pos.offset, len, digits.as_bytes())
        }
        Encoding::Uvarint { len } => {
            if data.get(pos.offset..pos.offset + len).is_none() {
                return data.to_vec();
            }
            let value = resolve(boundary, pos.ceiling, u64::MAX);
            splice(data, pos.offset, len, &encode_uvarint(value))
        }
    }
}

/// Minimal unsigned LEB128, matching `brygge-ir`'s `CanonWriter::uvarint`.
fn encode_uvarint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out
}

/// Replace `data[offset..offset+len]` with `replacement`, growing or shrinking as needed.
fn splice(data: &[u8], offset: usize, len: usize, replacement: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() - len + replacement.len());
    out.extend_from_slice(data.get(..offset).unwrap_or(&[]));
    out.extend_from_slice(replacement);
    out.extend_from_slice(data.get(offset + len..).unwrap_or(&[]));
    out
}

/// A generic, format-blind operator applied once to `data`, chosen and parameterized by `rng`. Returns
/// the mutated copy and a short, human-readable name for reporting.
#[must_use]
pub fn apply_generic(data: &[u8], other_seed: &[u8], rng: &mut SplitMix64) -> (Vec<u8>, String) {
    if data.is_empty() {
        return (Vec::new(), "empty-seed-noop".to_string());
    }
    match rng.below(7) {
        0 => {
            let mut out = data.to_vec();
            let i = rng.below(out.len());
            let bit = 1u8 << rng.below(8);
            if let Some(b) = out.get_mut(i) {
                *b ^= bit;
            }
            (out, format!("flip_bit@{i}"))
        }
        1 => {
            let mut out = data.to_vec();
            let i = rng.below(out.len());
            if let Some(b) = out.get_mut(i) {
                *b = 0;
            }
            (out, format!("set_zero@{i}"))
        }
        2 => {
            let mut out = data.to_vec();
            let i = rng.below(out.len());
            if let Some(b) = out.get_mut(i) {
                *b = 0xFF;
            }
            (out, format!("set_max@{i}"))
        }
        3 => {
            let mut out = data.to_vec();
            let i = rng.below(out.len());
            let v = rng.next_byte();
            if let Some(b) = out.get_mut(i) {
                *b = v;
            }
            (out, format!("set_random@{i}"))
        }
        4 => {
            let i = rng.below(data.len());
            let run_len = 1 + rng.below(16.min(data.len().max(1)));
            let mut out = Vec::with_capacity(data.len() + run_len);
            out.extend_from_slice(data.get(..i).unwrap_or(&[]));
            for _ in 0..run_len {
                out.push(rng.next_byte());
            }
            out.extend_from_slice(data.get(i..).unwrap_or(&[]));
            (out, format!("insert_run@{i}+{run_len}"))
        }
        5 => {
            let i = rng.below(data.len());
            let run_len = 1 + rng.below(16.min(data.len() - i));
            let mut out = Vec::with_capacity(data.len());
            out.extend_from_slice(data.get(..i).unwrap_or(&[]));
            out.extend_from_slice(data.get(i + run_len..).unwrap_or(&[]));
            (out, format!("delete_run@{i}-{run_len}"))
        }
        _ => {
            if rng.one_in(2) || other_seed.is_empty() {
                let i = rng.below(data.len());
                let run_len = 1 + rng.below(16.min(data.len() - i).max(1));
                let run = data.get(i..i + run_len).unwrap_or(&[]).to_vec();
                let mut out = data.to_vec();
                let at = rng.below(out.len() + 1);
                out.splice(at..at, run.iter().copied());
                (out, format!("duplicate_run@{i}+{run_len}->{at}"))
            } else {
                let cut_a = rng.below(data.len() + 1);
                let cut_b = rng.below(other_seed.len() + 1);
                let mut out = Vec::with_capacity(cut_a + (other_seed.len() - cut_b));
                out.extend_from_slice(data.get(..cut_a).unwrap_or(&[]));
                out.extend_from_slice(other_seed.get(cut_b..).unwrap_or(&[]));
                (out, format!("splice@{cut_a}+{cut_b}"))
            }
        }
    }
}

/// Truncate `data` to a pseudo-random shorter length (including zero).
#[must_use]
pub fn apply_truncate(data: &[u8], rng: &mut SplitMix64) -> (Vec<u8>, String) {
    if data.is_empty() {
        return (Vec::new(), "truncate-noop".to_string());
    }
    let n = rng.below(data.len());
    (
        data.get(..n).unwrap_or(&[]).to_vec(),
        format!("truncate@{n}"),
    )
}

#[cfg(test)]
mod tests;

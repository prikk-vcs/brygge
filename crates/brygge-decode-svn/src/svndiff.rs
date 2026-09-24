//! The svndiff (version 0) reader (RFC 013 D-2): a delta and a base in, the text or a typed error out. It
//! knows nothing of dumps.
//!
//! svndiff is what Subversion writes for a `Text-delta: true` node (`svnadmin dump --deltas`, and every
//! `svnrdump dump`): the header `SVN` and a version byte, then **windows**. Each window carries five
//! base-128 integers (source view offset and length, target view length, instruction section length,
//! new-data section length), then the instructions, then the new data. An instruction is one byte (the top two
//! bits the opcode: copy from the source view, copy from the target built so far, new data; `11` is invalid;
//! the low six bits the length, `0` meaning a length integer follows), then, for the two copies, an offset
//! integer. A target copy reads what the window has already produced, byte by byte, and may overlap what it is
//! writing (that is how a run is encoded).
//!
//! **Version 1 (zlib) and version 2 (lz4)** compress the two sections; they are refused by name
//! ([`DiffError::UnsupportedVersion`]), as OQ-4 of RFC 013 rules. `svnadmin dump --deltas` and `svnrdump`
//! write version 0.
//!
//! The delta is untrusted, so every length and offset is checked before it is used (checked arithmetic, `get`
//! rather than indexing), and nothing is allocated for a length that has not passed the ceiling: the output
//! grows only by bytes an instruction really produces, and the sum of the windows' target lengths is checked
//! against `max_target` **before** any of a window's instructions run. The result is a typed error, never a
//! panic.

/// Why a delta could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffError {
    /// The header names a version this build does not read: `1` (zlib) or `2` (lz4).
    UnsupportedVersion(u8),
    /// The delta is malformed (truncated, an out-of-range copy, an invalid opcode, a length that does not add
    /// up, ...). The message says which.
    Malformed(String),
    /// The target would exceed the ceiling given to [`apply`].
    TooLarge {
        /// The ceiling, in bytes.
        limit: usize,
    },
}

fn bad(what: &str) -> DiffError {
    DiffError::Malformed(what.to_string())
}

/// The svndiff header length: `SVN` and the version byte.
const HEADER_LEN: usize = 4;

/// A base-128 integer is at most this many bytes for a `u64` (`ceil(64 / 7)`).
const MAX_VARINT_BYTES: usize = 10;

/// A bounds-checked cursor over the delta.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn byte(&mut self) -> Result<u8, DiffError> {
        let b = *self.data.get(self.pos).ok_or_else(|| bad("truncated"))?;
        self.pos += 1;
        Ok(b)
    }

    /// A big-endian base-128 integer: seven bits a byte, the high bit meaning "more".
    fn varint(&mut self) -> Result<u64, DiffError> {
        let mut value: u64 = 0;
        for _ in 0..MAX_VARINT_BYTES {
            let b = self.byte()?;
            value = value
                .checked_mul(128)
                .and_then(|v| v.checked_add(u64::from(b & 0x7f)))
                .ok_or_else(|| bad("an integer does not fit 64 bits"))?;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(bad("an integer is longer than 64 bits allow"))
    }

    /// A varint as a `usize` (where it indexes or sizes something).
    fn size(&mut self) -> Result<usize, DiffError> {
        usize::try_from(self.varint()?)
            .map_err(|_| bad("an integer does not fit this platform's size"))
    }

    /// The next `n` bytes, or "truncated" when fewer remain (no allocation).
    fn take(&mut self, n: usize) -> Result<&'a [u8], DiffError> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("truncated"))?;
        let s = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| bad("truncated"))?;
        self.pos = end;
        Ok(s)
    }
}

/// Check the header alone: `SVN` and a version byte. Version 1 and 2 are [`DiffError::UnsupportedVersion`];
/// any other version, a wrong magic, or a body shorter than the header, is [`DiffError::Malformed`].
///
/// # Errors
/// As above.
pub fn check_header(delta: &[u8]) -> Result<(), DiffError> {
    let Some(header) = delta.get(..HEADER_LEN) else {
        return Err(bad("shorter than the svndiff header"));
    };
    if header.get(..3) != Some(b"SVN") {
        return Err(bad("not svndiff (the header is not `SVN`)"));
    }
    match header.get(3) {
        Some(0) => Ok(()),
        Some(&v @ (1 | 2)) => Err(DiffError::UnsupportedVersion(v)),
        Some(_) => Err(bad("an unknown svndiff version")),
        None => Err(bad("shorter than the svndiff header")),
    }
}

/// Apply `delta` (svndiff version 0) to `base`, giving the target text.
///
/// `max_target` is the ceiling on the target's length: a window whose declared target length would take the
/// running total past it is [`DiffError::TooLarge`], **before** any of its instructions run.
///
/// # Errors
/// [`DiffError::UnsupportedVersion`], [`DiffError::Malformed`] or [`DiffError::TooLarge`].
pub fn apply(delta: &[u8], base: &[u8], max_target: usize) -> Result<Vec<u8>, DiffError> {
    check_header(delta)?;
    let mut cur = Cursor::new(delta);
    cur.take(HEADER_LEN)?;
    let mut out: Vec<u8> = Vec::new();

    while !cur.at_end() {
        let source_offset = cur.size()?;
        let source_len = cur.size()?;
        let target_len = cur.size()?;
        let instr_len = cur.size()?;
        let new_len = cur.size()?;

        let source_end = source_offset
            .checked_add(source_len)
            .ok_or_else(|| bad("the source view overflows"))?;
        let source = base
            .get(source_offset..source_end)
            .ok_or_else(|| bad("the source view is outside the base"))?;

        // The ceiling first: nothing of this window is produced for a target that is already too large.
        let total = out
            .len()
            .checked_add(target_len)
            .ok_or(DiffError::TooLarge { limit: max_target })?;
        if total > max_target {
            return Err(DiffError::TooLarge { limit: max_target });
        }

        let instructions = cur.take(instr_len)?;
        let new_data = cur.take(new_len)?;
        run_window(&mut out, source, target_len, instructions, new_data)?;
    }
    Ok(out)
}

/// Run one window's instructions, appending exactly `target_len` bytes to `out`.
fn run_window(
    out: &mut Vec<u8>,
    source: &[u8],
    target_len: usize,
    instructions: &[u8],
    new_data: &[u8],
) -> Result<(), DiffError> {
    let start = out.len();
    let mut ins = Cursor::new(instructions);
    let mut new = Cursor::new(new_data);
    while !ins.at_end() {
        let op = ins.byte()?;
        let short = usize::from(op & 0x3f);
        let len = if short == 0 { ins.size()? } else { short };
        let produced = out.len() - start;
        if len > target_len - produced {
            return Err(bad("the instructions run past the target view"));
        }
        match op >> 6 {
            0 => {
                let offset = ins.size()?;
                let end = offset
                    .checked_add(len)
                    .ok_or_else(|| bad("a source copy overflows"))?;
                let piece = source
                    .get(offset..end)
                    .ok_or_else(|| bad("a source copy is outside the source view"))?;
                out.extend_from_slice(piece);
            }
            1 => {
                let offset = ins.size()?;
                if offset >= produced {
                    return Err(bad("a target copy starts at or after the current position"));
                }
                // Byte by byte: the copy may overlap what it writes (a run).
                for i in 0..len {
                    let b = *out
                        .get(start + offset + i)
                        .ok_or_else(|| bad("a target copy reads past what is built"))?;
                    out.push(b);
                }
            }
            2 => out.extend_from_slice(new.take(len)?),
            _ => return Err(bad("an invalid instruction (opcode 11)")),
        }
    }
    if out.len() - start != target_len {
        return Err(bad("the instructions do not fill the target view"));
    }
    if !new.at_end() {
        return Err(bad("the new-data section is not fully used"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

//! Small shared helpers.

use crate::Error;

fn hexval(b: u8) -> Result<u8, Error> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(Error::Read(format!("invalid hex digit {b:#x}"))),
    }
}

/// Parse a 40-character hex string into a 20-byte node id.
///
/// # Errors
/// [`Error::Read`] if the input is not exactly 40 hex characters.
pub(crate) fn parse_hex20(s: &str) -> Result<[u8; 20], Error> {
    let bytes = s.as_bytes();
    if bytes.len() != 40 {
        return Err(Error::Read(format!(
            "expected a 40-character hex node, got {} characters",
            bytes.len()
        )));
    }
    let mut out = [0u8; 20];
    for (slot, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
        if let [hi, lo] = pair {
            *slot = (hexval(*hi)? << 4) | hexval(*lo)?;
        }
    }
    Ok(out)
}

/// Render bytes for an error/refusal message losslessly: printable ASCII as-is, everything else as
/// `\xNN` — never a lossy UTF-8 conversion of untrusted data (review 009 R-2/R-4 parity with the other
/// decoders' non-UTF-8 path handling).
pub(crate) fn escape_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_graphic() || b == b' ' {
            out.push(b as char);
        } else {
            let _ = write!(out, "\\x{b:02x}");
        }
    }
    out
}

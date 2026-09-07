//! Parse a Mercurial manifest revision's text into `path → (filenode, mode)` (RFC 005 D-2).
//!
//! Format (verified against `hg debugdata -m`): one entry per line, `path\0<40-hex-filenode><flag?>\n`,
//! sorted by path. The optional flag is `x` (executable) or `l` (symlink); absent means a regular file.

use std::collections::BTreeMap;

use crate::Error;
use crate::util::parse_hex20;

/// A file's mode, mapped to the IR's Unix-style mode (matching the Git decoder's mapping).
fn mode_for_flag(flag: Option<u8>) -> Result<u32, Error> {
    match flag {
        None => Ok(0o100_644),
        Some(b'x') => Ok(0o100_755),
        Some(b'l') => Ok(0o120_000),
        Some(other) => Err(Error::Read(format!("unknown manifest flag {other:#x}"))),
    }
}

/// One manifest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The file's node in its filelog.
    pub filenode: [u8; 20],
    /// The IR mode (regular / executable / symlink).
    pub mode: u32,
}

/// Parse manifest text into a sorted `path → entry` map.
///
/// # Errors
/// [`Error::Read`] on a malformed line (missing `\0`, bad hex node, or unknown flag).
pub fn parse(text: &[u8]) -> Result<BTreeMap<String, Entry>, Error> {
    let mut out = BTreeMap::new();
    for line in text.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let sep = line
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::Read("manifest line has no NUL separator".to_string()))?;
        let path_bytes = line.get(..sep).unwrap_or(&[]);
        let rest = line.get(sep + 1..).unwrap_or(&[]);
        let path = std::str::from_utf8(path_bytes)
            .map_err(|_| Error::Read("manifest path is not valid UTF-8".to_string()))?
            .to_string();
        // rest is 40 hex chars, optionally followed by a single flag byte.
        let (node_hex, flag) = match rest.len() {
            40 => (rest, None),
            41 => (rest.get(..40).unwrap_or(&[]), rest.get(40).copied()),
            other => {
                return Err(Error::Read(format!(
                    "manifest entry for {path:?} has {other}-byte node field"
                )));
            }
        };
        let node_hex = std::str::from_utf8(node_hex)
            .map_err(|_| Error::Read("manifest node is not valid UTF-8".to_string()))?;
        out.insert(
            path,
            Entry {
                filenode: parse_hex20(node_hex)?,
                mode: mode_for_flag(flag)?,
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

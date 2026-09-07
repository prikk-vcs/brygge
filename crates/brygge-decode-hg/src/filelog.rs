//! Read a file revision from a filelog: its content, and any **source-recorded rename/copy** (RFC 005
//! D-3, SRC-H2). A filelog revision's text is an optional metadata header — `\x01\n key: value …\x01\n` —
//! followed by the file content. The header carries `copy` (the rename/copy source path) and `copyrev`.
//! A rename recorded here is a **stated fact** brygge carries as `Stated`, not a guess.

use crate::Error;
use crate::revlog::Revlog;

const META_MARK: &[u8; 2] = b"\x01\n";

/// One file revision: its raw content and, if the source recorded one, the copy/rename source path.
#[derive(Debug, Clone)]
pub struct FileRev {
    /// The file content (metadata header stripped).
    pub content: Vec<u8>,
    /// The path this file was copied/renamed from, if the source recorded it (`Stated`).
    pub copy_from: Option<String>,
}

/// Find the revision in `rl` whose node equals `filenode`.
#[must_use]
pub fn find_rev(rl: &Revlog, filenode: &[u8; 20]) -> Option<usize> {
    (0..rl.len()).find(|&rev| rl.entry(rev).is_some_and(|e| &e.node == filenode))
}

/// Read the file revision identified by `filenode` from filelog `rl`.
///
/// # Errors
/// [`Error::Read`] if the node is not in this filelog, the revision cannot be reconstructed, or the
/// metadata header is malformed.
pub fn read_revision(rl: &Revlog, filenode: &[u8; 20]) -> Result<FileRev, Error> {
    let rev = find_rev(rl, filenode)
        .ok_or_else(|| Error::Read("filenode not found in its filelog".to_string()))?;
    let text = rl.revision(rev)?;
    split_metadata(text)
}

/// Split a filelog revision text into its content and copy source.
fn split_metadata(text: Vec<u8>) -> Result<FileRev, Error> {
    if !text.starts_with(META_MARK) {
        return Ok(FileRev {
            content: text,
            copy_from: None,
        });
    }
    let rest = text.get(2..).unwrap_or(&[]);
    let end = rest
        .windows(2)
        .position(|w| w == META_MARK)
        .ok_or_else(|| Error::Read("filelog metadata header is not terminated".to_string()))?;
    let meta = rest.get(..end).unwrap_or(&[]);
    let content = rest.get(end + 2..).unwrap_or(&[]).to_vec();

    let mut copy_from = None;
    for line in meta.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix(b"copy: ") {
            copy_from = Some(
                std::str::from_utf8(value)
                    .map_err(|_| Error::Read("filelog copy path is not valid UTF-8".to_string()))?
                    .to_string(),
            );
        }
    }
    Ok(FileRev { content, copy_from })
}

#[cfg(test)]
mod tests;

//! Parse a Mercurial changelog revision's text into changeset metadata (RFC 005 D-2).
//!
//! Format (verified against `hg debugdata -c`):
//! ```text
//! <manifest-node-hex>\n <user>\n <time> <tz>[ <extras>]\n <file>\n...\n \n <description>
//! ```
//! The header runs up to the first blank line; the description is everything after it (it may itself
//! contain blank lines). `extras` is `\0`-separated `key:value` pairs (escaped), and carries `branch`.

use crate::Error;
use crate::util::parse_hex20;

/// The parsed metadata of one changeset.
#[derive(Debug, Clone)]
pub struct Changeset {
    /// The manifest node this changeset points at.
    pub manifest_node: [u8; 20],
    /// The committer's freeform user string (`"Name <email>"`).
    pub user: String,
    /// The commit time (source epoch seconds).
    pub time: i64,
    /// The named branch (`"default"` when unset).
    pub branch: String,
    /// The commit description/message.
    pub description: String,
}

/// Parse changeset text.
///
/// # Errors
/// [`Error::Read`] if the header is malformed (missing manifest node, user, or date line).
pub fn parse(text: &[u8]) -> Result<Changeset, Error> {
    let text = std::str::from_utf8(text)
        .map_err(|_| Error::Read("changelog entry is not valid UTF-8".to_string()))?;
    let (header, description) = match text.split_once("\n\n") {
        Some((h, d)) => (h, d.to_string()),
        None => (text, String::new()),
    };
    let mut lines = header.split('\n');
    let manifest_hex = lines
        .next()
        .ok_or_else(|| Error::Read("changelog entry missing manifest line".to_string()))?;
    let manifest_node = parse_hex20(manifest_hex)?;
    let user = lines
        .next()
        .ok_or_else(|| Error::Read("changelog entry missing user line".to_string()))?
        .to_string();
    let date_line = lines
        .next()
        .ok_or_else(|| Error::Read("changelog entry missing date line".to_string()))?;
    // remaining `lines` are the changed-file list; we derive ops from manifest diffs instead.

    let mut date_parts = date_line.splitn(3, ' ');
    let time: i64 = date_parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Read(format!("bad changelog time in {date_line:?}")))?;
    let _tz = date_parts.next(); // offset; times are stored as epoch seconds
    let branch = date_parts
        .next()
        .map(parse_extras_branch)
        .unwrap_or_else(|| "default".to_string());

    Ok(Changeset {
        manifest_node,
        user,
        time,
        branch,
        description,
    })
}

/// Extract the `branch` value from the encoded extras blob (`\0`-separated `key:value`).
fn parse_extras_branch(extras: &str) -> String {
    for entry in extras.split('\0') {
        if let Some(value) = entry.strip_prefix("branch:") {
            return unescape_extra(value);
        }
    }
    "default".to_string()
}

/// Undo Mercurial's extras escaping (`\\` `\n` `\r` `\0`).
fn unescape_extra(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests;

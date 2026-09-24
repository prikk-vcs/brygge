//! Parse a Mercurial changelog revision's text into changeset metadata (RFC 005 D-2, byte-exact text
//! per RFC 011 §5).
//!
//! Format (verified against `hg debugdata -c`):
//! ```text
//! <manifest-node-hex>\n <user>\n <time> <tz>[ <extras>]\n <file>\n...\n \n <description>
//! ```
//! The header runs up to the first blank line; the description is everything after it (it may itself
//! contain blank lines). `extras` is `\0`-separated, each entry an *escaped* `key:value` pair (review
//! 009 R-2), and always carries `branch`.
//!
//! Only the manifest-node-hex and the time/timezone tokens of the date line are structurally
//! fixed-format and must be ASCII; the user string, extras, and the description are carried as raw
//! bytes — no lossy UTF-8 conversion is applied to any of them here. Extras keys/values are decoded
//! from Mercurial's own escaping (`changelog.py`'s `decodeextra`/`_string_unescape`), but this reader is
//! **stricter** than Mercurial's: an escape, a duplicate key, or a key:value split that `hg` itself
//! could never have written is refused (`Error::Read`), never guessed.

use crate::Error;
use crate::util::parse_hex20;

/// One changelog `extras` entry: key → value, both raw bytes (review 009 R-2 — see the module doc).
type Extras = Vec<(Vec<u8>, Vec<u8>)>;

/// The parsed metadata of one changeset.
#[derive(Debug, Clone)]
pub struct Changeset {
    /// The manifest node this changeset points at.
    pub manifest_node: [u8; 20],
    /// The committer's freeform user string (`"Name <email>"`), as the source's raw bytes.
    pub user: Vec<u8>,
    /// The commit time (source epoch seconds).
    pub time: i64,
    /// The stored timezone offset, in seconds **west** of UTC (hg's own convention; negative means
    /// east). Corrections handoff §3: `offset_minutes = -(tz / 60)`, or absent if that does not divide
    /// evenly or does not fit `i16`.
    pub tz: i64,
    /// The named branch (`"default"` when unset), as UTF-8 text (an IR ref name).
    pub branch: String,
    /// Every changelog extra, key → value as raw bytes, unescaped, **in the order stored**,
    /// **including** `branch` (the caller carries all of them as [`brygge_ir::Extra`]s, exactly as stored —
    /// review 037 R-1). Keys are unique — a duplicate key is refused during parsing.
    pub extras: Extras,
    /// The commit description/message, as the source's raw bytes.
    pub description: Vec<u8>,
}

/// Find the first occurrence of `\n\n` in `text`, splitting it into `(header, rest-after-the-blank-line)`.
fn split_header(text: &[u8]) -> (&[u8], &[u8]) {
    for i in 0..text.len() {
        if text.get(i..i + 2) == Some(b"\n\n") {
            let header = text.get(..i).unwrap_or(text);
            let rest = text.get(i + 2..).unwrap_or(&[]);
            return (header, rest);
        }
    }
    (text, &[])
}

/// Split `header` into lines on `\n`, without requiring the whole header to be valid UTF-8.
fn split_lines(header: &[u8]) -> Vec<&[u8]> {
    header.split(|&b| b == b'\n').collect()
}

/// Split `line` at the first ASCII space, without requiring the whole line to be valid UTF-8.
fn split_once_byte(line: &[u8], sep: u8) -> Option<(&[u8], &[u8])> {
    let i = line.iter().position(|&b| b == sep)?;
    Some((line.get(..i)?, line.get(i + 1..).unwrap_or(&[])))
}

/// Parse changeset text.
///
/// # Errors
/// [`Error::Read`] if the header is malformed (missing manifest node, user, or date line; the manifest
/// node hex or the date line's time/timezone tokens are not valid ASCII; an extras entry is malformed,
/// carries an unrecognized escape, or a duplicate key; the branch name is not valid UTF-8) — those
/// structural pieces are fixed-format regardless of the changeset's own text encoding.
pub fn parse(text: &[u8]) -> Result<Changeset, Error> {
    let (header, description) = split_header(text);
    let lines = split_lines(header);
    let mut iter = lines.into_iter();
    let manifest_line = iter
        .next()
        .ok_or_else(|| Error::Read("changelog entry missing manifest line".to_string()))?;
    let manifest_hex = std::str::from_utf8(manifest_line)
        .map_err(|_| Error::Read("changelog manifest line is not valid UTF-8".to_string()))?;
    let manifest_node = parse_hex20(manifest_hex)?;
    let user = iter
        .next()
        .ok_or_else(|| Error::Read("changelog entry missing user line".to_string()))?
        .to_vec();
    let date_line = iter
        .next()
        .ok_or_else(|| Error::Read("changelog entry missing date line".to_string()))?;
    // remaining lines are the changed-file list; we derive ops from manifest diffs instead.

    // Only the time and timezone tokens are structurally fixed-format (ASCII digits); the extras field
    // that may follow them is parsed on raw bytes (review 009 R-2).
    let (time_bytes, rest) = split_once_byte(date_line, b' ')
        .ok_or_else(|| Error::Read("changelog entry missing date/timezone".to_string()))?;
    let (tz_bytes, extras_bytes) = match split_once_byte(rest, b' ') {
        Some((tz, extras)) => (tz, extras),
        None => (rest, &[][..]),
    };
    let time_str = std::str::from_utf8(time_bytes)
        .map_err(|_| Error::Read("changelog time is not valid ASCII".to_string()))?;
    let tz_str = std::str::from_utf8(tz_bytes)
        .map_err(|_| Error::Read("changelog timezone is not valid ASCII".to_string()))?;
    let time: i64 = time_str
        .parse()
        .map_err(|_| Error::Read(format!("bad changelog time {time_str:?}")))?;
    let tz: i64 = tz_str
        .parse()
        .map_err(|_| Error::Read(format!("bad changelog timezone {tz_str:?}")))?;

    let extras = parse_extras(extras_bytes)?;
    let branch = match extras.iter().find(|(k, _)| k.as_slice() == b"branch") {
        Some((_, v)) => String::from_utf8(v.clone())
            .map_err(|_| Error::Read("changelog branch name is not valid UTF-8".to_string()))?,
        None => "default".to_string(),
    };

    Ok(Changeset {
        manifest_node,
        user,
        time,
        tz,
        branch,
        extras,
        description: description.to_vec(),
    })
}

/// Parse the encoded extras blob (`\0`-separated, each entry an escaped `key:value` pair), preserving
/// order, following `changelog.py`'s `decodeextra` exactly: split on `\0`, unescape the **whole entry**,
/// then split at the **first** `:` (review 009 R-2).
///
/// # Errors
/// [`Error::Read`] on an entry with no `:`, an unrecognized escape, or a duplicate key.
fn parse_extras(field: &[u8]) -> Result<Extras, Error> {
    let mut out: Extras = Vec::new();
    let mut seen: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    for entry in field.split(|&b| b == 0) {
        if entry.is_empty() {
            continue;
        }
        let unescaped = string_unescape(entry)?;
        let colon = unescaped
            .iter()
            .position(|&b| b == b':')
            .ok_or_else(|| Error::Read("changelog extra entry has no ':'".to_string()))?;
        let key = unescaped.get(..colon).unwrap_or(&[]).to_vec();
        let value = unescaped.get(colon + 1..).unwrap_or(&[]).to_vec();
        if !seen.insert(key.clone()) {
            return Err(Error::Read(format!(
                "duplicate changelog extra key {:?}",
                crate::util::escape_bytes(&key)
            )));
        }
        out.push((key, value));
    }
    Ok(out)
}

/// Mercurial's `changelog.py::_string_unescape`: protect existing `\\` (escaped-backslash) sequences
/// with a temporary newline marker before converting literal `\0` byte-pairs to real NUL bytes — so a
/// backslash immediately followed by a literal `0` character (itself the escaped form of a real
/// backslash-then-'0') is never mistaken for the `\0` escape — then delegate to the general
/// escape-decode grammar.
fn string_unescape(text: &[u8]) -> Result<Vec<u8>, Error> {
    let mut buf = text.to_vec();
    if contains(&buf, b"\\0") {
        buf = replace_all(&buf, b"\\\\", b"\\\\\n");
        buf = replace_all(&buf, b"\\0", b"\0");
        buf.retain(|&b| b != b'\n');
    }
    escape_decode(&buf)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Non-overlapping left-to-right replace, matching Python's `bytes.replace`.
fn replace_all(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack.get(i..).is_some_and(|rest| rest.starts_with(from)) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            if let Some(&b) = haystack.get(i) {
                out.push(b);
            }
            i += 1;
        }
    }
    out
}

/// Python's `codecs.escape_decode` grammar (what `stringutil.unescapestr` calls), restricted to the
/// escapes Mercurial's own writer (`_string_escape`/`escapestr`) can ever produce or that its reader
/// documents: `\\ \' \" \a \b \f \n \r \t \v`, octal `\ooo` (1-3 digits), and hex `\xhh` (exactly 2
/// digits). Any other escape is refused (review 009 R-2, deliberately stricter than Mercurial's own
/// reader, which passes an unrecognized escape through literally): it cannot have come from `hg`, so a
/// crafted store is refused, not guessed.
fn escape_decode(text: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let b = *text.get(i).unwrap_or(&0);
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        let next = *text
            .get(i + 1)
            .ok_or_else(|| Error::Read("changelog extra ends with a bare backslash".to_string()))?;
        match next {
            b'\\' => {
                out.push(b'\\');
                i += 2;
            }
            b'\'' => {
                out.push(b'\'');
                i += 2;
            }
            b'"' => {
                out.push(b'"');
                i += 2;
            }
            b'a' => {
                out.push(0x07);
                i += 2;
            }
            b'b' => {
                out.push(0x08);
                i += 2;
            }
            b'f' => {
                out.push(0x0C);
                i += 2;
            }
            b'n' => {
                out.push(b'\n');
                i += 2;
            }
            b'r' => {
                out.push(b'\r');
                i += 2;
            }
            b't' => {
                out.push(b'\t');
                i += 2;
            }
            b'v' => {
                out.push(0x0B);
                i += 2;
            }
            b'x' => {
                let hex = text.get(i + 2..i + 4).ok_or_else(|| {
                    Error::Read("truncated \\x escape in changelog extra".to_string())
                })?;
                // Both bytes must be ASCII hex digits: `from_str_radix` alone would accept a sign
                // (`\x+f`), where Python's `escape_decode` raises (review 009 F-1).
                if !hex.iter().all(u8::is_ascii_hexdigit) {
                    return Err(Error::Read("bad \\x escape in changelog extra".to_string()));
                }
                let s = std::str::from_utf8(hex)
                    .map_err(|_| Error::Read("bad \\x escape in changelog extra".to_string()))?;
                let v = u8::from_str_radix(s, 16)
                    .map_err(|_| Error::Read("bad \\x escape in changelog extra".to_string()))?;
                out.push(v);
                i += 4;
            }
            b'0'..=b'7' => {
                let mut j = i + 1;
                let mut val: u32 = 0;
                let mut count = 0;
                while count < 3 {
                    let Some(&d) = text.get(j) else { break };
                    if !(b'0'..=b'7').contains(&d) {
                        break;
                    }
                    val = val * 8 + u32::from(d - b'0');
                    j += 1;
                    count += 1;
                }
                out.push((val & 0xFF) as u8);
                i = j;
            }
            other => {
                return Err(Error::Read(format!(
                    "unrecognized escape '\\{}' in changelog extra",
                    crate::util::escape_bytes(&[other])
                )));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

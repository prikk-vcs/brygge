//! Mercurial's **fncache store path encoding** (RFC 005 D-1): map a repo-relative file path to the
//! filelog index path under `.hg/store/`. Verified against real stores (see tests): `Sub/File.TXT` →
//! `data/_sub/_file._t_x_t.i`, `.config/App.ini` → `data/~2econfig/_app.ini.i`, `_under.txt` →
//! `data/__under.txt.i`.
//!
//! Per path component: a leading `.` or space is **dotencoded** (`~2e`/`~20`); then each byte is encoded —
//! `A`–`Z` → `_` + lowercase, `_` → `__`, `~` → `~7e`, control/high/Windows-reserved bytes → `~xx`, the
//! rest pass through. A path long enough to need Mercurial's hashed (`dh/`) encoding is **refused** rather
//! than misread (rare; RFC 005 clean/safe posture).

use crate::Error;

/// Mercurial's store path length budget before it switches to the hashed `dh/` encoding.
const MAX_STORE_PATH: usize = 120;

/// Encode a repo-relative file path (e.g. `"Sub/File.TXT"`) to its store-relative filelog index path
/// (e.g. `"data/_sub/_file._t_x_t.i"`).
///
/// # Errors
/// [`Error::Read`] if the encoded path would exceed the store length budget (the hashed `dh/` encoding is
/// not implemented in this build) or the logical path is empty/absolute.
pub fn store_path(logical: &str) -> Result<String, Error> {
    if logical.is_empty() || logical.starts_with('/') {
        return Err(Error::Read(format!(
            "invalid repo-relative path {logical:?}"
        )));
    }
    let mut out = String::from("data/");
    for (i, comp) in logical.split('/').enumerate() {
        if comp.is_empty() || comp == "." || comp == ".." {
            return Err(Error::Read(format!(
                "invalid path component in {logical:?}"
            )));
        }
        if i > 0 {
            out.push('/');
        }
        encode_component(comp, &mut out);
    }
    out.push_str(".i");
    if out.len() > MAX_STORE_PATH {
        return Err(Error::Read(format!(
            "path {logical:?} needs Mercurial's hashed store encoding (>{MAX_STORE_PATH} bytes), \
             which this build does not implement; refused rather than misread"
        )));
    }
    Ok(out)
}

fn encode_component(comp: &str, out: &mut String) {
    let bytes = comp.as_bytes();
    let mut start = 0;
    match bytes.first() {
        Some(b'.') => {
            out.push_str("~2e");
            start = 1;
        }
        Some(b' ') => {
            out.push_str("~20");
            start = 1;
        }
        _ => {}
    }
    for &b in bytes.iter().skip(start) {
        encode_byte(b, out);
    }
}

fn encode_byte(b: u8, out: &mut String) {
    use std::fmt::Write as _;
    match b {
        b'A'..=b'Z' => {
            out.push('_');
            out.push((b | 0x20) as char);
        }
        b'_' => out.push_str("__"),
        b'~' => out.push_str("~7e"),
        b'\\' | b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|' => {
            let _ = write!(out, "~{b:02x}");
        }
        0x20..=0x7d => out.push(b as char),
        _ => {
            let _ = write!(out, "~{b:02x}"); // control (<0x20) and high (>=0x7e handled: 0x7e is '~')
        }
    }
}

#[cfg(test)]
mod tests;

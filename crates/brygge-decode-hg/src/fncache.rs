//! Mercurial's **store path encoding** (RFC 005 D-1, completed by RFC 013 D-1): map a repo-relative file
//! path to the names of its filelog's two files under `.hg/store/`: the index (`.i`) and the data (`.d`).
//!
//! This is Mercurial's own `mercurial/store.py` (7.2.4) `_hybridencode`, function for function:
//!
//! 1. [`encodedir`] (`_encodedir`): a directory named `*.hg`, `*.i` or `*.d` gets `.hg` appended
//!    (`dir.i/` → `dir.i.hg/`), so a file `foo` (`foo.i`) and a directory `foo.i` cannot collide.
//! 2. [`encode_filename`] (`_encodefname`, reversible): `A`–`Z` → `_` + lowercase, `_` → `__`, and every
//!    control, `\:*?"<>|`, `~` and high byte → `~xx`.
//! 3. [`aux_encode`] (`_auxencode`) on each `/`-separated component: with `dotencode`, a leading `.` or
//!    space → `~2e` / `~20`; otherwise a Windows-reserved name (`aux`, `con`, `prn`, `nul`, `com1`–`com9`,
//!    `lpt1`–`lpt9`, alone or before a `.`) has its third byte encoded (`aux` → `au~78`); and in every case
//!    a trailing `.` or space → `~2e` / `~20`.
//! 4. If the result is at most [`MAX_STORE_PATH`] (120) bytes, that is the store name. Otherwise
//!    [`hash_encode`] (`_hashencode`, **not** reversible): `dh/`, the first 8 bytes of each lowercased
//!    directory (as many as fit in 68 bytes), the beginning of the basename as a filler, the SHA-1 of the
//!    whole path from step 1, and the extension.
//!
//! **A hashed name embeds the SHA-1 of its own path**, and the `.i` and `.d` paths differ, so the two files
//! of a filelog are named separately ([`StorePaths`]); the `.d` name is never derived from the `.i` name.
//! `dotencode` follows the repository's `.hg/requires`. Mercurial's own encoder is the ground truth: the
//! tests hold a table generated from it.

use sha1_checked::{CollisionResult, Digest, Sha1};

use crate::Error;

/// Mercurial's store path length budget (`_maxstorepathlen`): a longer encoded name is hashed.
const MAX_STORE_PATH: usize = 120;
/// The bytes kept of each directory in a hashed name (`_dirprefixlen`).
const DIR_PREFIX_LEN: usize = 8;
/// The longest the kept directory prefixes may be, joined (`_maxshortdirslen`).
const MAX_SHORT_DIRS_LEN: usize = 8 * (DIR_PREFIX_LEN + 1) - 4;

/// The store-relative names of one filelog's two files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePaths {
    /// The index file, e.g. `data/_sub/_file._t_x_t.i`.
    pub index: String,
    /// The data file of a non-inline revlog, e.g. `data/_sub/_file._t_x_t.d`.
    pub data: String,
}

/// The store names of the filelog for the repo-relative file `logical` (e.g. `"Sub/File.TXT"`), each encoded
/// from its own path.
///
/// # Errors
/// [`Error::Read`] if the logical path is empty, absolute or has an empty, `.` or `..` component, or if
/// hashing it triggers SHA-1 collision detection (a crafted path).
pub fn store_paths(logical: &str, dotencode: bool) -> Result<StorePaths, Error> {
    if logical.is_empty() || logical.starts_with('/') {
        return Err(Error::Read(format!(
            "invalid repo-relative path {logical:?}"
        )));
    }
    if logical
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err(Error::Read(format!(
            "invalid path component in {logical:?}"
        )));
    }
    Ok(StorePaths {
        index: encode(logical, ".i", dotencode)?,
        data: encode(logical, ".d", dotencode)?,
    })
}

/// `_hybridencode(b"data/" + logical + ext, dotencode)`.
fn encode(logical: &str, ext: &str, dotencode: bool) -> Result<String, Error> {
    let mut path = b"data/".to_vec();
    path.extend_from_slice(logical.as_bytes());
    path.extend_from_slice(ext.as_bytes());
    let path = encodedir(&path);

    let encoded = encode_filename(&path);
    let mut parts: Vec<Vec<u8>> = encoded.split(|&b| b == b'/').map(<[u8]>::to_vec).collect();
    aux_encode(&mut parts, dotencode);
    let res = parts.join(&b'/');
    let res = if res.len() > MAX_STORE_PATH {
        hash_encode(&path, dotencode)?
    } else {
        res
    };
    String::from_utf8(res)
        .map_err(|_| Error::Read("an encoded store path is not ASCII".to_string()))
}

/// `_encodedir`: three sequential replacements over the whole path, in Mercurial's order.
fn encodedir(path: &[u8]) -> Vec<u8> {
    let step = replace(path, b".hg/", b".hg.hg/");
    let step = replace(&step, b".i/", b".i.hg/");
    replace(&step, b".d/", b".d.hg/")
}

/// Replace every non-overlapping occurrence of `from` with `to`, left to right (Python `bytes.replace`).
fn replace(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while let Some(rest) = hay.get(i..).filter(|r| !r.is_empty()) {
        if rest.starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.extend_from_slice(rest.get(..1).unwrap_or(&[]));
            i += 1;
        }
    }
    out
}

/// The bytes Mercurial escapes as `~xx` (`_reserved`): controls, `\:*?"<>|`, and `~` and everything above.
fn reserved(b: u8) -> bool {
    !(32..126).contains(&b) || matches!(b, b'\\' | b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|')
}

/// `_encodefname`, the reversible per-byte encoding.
fn encode_filename(path: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(path.len() + path.len() / 4);
    for &b in path {
        if reserved(b) {
            push_tilde(&mut out, b);
        } else if b.is_ascii_uppercase() {
            out.push(b'_');
            out.push(b.to_ascii_lowercase());
        } else if b == b'_' {
            out.extend_from_slice(b"__");
        } else {
            out.push(b);
        }
    }
    out
}

/// `lowerencode`, the non-reversible encoding of the hashed name: `A`–`Z` lowercased, reserved → `~xx`, and
/// `_` left alone (it is **not** doubled here).
fn lower_encode(path: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(path.len());
    for &b in path {
        if reserved(b) {
            push_tilde(&mut out, b);
        } else {
            out.push(b.to_ascii_lowercase());
        }
    }
    out
}

fn push_tilde(out: &mut Vec<u8>, b: u8) {
    let hex = |n: u8| if n < 10 { b'0' + n } else { b'a' + n - 10 };
    out.push(b'~');
    out.push(hex(b >> 4));
    out.push(hex(b & 15));
}

/// `_auxencode`: encode Windows-reserved names and a trailing `.` or space, and (with `dotencode`) a leading
/// `.` or space, in each component.
fn aux_encode(parts: &mut [Vec<u8>], dotencode: bool) {
    for part in parts.iter_mut() {
        let Some((&first, rest)) = part.split_first() else {
            continue;
        };
        if dotencode && (first == b'.' || first == b' ') {
            let mut n = Vec::with_capacity(part.len() + 2);
            push_tilde(&mut n, first);
            n.extend_from_slice(rest);
            *part = n;
        } else {
            let l = part.iter().position(|&b| b == b'.').unwrap_or(part.len());
            let name3 = part.get(..3).unwrap_or(&[]);
            let reserved_name = (l == 3 && matches!(name3, b"aux" | b"con" | b"prn" | b"nul"))
                || (l == 4
                    && part.get(3).is_some_and(|d| (b'1'..=b'9').contains(d))
                    && matches!(name3, b"com" | b"lpt"));
            if reserved_name {
                // encode the third letter (`aux` -> `au~78`)
                let mut n = part.get(..2).unwrap_or(&[]).to_vec();
                push_tilde(&mut n, part.get(2).copied().unwrap_or(0));
                n.extend_from_slice(part.get(3..).unwrap_or(&[]));
                *part = n;
            }
        }
        // `part` is the possibly updated name, as Mercurial's `n`.
        if let Some(&last) = part.last() {
            if last == b'.' || last == b' ' {
                part.pop();
                push_tilde(part, last);
            }
        }
    }
}

/// `_hashencode`: the hashed `dh/` name of `path` (already `encodedir`ed, `data/…<ext>`).
fn hash_encode(path: &[u8], dotencode: bool) -> Result<Vec<u8>, Error> {
    let digest = hex_sha1(path)?;
    let mut parts: Vec<Vec<u8>> = lower_encode(path.get(5..).unwrap_or(&[]))
        .split(|&b| b == b'/')
        .map(<[u8]>::to_vec)
        .collect();
    aux_encode(&mut parts, dotencode);
    let basename = parts.last().cloned().unwrap_or_default();
    let ext = splitext_ext(&basename);

    let mut dirs: Vec<u8> = Vec::new();
    let mut dirs_len = 0usize;
    for p in parts.iter().take(parts.len().saturating_sub(1)) {
        let mut d = p.get(..DIR_PREFIX_LEN).unwrap_or(p).to_vec();
        if let Some(&last) = d.last() {
            if last == b'.' || last == b' ' {
                // Windows cannot access a directory ending in a period or a space.
                d.pop();
                d.push(b'_');
            }
        }
        let t = if dirs_len == 0 {
            d.len()
        } else {
            let t = dirs_len + 1 + d.len();
            if t > MAX_SHORT_DIRS_LEN {
                break;
            }
            dirs.push(b'/');
            t
        };
        dirs.extend_from_slice(&d);
        dirs_len = t;
    }
    if !dirs.is_empty() {
        dirs.push(b'/');
    }

    let mut res = b"dh/".to_vec();
    res.extend_from_slice(&dirs);
    res.extend_from_slice(digest.as_bytes());
    res.extend_from_slice(ext);
    if res.len() < MAX_STORE_PATH {
        let space_left = MAX_STORE_PATH - res.len();
        let filler = basename.get(..space_left).unwrap_or(&basename);
        let mut with = b"dh/".to_vec();
        with.extend_from_slice(&dirs);
        with.extend_from_slice(filler);
        with.extend_from_slice(digest.as_bytes());
        with.extend_from_slice(ext);
        res = with;
    }
    Ok(res)
}

/// `os.path.splitext(basename)[1]` for a name without a separator: the part from the last `.`, unless only dots
/// precede it (leading dots do not start an extension).
fn splitext_ext(basename: &[u8]) -> &[u8] {
    let Some(dot) = basename.iter().rposition(|&b| b == b'.') else {
        return &[];
    };
    if basename
        .get(..dot)
        .is_some_and(|lead| lead.iter().any(|&b| b != b'.'))
    {
        basename.get(dot..).unwrap_or(&[])
    } else {
        &[]
    }
}

/// Lowercase hex of the SHA-1 of `data`, refusing a crafted input (`sha1-checked` collision detection).
fn hex_sha1(data: &[u8]) -> Result<String, Error> {
    let mut hasher = Sha1::new();
    Digest::update(&mut hasher, data);
    let result = hasher.try_finalize();
    if result.has_collision() {
        return Err(Error::Read(
            "a file path triggers SHA-1 collision detection (crafted store)".to_string(),
        ));
    }
    let CollisionResult::Ok(digest) = result else {
        return Err(Error::Read(
            "SHA-1 collision detection reported an unexpected state".to_string(),
        ));
    };
    Ok(crate::util::hex(digest.as_slice()))
}

#[cfg(test)]
mod tests;

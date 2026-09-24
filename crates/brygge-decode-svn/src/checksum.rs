//! The checksums a dump states (RFC 013 D-2, OQ-5), computed to check them.
//!
//! These are **consistency** checks, not authenticity: the dump is untrusted and can state any checksum it
//! likes, so a match proves only that the text we rebuilt (from fulltext, or from a delta and its base) is the
//! text the dump's producer meant. They are also the end-to-end check of our own svndiff application, which is
//! why every node's checksum is checked and why MD5 matters: `svnrdump` writes MD5 only. SHA-1 is computed with
//! collision detection (`sha1-checked`), as elsewhere in the workspace; a detected collision is a refusal.

use md5::Md5;
use sha1_checked::{CollisionResult, Digest, Sha1};

use crate::Error;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Lowercase hex of the MD5 of `data`. Infallible in effect; the `Result` gives it the same shape as
/// [`sha1_hex`].
///
/// # Errors
/// Never (kept so the two hash functions have one signature).
pub fn md5_hex(data: &[u8]) -> Result<String, Error> {
    let mut hasher = Md5::new();
    md5::Digest::update(&mut hasher, data);
    Ok(hex(md5::Digest::finalize(hasher).as_slice()))
}

/// Lowercase hex of the SHA-1 of `data`, refusing a crafted input (`sha1-checked` collision detection).
///
/// # Errors
/// [`Error::Read`] if collision detection fires.
pub fn sha1_hex(data: &[u8]) -> Result<String, Error> {
    let mut hasher = Sha1::new();
    Digest::update(&mut hasher, data);
    let result = hasher.try_finalize();
    if result.has_collision() {
        return Err(Error::Read(
            "a node's text triggers SHA-1 collision detection (a crafted dump)".to_string(),
        ));
    }
    let CollisionResult::Ok(digest) = result else {
        return Err(Error::Read(
            "SHA-1 collision detection reported an unexpected state".to_string(),
        ));
    };
    Ok(hex(digest.as_slice()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::{md5_hex, sha1_hex};

    #[test]
    fn known_digests() {
        assert_eq!(md5_hex(b"").unwrap(), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            md5_hex(b"hello\n").unwrap(),
            "b1946ac92492d2347c6235b4d2611184"
        );
        assert_eq!(
            sha1_hex(b"").unwrap(),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            sha1_hex(b"hello\n").unwrap(),
            "f572d396fae9206628714fb2ce00f72e94f2258f"
        );
    }
}

//! Untrusted-text neutralization (CR-19, threat model T-1/T-3). Every string that can originate in a
//! source repository — a path, a name, an email, a message, a ref name, a drop's `what`, an error text
//! carrying a path — is routed through one of these two functions before it reaches stdout or stderr, in
//! both human and machine form. Neither function can be bypassed per-call-site by construction: there is
//! no third way to write source-derived text.
//!
//! - [`human`] makes control characters and invisible/bidirectional Unicode formatting characters visible
//!   as `\u{XXXX}`, so source text can never drive or disguise the terminal.
//! - [`machine_value`] percent-encodes anything outside a safe ASCII set, so source text can never forge a
//!   machine-format key or line.

use std::borrow::Cow;
use std::fmt::Write as _;

/// True for a character `human` must escape: the C0/C1 control ranges, and the Unicode bidirectional,
/// invisible, or line-breaking formatting characters an attacker could use to disguise, reorder, or split
/// terminal or log-viewer output. Extended after review 003 R-2: the first pass (handoff §3.6) missed
/// U+00AD (soft hyphen, invisible), U+061C (Arabic letter mark, a bidi control), U+180E (Mongolian vowel
/// separator, invisible), U+2028/U+2029 (line/paragraph separators — these can forge a line), U+206A–
/// U+206F (deprecated format controls), and U+FFF9–U+FFFB (interlinear annotation controls).
fn needs_escape(c: char) -> bool {
    let cp = u32::from(c);
    matches!(cp, 0x00..=0x1F | 0x7F | 0x80..=0x9F)
        || matches!(
            cp,
            0x00AD
                | 0x061C
                | 0x180E
                | 0x200B..=0x200F
                | 0x2028..=0x2029
                | 0x202A..=0x202E
                | 0x2060..=0x206F
                | 0xFEFF
                | 0xFFF9..=0xFFFB
        )
}

/// Escape `s` for safe display on a terminal (human output). Every control character and every
/// bidirectional/invisible Unicode format character becomes `\u{XXXX}`; a literal backslash becomes `\\`
/// so that an escape sequence in the *output* is never ambiguous with one that arrived in the *input* —
/// only this function's own escaping can produce a lone backslash followed by `u{`.
#[must_use]
pub fn human(s: &str) -> Cow<'_, str> {
    if !s.chars().any(|c| c == '\\' || needs_escape(c)) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\\' {
            out.push_str("\\\\");
        } else if needs_escape(c) {
            let _ = write!(out, "\\u{{{:04x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// True for an ASCII byte that may pass through [`machine_value`] unescaped.
fn is_safe_machine_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b'@' | b':' | b'+')
}

/// Encode `s` for safe use as a machine-format *value* (never a key — untrusted text never appears in a
/// key at all, CR-19). Every byte outside `A-Za-z0-9-._~/@:+` — including `%`, `=`, space, and every byte
/// of a non-ASCII UTF-8 sequence — becomes `%XX` (uppercase hex). The result can never contain `=` or a
/// newline, so it can never forge a line or a key in the line-oriented machine format.
#[must_use]
pub fn machine_value(s: &str) -> String {
    machine_bytes(s.as_bytes())
}

/// The byte-level form of [`machine_value`], for text that is not necessarily UTF-8 (a commit message is
/// bytes, RFC 011 D-4). Percent-encoded once: a consumer percent-decodes to the **exact** bytes, whatever
/// they are, and no display escaping is applied first (that belongs to the human form only).
#[must_use]
pub fn machine_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if is_safe_machine_byte(b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests;

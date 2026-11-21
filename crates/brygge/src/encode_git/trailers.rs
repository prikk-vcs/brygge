//! Identity lines, timestamps, and the `Brygge-*` trailer block (RFC 019 D-2). Adapted from the
//! retired `examples/encode-git` (RFC 017 batch G, reviews 091/092): the honesty machinery is unchanged,
//! renamed from `Encoded-*` to one `Brygge-` prefix (RFC 019 D-2: "one name per concept"), with two
//! trailers added (`Brygge-Encoded`, `Brygge-Source-Id`).

use std::collections::BTreeMap;

use brygge_ir::{
    ChangeAtom, ChangedPart, EpistemicStatus, Identity, PathOp, SourceKind, Time, TransformRecord,
};

/// Lowercase-hex a byte slice (no dependency, matching `brygge_ir::content`'s own private helper, which
/// this module — isolated to `brygge_ir` and `std` alone, RFC 019's acceptance refinement — cannot
/// reach).
pub(super) fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The fallback for [`source_id_text`], and for any source kind (`Other`) with no defined readable
/// form: `source.atom_id` as is, if every byte is printable ASCII (`0x21`–`0x7E`); otherwise `hex:`
/// followed by lowercase hex.
fn printable_ascii_or_hex(atom_id: &[u8]) -> String {
    if !atom_id.is_empty() && atom_id.iter().all(|&b| (0x21..=0x7E).contains(&b)) {
        // Printable ASCII only, so this is also valid UTF-8.
        String::from_utf8(atom_id.to_vec()).unwrap_or_else(|_| format!("hex:{}", to_hex(atom_id)))
    } else {
        format!("hex:{}", to_hex(atom_id))
    }
}

/// `source.atom_id`, as `Brygge-Source-Id` must write it (review 137 F-7): one readable form per source
/// kind, read by the people who migrated the history, not brygge's own internal encoding of it —
/// `r1234` for a Subversion revision, not a hex dump of its 8 big-endian bytes; a CVS id's `path@rev`
/// pairs each on their own line, not one ~2 KB line of raw bytes. An id that does not have its kind's
/// form (the wrong length for git/hg/svn; anything malformed for cvs) falls back to `hex:` plus lowercase
/// hex, so one bad id never breaks the rest. Specified once here and in the guide; D-6 (batch G2) parses
/// the same forms back.
pub(super) fn source_id_text(kind: &SourceKind, atom_id: &[u8]) -> String {
    match kind {
        SourceKind::Git | SourceKind::Hg => {
            // A 20-byte object id/node: 40 lowercase hex, no prefix — exactly what every other Git/hg
            // tool already shows for one.
            if atom_id.len() == 20 {
                to_hex(atom_id)
            } else {
                format!("hex:{}", to_hex(atom_id))
            }
        }
        SourceKind::Svn => {
            // A revision as 8 big-endian bytes (machine-output.md's own convention): `r<decimal>`.
            if let Ok(bytes) = <[u8; 8]>::try_from(atom_id) {
                format!("r{}", u64::from_be_bytes(bytes))
            } else {
                format!("hex:{}", to_hex(atom_id))
            }
        }
        SourceKind::Cvs => cvs_source_id_text(atom_id),
        SourceKind::Other(_) => printable_ascii_or_hex(atom_id),
    }
}

/// CVS's own two id shapes (`crates/brygge-decode-cvs/src/decode.rs`'s `changeset_atom_id`, and its
/// `branch-point:<name>` atoms): a literal `branch-point:<name>` is written as is; a changeset's sorted
/// `path@rev` pairs, joined by LF in the raw id, are written one pair per line — the first right after
/// the trailer's key, each later one on its own continuation line (an LF, then exactly one space), which
/// `git interpret-trailers --parse` reads back as the one trailer value it already is. Anything that does
/// not cleanly fit either shape (not UTF-8, an empty pair, a pair with a leading/trailing space — which a
/// continuation line could not be told apart from, or no `@` at all for the pairs form) falls back to
/// `hex:`.
fn cvs_source_id_text(atom_id: &[u8]) -> String {
    let Ok(s) = std::str::from_utf8(atom_id) else {
        return format!("hex:{}", to_hex(atom_id));
    };
    if let Some(name) = s.strip_prefix("branch-point:") {
        if name.is_empty() || s.contains('\n') || s.starts_with(' ') || s.ends_with(' ') {
            return format!("hex:{}", to_hex(atom_id));
        }
        return s.to_string();
    }
    let lines: Vec<&str> = s.split('\n').collect();
    let well_formed = lines
        .iter()
        .all(|l| !l.is_empty() && !l.starts_with(' ') && !l.ends_with(' ') && l.contains('@'));
    let Some((first, rest)) = lines.split_first().filter(|_| well_formed) else {
        return format!("hex:{}", to_hex(atom_id));
    };
    let mut out = first.to_string();
    for pair in rest {
        out.push('\n');
        out.push(' ');
        out.push_str(pair);
    }
    out
}

/// Decodes a lowercase-hex string (exactly what [`to_hex`] writes) back to bytes. `None` on an odd
/// length or any non-lowercase-hex character — this is the inverse of brygge's own writer, not a general
/// hex parser, so it is deliberately strict.
fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let a = *pair.first()?;
        let b = *pair.get(1)?;
        if a.is_ascii_uppercase() || b.is_ascii_uppercase() {
            return None;
        }
        let hi = (a as char).to_digit(16)?;
        let lo = (b as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// The exact inverse of [`source_id_text`] (batch G2, D-6): given the artifact's own source kind (one
/// IR, one import, one source kind) and the text a `Brygge-Source-Id:` trailer wrote, recovers the
/// `source.atom_id` bytes it was written from — or `None` when the text fits neither that kind's form
/// nor the `hex:` fallback, which the caller treats as "this commit names no atom in the artifact".
pub(super) fn parse_source_id(kind: &SourceKind, text: &str) -> Option<Vec<u8>> {
    if let Some(hex) = text.strip_prefix("hex:") {
        return from_hex(hex);
    }
    match kind {
        SourceKind::Git | SourceKind::Hg => from_hex(text).filter(|b| b.len() == 20),
        SourceKind::Svn => {
            let digits = text.strip_prefix('r')?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: u64 = digits.parse().ok()?;
            Some(n.to_be_bytes().to_vec())
        }
        SourceKind::Cvs => {
            if let Some(name) = text.strip_prefix("branch-point:") {
                return if name.is_empty() || name.contains('\n') {
                    None
                } else {
                    Some(format!("branch-point:{name}").into_bytes())
                };
            }
            let mut out = String::new();
            for (i, line) in text.split('\n').enumerate() {
                if i == 0 {
                    out.push_str(line);
                } else {
                    let cont = line.strip_prefix(' ')?;
                    out.push('\n');
                    out.push_str(cont);
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out.into_bytes())
            }
        }
        // `Other`'s forward direction (`printable_ascii_or_hex`) writes the id as is when every byte is
        // printable ASCII; that text is already the id's own bytes.
        SourceKind::Other(_) => Some(text.as_bytes().to_vec()),
    }
}

/// Extracts one trailer's value from a raw commit/tag message (batch G2, D-6): the text after
/// `"<key>: "` on the line where it starts, plus every immediately-following continuation line (one
/// leading space, a marker that a line continues the value above it — never stripped: F-7's CVS pairs
/// form embeds that space as part of the value [`source_id_text`] itself returned, so [`parse_source_id`]
/// expects it back verbatim, exactly as git's own trailer parser would hand it back).
///
/// **Splits on raw LF bytes, never requiring the whole message to be UTF-8:** the source's own message
/// text, which this encoder carries unchanged (RFC 011 D-2: "not assumed to be UTF-8 just because most
/// sources are"), may hold a byte that is not valid UTF-8 anywhere before the trailer block — a real
/// historic commit in fish-shell's own corpus does. Only the lines this function actually reads (the
/// trailer this encoder itself appended, always valid UTF-8) are UTF-8-checked; nothing earlier in the
/// message needs to be.
///
/// `None` if `key` does not appear, or if the lines read back are not UTF-8 (which this encoder never
/// itself writes there, so that is always evidence of tampering, not a parsing gap).
pub(super) fn extract_trailer(message: &[u8], key: &str) -> Option<String> {
    let lines: Vec<&[u8]> = message.split(|&b| b == b'\n').collect();
    let prefix = format!("{key}: ");
    let prefix_bytes = prefix.as_bytes();
    let idx = lines.iter().position(|l| l.starts_with(prefix_bytes))?;
    let first_line = *lines.get(idx)?;
    let mut out = std::str::from_utf8(first_line.get(prefix_bytes.len()..)?)
        .ok()?
        .to_string();
    let mut i = idx + 1;
    while let Some(line) = lines.get(i) {
        if line.starts_with(b" ") {
            out.push('\n');
            out.push_str(std::str::from_utf8(line).ok()?);
            i += 1;
        } else {
            break;
        }
    }
    Some(out)
}

/// Truncates `s` to 80 characters plus `…` for a refusal message (review 137 F-7): a CVS source id can
/// be a whole changeset's worth of `path@rev` pairs, and a refusal naming one must stay one line a
/// person can actually read, not reproduce it in full.
pub(super) fn truncate_for_refusal(s: &str) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(80).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// The source kind's label for `Brygge-Encoded: brygge <version> from <label>`.
pub(super) fn source_kind_label(kind: &SourceKind) -> String {
    match kind {
        SourceKind::Git => "git".to_string(),
        SourceKind::Hg => "hg".to_string(),
        SourceKind::Svn => "svn".to_string(),
        SourceKind::Cvs => "cvs".to_string(),
        SourceKind::Other(label) => label.to_lowercase(),
    }
}

/// Everything invented to satisfy Git's format that the IR did not itself carry, named so
/// `Brygge-Supplied` can list exactly what was supplied for one atom or ref (RFC 017 D-1 §1, carried by
/// RFC 019 D-2: "never invent a value the IR does not carry… supply it and mark it as supplied").
#[derive(Debug, Default)]
pub(super) struct Supplied(Vec<String>);

impl Supplied {
    pub(super) fn mark(&mut self, field: &str) {
        self.0.push(field.to_string());
    }

    pub(super) fn trailer(&self) -> Option<String> {
        if self.0.is_empty() {
            None
        } else {
            Some(format!("Brygge-Supplied: {}", self.0.join(", ")))
        }
    }

    /// Folds this instance's fields into a running per-field total (for the encode report's
    /// `encode.supplied.<field>` counts).
    pub(super) fn fold_into(&self, totals: &mut BTreeMap<String, u64>) {
        for field in &self.0 {
            *totals.entry(field.clone()).or_insert(0) += 1;
        }
    }
}

/// Everything this encoder *altered* about a value the IR does carry, to fit Git's format — distinct
/// from [`Supplied`]: a value the IR does not carry, and this encoder invents, is `Brygge-Supplied`; a
/// value the IR *does* carry, and this encoder had to change to write at all, is `Brygge-Altered`,
/// naming the field and why. Never silent. One entry per field: a caller with several reasons for one
/// field joins them itself before calling [`Altered::mark`] (see `sanitize_identity_part`), so repeated
/// alterations to the same field combine into one reason list, not one line per reason.
#[derive(Debug, Default)]
pub(super) struct Altered(Vec<(String, String)>);

impl Altered {
    pub(super) fn mark(&mut self, field: &str, why: &str) {
        self.0.push((field.to_string(), why.to_string()));
    }

    pub(super) fn trailer(&self) -> Option<String> {
        if self.0.is_empty() {
            None
        } else {
            let parts: Vec<String> = self
                .0
                .iter()
                .map(|(field, why)| format!("{field} ({why})"))
                .collect();
            Some(format!("Brygge-Altered: {}", parts.join(", ")))
        }
    }

    /// As [`Supplied::fold_into`], counted by field name (never by reason — RFC 019 D-2's report counts
    /// "every field", not every distinct reason).
    pub(super) fn fold_into(&self, totals: &mut BTreeMap<String, u64>) {
        for (field, _why) in &self.0 {
            *totals.entry(field.clone()).or_insert(0) += 1;
        }
    }
}

/// `name <email>`, Git's identity-line shape. `email` is `<>` — empty, but present — when the IR has
/// none; an absent `<email>` is not valid Git author/committer/tagger syntax.
pub(super) fn identity_line(
    identity: Option<&Identity>,
    supplied: &mut Supplied,
    altered: &mut Altered,
    field: &'static str,
) -> String {
    let Some(identity) = identity else {
        supplied.mark(field);
        return "unknown <>".to_string();
    };
    let name = match identity.name.as_utf8() {
        Some(s) => sanitize_identity_part(s, &format!("{field}-name"), altered),
        None => {
            altered.mark(&format!("{field}-name"), "not UTF-8, replaced");
            "unknown".to_string()
        }
    };
    let email = match &identity.email {
        Some(email) => match email.as_utf8() {
            Some(s) => sanitize_identity_part(s, &format!("{field}-email"), altered),
            None => {
                altered.mark(&format!("{field}-email"), "not UTF-8, replaced");
                String::new()
            }
        },
        None => {
            // Review 136 F-1: the whole identity being absent (the `let Some(identity) = identity`
            // guard above) and only its email being absent are different claims — "brygge invented
            // this author" against "brygge filled in a missing email" — and must be marked under
            // different field names, or a reader cannot tell them apart.
            supplied.mark(&format!("{field}-email"));
            String::new()
        }
    };
    format!("{name} <{email}>")
}

/// `<epoch> <+HHMM|-HHMM>`. A missing offset is supplied as `+0000`.
pub(super) fn when(
    time: Option<Time>,
    supplied: &mut Supplied,
    time_field: &'static str,
    tz_field: &'static str,
) -> String {
    let Some(time) = time else {
        supplied.mark(time_field);
        supplied.mark(tz_field);
        return "0 +0000".to_string();
    };
    let offset = time.offset_minutes.unwrap_or_else(|| {
        supplied.mark(tz_field);
        0
    });
    let sign = if offset < 0 { '-' } else { '+' };
    let abs = offset.unsigned_abs();
    format!("{} {sign}{:02}{:02}", time.seconds, abs / 60, abs % 60)
}

/// Removes or folds everything Git's identity-line syntax cannot carry: a literal LF/CR is folded to a
/// space; `<`/`>` and a NUL are removed. Marks each change `Brygge-Altered`: this is a change to a value
/// the IR *did* carry, never silent.
fn sanitize_identity_part(value: &str, field: &str, altered: &mut Altered) -> String {
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' | '\r' => {
                if !reasons.contains(&"newline folded") {
                    reasons.push("newline folded");
                }
                out.push(' ');
            }
            '<' | '>' => {
                if !reasons.contains(&"angle bracket removed") {
                    reasons.push("angle bracket removed");
                }
            }
            '\0' => {
                if !reasons.contains(&"NUL removed") {
                    reasons.push("NUL removed");
                }
            }
            _ => out.push(ch),
        }
    }
    if !reasons.is_empty() {
        altered.mark(field, &reasons.join(", "));
    }
    out
}

/// A path as an unquoted fast-import token when safe, else a minimal C-style quoted string: quoting is
/// needed only when the path starts with `"` or contains an LF.
pub(super) fn path_token(path: &str) -> String {
    if path.starts_with('"') || path.contains('\n') {
        let mut out = String::from("\"");
        for ch in path.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                _ => out.push(ch),
            }
        }
        out.push('"');
        out
    } else {
        path.to_string()
    }
}

/// One `Brygge-Derived:` trailer line for a `Derived` status, scoped to say what it is about.
fn derived_trailer(scope: &str, status: &EpistemicStatus) -> Option<String> {
    let EpistemicStatus::Derived(d) = status else {
        return None;
    };
    let params: Vec<String> = d.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let params = if params.is_empty() {
        String::new()
    } else {
        format!(" {}", params.join(","))
    };
    Some(format!(
        "Brygge-Derived: {scope} {}{params}",
        d.kind.label()
    ))
}

/// One trailer line per layer of `status`'s own chain, outermost first (RFC 018 D-5): a `Transformed`
/// layer gets `Brygge-Transformed: <scope> <step> <name> <version> (<changed>)`, naming the record
/// `transforms[step]` points at; its `prior` is walked the same way, so a multi-step chain gets one line
/// per layer. A `Derived` status with no `Transformed` wrapping it reuses [`derived_trailer`] unchanged.
pub(super) fn status_chain_trailers(
    scope: &str,
    status: &EpistemicStatus,
    transforms: &[TransformRecord],
) -> Vec<String> {
    match status {
        EpistemicStatus::Transformed {
            step,
            changed,
            prior,
        } => {
            let parts: Vec<&str> = changed.iter().map(ChangedPart::as_str).collect();
            let (name, version) = usize::try_from(*step)
                .ok()
                .and_then(|i| transforms.get(i))
                .map(|t| (t.name.as_str(), t.version.as_str()))
                .unwrap_or(("?", "?"));
            let mut out = vec![format!(
                "Brygge-Transformed: {scope} {step} {name} {version} ({})",
                parts.join(",")
            )];
            out.extend(status_chain_trailers(scope, prior, transforms));
            out
        }
        EpistemicStatus::Derived(_) => derived_trailer(scope, status).into_iter().collect(),
        EpistemicStatus::Stated => Vec::new(),
    }
}

/// Builds every `Brygge-Derived:`/`Brygge-Transformed:` line for one atom: the atom's own status, then
/// each op's, each in canonical (path) order. Scope is `atom` for the atom itself, or `op:<path>` for one
/// of its path operations.
pub(super) fn status_trailers(atom: &ChangeAtom, transforms: &[TransformRecord]) -> Vec<String> {
    let mut out = status_chain_trailers("atom", &atom.status, transforms);
    for op in &atom.ops {
        let (path, status) = match op {
            PathOp::Add { path, status, .. }
            | PathOp::Modify { path, status, .. }
            | PathOp::Delete { path, status, .. }
            | PathOp::Replace { path, status, .. } => (path, status),
        };
        out.extend(status_chain_trailers(
            &format!("op:{path}"),
            status,
            transforms,
        ));
    }
    out
}

#[cfg(test)]
mod tests;

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

/// `source.atom_id`, as `Brygge-Source-Id` must write it (RFC 019 D-2): as is, if every byte is
/// printable ASCII (`0x21`–`0x7E`); otherwise `hex:` followed by lowercase hex. One rule, specified once
/// here and in the guide.
pub(super) fn source_id_text(atom_id: &[u8]) -> String {
    if !atom_id.is_empty() && atom_id.iter().all(|&b| (0x21..=0x7E).contains(&b)) {
        // Printable ASCII only, so this is also valid UTF-8.
        String::from_utf8(atom_id.to_vec()).unwrap_or_else(|_| format!("hex:{}", to_hex(atom_id)))
    } else {
        format!("hex:{}", to_hex(atom_id))
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

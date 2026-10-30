//! `brygge-transform-authors-example`: a mailmap-style author/committer rewrite, built on `brygge-ir`
//! alone (RFC 018 D-6 §4) — the same isolation proof `examples/encode-git` makes for an encoder. **An
//! unsupported example; not part of brygge.** See `README.md`.

use std::collections::BTreeMap;

use brygge_ir::transform::Edit;
use brygge_ir::{BlobId, Decoded, Identity, Ir, Text};

/// Shown in this plugin's own record version string and its `--help`: the same "not part of brygge"
/// labelling `examples/encode-git` uses for the same reason.
pub const LABEL: &str = "an unsupported example; not part of brygge";

/// One mapping line's two sides (RFC 018 D-6 §4): `Old Name <old@email> = New Name <new@email>`.
/// Comparison against an atom's own claim is always by exact bytes — never case- or whitespace-folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// The identity to match, on an atom's own author or committer claim.
    pub old: Identity,
    /// The identity to substitute when `old` matches.
    pub new: Identity,
}

/// Parses a mailmap-style map file's bytes into its mapping lines, in file order. A blank line, or one
/// whose first non-whitespace byte is `#`, is skipped. Every other line must be
/// `Old Name <old@email> = New Name <new@email>`: a name, an angle-bracketed email, an `=`, then the
/// same shape again — both names and both emails are required (an email-less identity cannot be named
/// by this map at all, since there would be nothing after the name to delimit it).
///
/// # Errors
/// A message naming the first malformed line (1-based).
pub fn parse_map(bytes: &[u8]) -> Result<Vec<Mapping>, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "the map file is not valid UTF-8".to_string())?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((left, right)) = line.split_once('=') else {
            return Err(format!(
                "line {lineno}: missing '=' separating old from new"
            ));
        };
        let old = parse_identity(left)
            .ok_or_else(|| format!("line {lineno}: malformed left side (want: Name <email>)"))?;
        let new = parse_identity(right)
            .ok_or_else(|| format!("line {lineno}: malformed right side (want: Name <email>)"))?;
        out.push(Mapping { old, new });
    }
    Ok(out)
}

/// Parses one `Name <email>` side. The email is the text between the *last* `<` and the *last* `>` (so
/// a name that itself contains `<`/`>` cannot be misread as the email delimiter); both the name and the
/// email must be non-empty once trimmed.
fn parse_identity(s: &str) -> Option<Identity> {
    let s = s.trim();
    let open = s.rfind('<')?;
    let close = s.rfind('>')?;
    if close < open {
        return None;
    }
    let name = s.get(..open)?.trim();
    let email = s.get(open + 1..close)?.trim();
    if name.is_empty() || email.is_empty() {
        return None;
    }
    Some(Identity {
        name: Text::utf8(name),
        email: Some(Text::utf8(email)),
    })
}

fn identity_eq(a: &Identity, b: &Identity) -> bool {
    a.name.bytes == b.name.bytes
        && a.email.as_ref().map(|e| &e.bytes) == b.email.as_ref().map(|e| &e.bytes)
}

/// `map`'s own new identity for `identity`, if `identity` byte-matches some entry's `old` side.
fn remap<'a>(map: &'a [Mapping], identity: &Identity) -> Option<&'a Identity> {
    map.iter()
        .find(|m| identity_eq(&m.old, identity))
        .map(|m| &m.new)
}

/// Builds the transformed output (RFC 018 D-6 §4): every atom whose author or committer claim
/// byte-matches `map`'s own old side gets that one claim replaced with the mapped identity; everything
/// else — every op, copy, ref, and every atom this map does not touch — passes through unedited. The new
/// transform record's own `params` carry `map_sha256` (the map file's own content hash, hex) and
/// `map_entries` (how many mapping lines it had) — **never the map file's own path**, so the artifact
/// alone can never reveal where on disk this ran.
///
/// # Errors
/// Whatever [`Edit::finish`] can return. Not expected in practice: every edit this function makes only
/// ever replaces an author/committer claim, exactly the shape `brygge_ir::transform::check` accepts.
pub fn transform(input: &Decoded, map: &[Mapping], map_bytes: &[u8]) -> brygge_ir::Result<Ir> {
    let mut params = BTreeMap::new();
    params.insert("map_sha256".to_string(), BlobId::of(map_bytes).to_hex());
    params.insert("map_entries".to_string(), map.len().to_string());

    let mut edit = Edit::begin(input, "authors", env!("CARGO_PKG_VERSION"), params);
    for (in_atom, atom_edit) in input.ir.atoms.iter().zip(edit.atoms()) {
        if let Some(author) = &in_atom.metadata.author {
            if let Some(new) = remap(map, author) {
                atom_edit.set_author(Some(new.clone()));
            }
        }
        if let Some(committer) = &in_atom.metadata.committer {
            if let Some(new) = remap(map, committer) {
                atom_edit.set_committer(Some(new.clone()));
            }
        }
    }
    edit.finish()
}

#[cfg(test)]
mod tests;

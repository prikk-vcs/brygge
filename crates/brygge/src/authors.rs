//! The authors map format (RFC 019 D-4, OQ-2): a bare name matches a source identity that has no email
//! (the Subversion/CVS case), and `Name <email>` matches one with exactly that name and email (Git/hg).
//! Both sides compare by exact bytes — never case- or whitespace-folded, and never fuzzy. Shared by
//! `brygge inspect --authors` (which writes a map already in this form) and the built-in `brygge
//! transform authors` (which reads one).

use std::collections::{BTreeMap, BTreeSet};

use brygge_ir::{Identity, Ir, Text};

/// One parsed mapping line: `<source> = <target>`, with its 1-based line number (for the "matched
/// nothing" warning and duplicate-source refusal).
#[derive(Debug, Clone)]
pub struct Mapping {
    /// The line this mapping came from.
    pub line: usize,
    /// The identity to match: a bare name (no email) or an exact name+email pair.
    pub source: Identity,
    /// The identity to substitute when `source` matches, already checked against what `encode git`
    /// accepts (no LF, CR, `<`, `>` or NUL in either side).
    pub target: Identity,
}

/// A parsed, validated map file.
#[derive(Debug, Clone, Default)]
pub struct Map {
    /// Every mapping, in file order.
    pub mappings: Vec<Mapping>,
}

/// Parses a map file's bytes. `#` starts a comment, to the end of the line; blank lines (after removing
/// any comment) are ignored. Every other line must be `<source> = <target>`.
///
/// # Errors
/// Every problem found, each naming its 1-based line number, so a user can fix them all in one pass
/// rather than one refusal at a time.
pub fn parse_map(bytes: &[u8]) -> Result<Map, Vec<String>> {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return Err(vec!["the map file is not valid UTF-8".to_string()]),
    };
    let mut mappings = Vec::new();
    let mut errors = Vec::new();
    let mut seen: BTreeSet<(Vec<u8>, Option<Vec<u8>>)> = BTreeSet::new();
    for (i, raw_line) in text.lines().enumerate() {
        let lineno = i + 1;
        let content = match raw_line.split_once('#') {
            Some((before, _)) => before,
            None => raw_line,
        };
        if content.trim().is_empty() {
            continue;
        }
        let Some((left, right)) = content.split_once('=') else {
            errors.push(format!(
                "line {lineno}: missing '=' separating the source identity from the target"
            ));
            continue;
        };
        let left = left.trim();
        let right = right.trim();
        if left.is_empty() {
            errors.push(format!("line {lineno}: the source side is empty"));
            continue;
        }
        if right.is_empty() {
            errors.push(format!("line {lineno}: the target side is empty"));
            continue;
        }
        let source = match parse_source_side(left) {
            Ok(id) => id,
            Err(msg) => {
                errors.push(format!("line {lineno}: {msg}"));
                continue;
            }
        };
        let target = match parse_target_side(right) {
            Ok(id) => id,
            Err(msg) => {
                errors.push(format!("line {lineno}: {msg}"));
                continue;
            }
        };
        let key = identity_key(&source);
        if !seen.insert(key) {
            errors.push(format!(
                "line {lineno}: this source identity is given more than once"
            ));
            continue;
        }
        mappings.push(Mapping {
            line: lineno,
            source,
            target,
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Map { mappings })
}

/// A bare name (no `<`), or `Name <email>` (the last `<`/`>` pair, as the trailer writer's own form).
fn parse_source_side(s: &str) -> Result<Identity, String> {
    let Some(open) = s.find('<') else {
        return Ok(Identity {
            name: Text::utf8(s),
            email: None,
        });
    };
    let Some(close) = s.rfind('>') else {
        return Err("missing a closing '>' in the source identity".to_string());
    };
    if close < open {
        return Err("missing a closing '>' in the source identity".to_string());
    }
    let name = s[..open].trim();
    let email = s[open + 1..close].trim();
    if name.is_empty() {
        return Err("the source name is empty".to_string());
    }
    if email.is_empty() {
        return Err("the source email is empty".to_string());
    }
    Ok(Identity {
        name: Text::utf8(name),
        email: Some(Text::utf8(email)),
    })
}

/// `Name <email>`: both sides required and non-empty, and checked against what `encode git` accepts for
/// a written identity (RFC 019 D-2/G1 F-1), so a mapped identity is never altered later.
fn parse_target_side(s: &str) -> Result<Identity, String> {
    let open = s
        .find('<')
        .ok_or_else(|| "the target needs <email>: Name <email>".to_string())?;
    let close = s
        .rfind('>')
        .ok_or_else(|| "the target needs a closing '>': Name <email>".to_string())?;
    if close < open {
        return Err("the target needs <email> after the name: Name <email>".to_string());
    }
    let name = s[..open].trim();
    let email = s[open + 1..close].trim();
    if name.is_empty() {
        return Err("the target name is empty".to_string());
    }
    if email.is_empty() {
        return Err("the target email is empty".to_string());
    }
    check_identity_part(name, "name")?;
    check_identity_part(email, "email")?;
    Ok(Identity {
        name: Text::utf8(name),
        email: Some(Text::utf8(email)),
    })
}

/// Refuses what an identity line cannot carry (RFC 019 G1 F-1): a literal LF/CR, `<`, `>`, or NUL.
fn check_identity_part(s: &str, field: &str) -> Result<(), String> {
    if s.contains(['\n', '\r', '<', '>', '\0']) {
        return Err(format!(
            "the target {field} must not contain a newline, '<', '>' or NUL"
        ));
    }
    Ok(())
}

fn identity_key(identity: &Identity) -> (Vec<u8>, Option<Vec<u8>>) {
    (
        identity.name.bytes.clone(),
        identity.email.as_ref().map(|e| e.bytes.clone()),
    )
}

/// `map`'s mapping index whose `source` matches `identity` exactly: same name bytes, and either both
/// have no email or both have the same email bytes. No fuzzy match: a bare-name source never matches an
/// identity that has an email, and the reverse.
#[must_use]
pub fn lookup(map: &Map, identity: &Identity) -> Option<usize> {
    let key = identity_key(identity);
    map.mappings
        .iter()
        .position(|m| identity_key(&m.source) == key)
}

/// As [`lookup`], but returns the matched mapping's own `target` directly — the form every call site
/// that actually applies a mapping wants, with no index to carry or re-check.
#[must_use]
pub fn remap<'a>(map: &'a Map, identity: &Identity) -> Option<&'a Identity> {
    let key = identity_key(identity);
    map.mappings
        .iter()
        .find(|m| identity_key(&m.source) == key)
        .map(|m| &m.target)
}

/// One distinct identity an artifact carries, with its count per role (`inspect --authors`, RFC 019
/// D-4 §1). Two identities are distinct whenever their name or email bytes differ at all.
#[derive(Debug, Clone)]
pub struct IdentityCount {
    /// The identity itself.
    pub identity: Identity,
    /// How many atoms claim it as their author.
    pub author: u64,
    /// How many atoms claim it as their committer.
    pub committer: u64,
    /// How many annotated refs claim it as their tagger.
    pub tagger: u64,
}

impl IdentityCount {
    /// The sum across every role — the primary sort key (descending).
    #[must_use]
    pub fn total(&self) -> u64 {
        self.author + self.committer + self.tagger
    }
}

/// Every distinct identity `ir` carries as an atom's author or committer, or an annotated ref's tagger,
/// each with its own per-role count. Sorted by total count (descending), then by the identity's own
/// bytes (name, then email), for a deterministic order when counts tie.
#[must_use]
pub fn collect_identities(ir: &Ir) -> Vec<IdentityCount> {
    let mut by_key: BTreeMap<(Vec<u8>, Option<Vec<u8>>), IdentityCount> = BTreeMap::new();
    let mut bump = |identity: &Identity, role: Role| {
        let key = identity_key(identity);
        let entry = by_key.entry(key).or_insert_with(|| IdentityCount {
            identity: identity.clone(),
            author: 0,
            committer: 0,
            tagger: 0,
        });
        match role {
            Role::Author => entry.author += 1,
            Role::Committer => entry.committer += 1,
            Role::Tagger => entry.tagger += 1,
        }
    };
    for atom in &ir.atoms {
        if let Some(author) = &atom.metadata.author {
            bump(author, Role::Author);
        }
        if let Some(committer) = &atom.metadata.committer {
            bump(committer, Role::Committer);
        }
    }
    for r in &ir.refs {
        if let Some(tagger) = r.annotation.as_ref().and_then(|a| a.tagger.as_ref()) {
            bump(tagger, Role::Tagger);
        }
    }
    let mut out: Vec<IdentityCount> = by_key.into_values().collect();
    out.sort_by(|a, b| {
        b.total()
            .cmp(&a.total())
            .then_with(|| a.identity.name.bytes.cmp(&b.identity.name.bytes))
            .then_with(|| {
                let ae = a.identity.email.as_ref().map(|e| &e.bytes);
                let be = b.identity.email.as_ref().map(|e| &e.bytes);
                ae.cmp(&be)
            })
    });
    out
}

enum Role {
    Author,
    Committer,
    Tagger,
}

/// The identity exactly as a map file's own source/target side would write it: a bare name if there is
/// no email, otherwise `Name <email>`. Bytes are shown lossily (replacing invalid UTF-8), escaped by the
/// caller for the output they are going to (`display::human`/`display::machine_bytes`) — this function
/// itself does no escaping, so it can be reused for both.
#[must_use]
pub fn identity_as_map_source(identity: &Identity) -> String {
    let name = String::from_utf8_lossy(&identity.name.bytes).into_owned();
    match &identity.email {
        Some(email) => format!("{name} <{}>", String::from_utf8_lossy(&email.bytes)),
        None => name,
    }
}

#[cfg(test)]
mod tests;

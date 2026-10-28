//! An unsupported example encoder: brygge's IR to a `git fast-import` stream (RFC 017 D-1/D-2).
//!
//! **Not part of brygge. Never released, never published.** It exists to show that an encoder is
//! buildable on `brygge-ir` alone (PU-3) — see `cargo tree -p brygge-encode-git-example`, which shows
//! only `brygge-ir`'s own dependency closure, no Git library at all.
//!
//! Every commit's message gets four trailers (`trailers`): `Encoded-by`, `Encoded-supplied` (when
//! anything was invented to satisfy Git's format), `Brygge-Derived` (one per `Derived` assertion this
//! commit carries), and `Brygge-Artifact` (the artifact's own digest, so the target records what it was
//! built from). See `README.md` for the full IR→Git mapping and the honesty rules this follows (the
//! encoder author's guide, `docs/src/reference/writing-an-encoder.md`, states the rules in general).

use std::collections::BTreeMap;
use std::io::Write;

use brygge_ir::{
    AtomId, BlobId, ChangeAtom, EpistemicStatus, Identity, Ir, PathOp, RefKind, RefRecord, Time,
};

/// The label every trailer, `--help`, and `README.md` carry (RFC 017 D-1 §2's "the labelling").
pub const LABEL: &str = "an unsupported example; not part of brygge";

/// Lowercase-hex a byte slice (no dependency, matching `brygge_ir::content`'s own private helper, which
/// this crate — deliberately depending on `brygge-ir` alone — cannot reach).
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A Unix file mode this encoder knows how to write. Any other mode is refused by name (RFC 017 D-1 §2:
/// "refuse any other mode by name", never silently coerced to one of these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitMode {
    Regular,
    Executable,
    Symlink,
}

impl GitMode {
    fn from_unix(mode: u32) -> Result<Self, String> {
        match mode {
            0o100_644 => Ok(Self::Regular),
            0o100_755 => Ok(Self::Executable),
            0o120_000 => Ok(Self::Symlink),
            other => Err(format!(
                "mode {other:#o} is not one brygge-encode-git-example writes (only 100644, 100755, \
                 120000 — a regular file, an executable file, or a symlink); refusing rather than \
                 guessing"
            )),
        }
    }

    fn git_mode_str(self) -> &'static str {
        match self {
            Self::Regular => "100644",
            Self::Executable => "100755",
            Self::Symlink => "120000",
        }
    }
}

/// Everything invented to satisfy Git's format that the IR did not itself carry, named so
/// `Encoded-supplied` can list exactly what was supplied for one atom (RFC 017 D-1 §1: "never invent a
/// value the IR does not carry… supply it and mark it as supplied by the encoder").
#[derive(Debug, Default)]
struct Supplied(Vec<&'static str>);

impl Supplied {
    fn mark(&mut self, field: &'static str) {
        self.0.push(field);
    }

    fn trailer(&self) -> Option<String> {
        if self.0.is_empty() {
            None
        } else {
            Some(format!("Encoded-supplied: {}", self.0.join(", ")))
        }
    }
}

/// Everything this program *altered* about a value the IR does carry, to fit Git's format — distinct
/// from [`Supplied`] (review 091 F-1): a value the IR does not carry, and this program invents, is
/// `Encoded-supplied`; a value the IR *does* carry, and this program had to change to write at all, is
/// `Encoded-altered`, naming the field and why. Never silent — the honesty rule ("never invent a value
/// the IR does not carry") extends to "never silently change one it does".
#[derive(Debug, Default)]
struct Altered(Vec<String>);

impl Altered {
    fn mark(&mut self, field: &str, why: &str) {
        self.0.push(format!("{field} ({why})"));
    }

    fn trailer(&self) -> Option<String> {
        if self.0.is_empty() {
            None
        } else {
            Some(format!("Encoded-altered: {}", self.0.join(", ")))
        }
    }
}

/// `name <email>`, Git's identity-line shape. `email` is `<>` — empty, but present — when the IR has
/// none; an absent `<email>` is not valid Git author/committer/tagger syntax.
fn identity_line(
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
            supplied.mark(field);
            String::new()
        }
    };
    format!("{name} <{email}>")
}

/// `<epoch> <+HHMM|-HHMM>`. A missing offset is supplied as `+0000` (RFC 017 D-1 §2, verbatim).
fn when(
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

/// Removes or folds everything Git's identity-line syntax cannot carry — a literal LF/CR (the
/// line-oriented format would read it as ending the line) is folded to a space; `<`/`>` (which would
/// make the identity line unparseable, or parse as a different person) and a NUL are removed — and
/// marks each change `Encoded-altered` (review 091 F-1): this is a change to a value the IR *did*
/// carry, not a value this program invented, so it gets its own trailer, not `Encoded-supplied`.
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

/// A path as an unquoted fast-import token when safe, else a minimal C-style quoted string (RFC 017 D-1
/// §2's `M`/`D` lines): quoting is needed only when the path starts with `"` or contains an LF — the
/// two cases an unquoted path cannot represent (`git-fast-import(1)`).
fn path_token(path: &str) -> String {
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

/// Builds every `Brygge-Derived:` line for one atom: the atom's own status, then each op's, each in
/// canonical (path) order — RFC 017 D-1 §2's "op-level derivations in a form you choose and document".
/// Scope is `atom` for the atom itself, or `op:<path>` for one of its path operations.
fn derived_trailers(atom: &ChangeAtom) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(line) = derived_trailer("atom", &atom.status) {
        out.push(line);
    }
    for op in &atom.ops {
        let (path, status) = match op {
            PathOp::Add { path, status, .. }
            | PathOp::Modify { path, status, .. }
            | PathOp::Delete { path, status, .. }
            | PathOp::Replace { path, status, .. } => (path, status),
        };
        if let Some(line) = derived_trailer(&format!("op:{path}"), status) {
            out.push(line);
        }
    }
    out
}

/// A scratch ref every commit is written under, regardless of which real ref it ultimately belongs to
/// (RFC 017 D-1 §2's ref mapping does not require knowing, for an interior commit, which named ref will
/// eventually contain it — only the DAG shape, which `from`/`merge` already state explicitly). Every
/// real ref is set with its own `reset … from :<mark>` once every commit is written, and this one is
/// then itself `reset` with no `from` — which removes it (`git-fast-import(1)`), so it never appears in
/// the resulting repository.
const SCRATCH_REF: &str = "refs/heads/brygge-encode-git-example-scratch";

/// The Git ref an IR ref maps to, before any writing happens: `Tag` goes to `refs/tags/<name>`;
/// `Branch`/`Bookmark`/`NamedBranch`/`Other` all flatten to `refs/heads/<name>` (the mapping table's own
/// "all flattened to one namespace").
fn git_ref_path(rf: &RefRecord) -> String {
    match rf.kind {
        RefKind::Tag => format!("refs/tags/{}", rf.name),
        _ => format!("refs/heads/{}", rf.name),
    }
}

/// `rf.kind`, named for an error message (not Git syntax).
fn ref_kind_label(kind: &RefKind) -> String {
    match kind {
        RefKind::Branch => "Branch".to_string(),
        RefKind::Tag => "Tag".to_string(),
        RefKind::Bookmark => "Bookmark".to_string(),
        RefKind::NamedBranch => "NamedBranch".to_string(),
        RefKind::Other(label) => format!("Other({label})"),
    }
}

/// Refuses, by name, when two IR refs would flatten to the same Git ref (review 091 F-2): branches,
/// bookmarks, named branches and `Other` all share the `refs/heads/` namespace, so (for example) an hg
/// named branch and a bookmark of the same name would otherwise both `reset` one Git ref, the second
/// silently winning with no trace that the first ever existed. This program does not pick one, and does
/// not rename either — it refuses, naming both.
fn check_no_ref_collisions(refs: &[RefRecord]) -> Result<(), EncodeError> {
    let mut seen: BTreeMap<String, (String, String)> = BTreeMap::new();
    for rf in refs {
        let path = git_ref_path(rf);
        let label = ref_kind_label(&rf.kind);
        if let Some((other_name, other_label)) = seen.get(&path) {
            return Err(EncodeError::Content(format!(
                "two IR refs both map to Git ref {path:?}: {other_name:?} ({other_label}) and {:?} \
                 ({label})",
                rf.name
            )));
        }
        seen.insert(path, (rf.name.clone(), label));
    }
    Ok(())
}

/// Encodes `ir` as a `git fast-import` stream, writing it to `out`. `artifact_digest` becomes every
/// commit's `Brygge-Artifact:` trailer.
///
/// Blobs are read one at a time from `ir.content` and written immediately, each exactly once, in the
/// order its mark is first needed (RFC 017 D-1 §2) — never all buffered at once. Atoms are assumed to
/// already be in canonical order (parents before children — true of every `Ir` this crate's own
/// decoders, or `brygge_ir::artifact`'s reader, produce); encoding does not re-sort them.
///
/// # Errors
/// A [`std::io::Error`] from `out`, or a `String` naming a mode this encoder refuses to write (RFC 017
/// D-1 §2: any mode other than a regular file, an executable file, or a symlink).
pub fn encode<W: Write>(
    ir: &Ir,
    artifact_digest: [u8; 32],
    out: &mut W,
) -> Result<(), EncodeError> {
    check_no_ref_collisions(&ir.refs)?;

    let mut next_mark: u64 = 1;
    let mut blob_marks: BTreeMap<BlobId, u64> = BTreeMap::new();
    let mut atom_marks: BTreeMap<AtomId, u64> = BTreeMap::new();
    let digest_hex = to_hex(&artifact_digest);

    for atom in &ir.atoms {
        // Every blob this atom's ops reference, written the first time it is seen, by mark, one blob's
        // bytes in memory at a time.
        for op in &atom.ops {
            let blob = match op {
                PathOp::Add { blob, .. }
                | PathOp::Modify { blob, .. }
                | PathOp::Replace { blob, .. } => Some(*blob),
                PathOp::Delete { .. } => None,
            };
            if let Some(blob) = blob {
                if let std::collections::btree_map::Entry::Vacant(e) = blob_marks.entry(blob) {
                    let mark = next_mark;
                    next_mark += 1;
                    e.insert(mark);
                    let bytes = ir
                        .content
                        .get(&blob)
                        .map_err(|e| EncodeError::Content(e.to_string()))?
                        .ok_or_else(|| {
                            EncodeError::Content(format!("blob {blob} indexed but not readable"))
                        })?;
                    writeln!(out, "blob")?;
                    writeln!(out, "mark :{mark}")?;
                    writeln!(out, "data {}", bytes.len())?;
                    out.write_all(&bytes)?;
                    writeln!(out)?;
                }
            }
        }

        let mut supplied = Supplied::default();
        let mut altered = Altered::default();
        let author_line = identity_line(
            atom.metadata.author.as_ref(),
            &mut supplied,
            &mut altered,
            "author",
        );
        let author_when = when(
            atom.metadata.author_time,
            &mut supplied,
            "author-time",
            "author-time-zone",
        );
        // The one-claim rule (RFC 011): no distinct committer means the source drew no distinction, so
        // Git's required committer line is supplied from the author's own claim (marked, since Git's
        // format forces a value the IR itself did not separately state).
        let (committer_line, committer_when) =
            match (&atom.metadata.committer, atom.metadata.commit_time) {
                (None, None) => {
                    supplied.mark("committer");
                    supplied.mark("commit-time");
                    (author_line.clone(), author_when.clone())
                }
                (committer, commit_time) => {
                    let committer_line = if committer.is_some() {
                        identity_line(committer.as_ref(), &mut supplied, &mut altered, "committer")
                    } else {
                        supplied.mark("committer");
                        author_line.clone()
                    };
                    let committer_when = if commit_time.is_some() {
                        when(
                            commit_time,
                            &mut supplied,
                            "commit-time",
                            "commit-time-zone",
                        )
                    } else {
                        supplied.mark("commit-time");
                        author_when.clone()
                    };
                    (committer_line, committer_when)
                }
            };

        let message = atom
            .metadata
            .message
            .as_ref()
            .map(|t| t.bytes.clone())
            .unwrap_or_default();

        let mut trailers = vec![format!("Encoded-by: brygge-encode-git-example ({LABEL})")];
        if let Some(line) = supplied.trailer() {
            trailers.push(line);
        }
        if let Some(line) = altered.trailer() {
            trailers.push(line);
        }
        trailers.extend(derived_trailers(atom));
        trailers.push(format!("Brygge-Artifact: sha256:{digest_hex}"));

        let mut body = message;
        if !body.ends_with(b"\n") {
            body.push(b'\n');
        }
        body.push(b'\n');
        body.extend(trailers.join("\n").into_bytes());

        let mark = next_mark;
        next_mark += 1;
        atom_marks.insert(atom.id, mark);

        writeln!(out, "commit {SCRATCH_REF}")?;
        writeln!(out, "mark :{mark}")?;
        writeln!(out, "author {author_line} {author_when}")?;
        writeln!(out, "committer {committer_line} {committer_when}")?;
        // The message's own declared encoding (review 091 F-3), when the source stated one
        // (`Text.encoding`), is written as `git-fast-import(1)`'s own `encoding` command: the bytes
        // below reach Git exactly as the source gave them, tagged with what they are, never re-encoded.
        if let Some(encoding) = atom
            .metadata
            .message
            .as_ref()
            .and_then(|t| t.encoding.as_deref())
        {
            writeln!(out, "encoding {encoding}")?;
        }
        writeln!(out, "data {}", body.len())?;
        out.write_all(&body)?;
        writeln!(out)?;

        let mut parents = atom.parents.iter();
        if let Some(first) = parents.next() {
            let parent_mark = atom_marks.get(first).ok_or_else(|| {
                EncodeError::Content(format!(
                    "atom {first} is not yet encoded (not canonically ordered)"
                ))
            })?;
            writeln!(out, "from :{parent_mark}")?;
        }
        for other in parents {
            let parent_mark = atom_marks.get(other).ok_or_else(|| {
                EncodeError::Content(format!(
                    "atom {other} is not yet encoded (not canonically ordered)"
                ))
            })?;
            writeln!(out, "merge :{parent_mark}")?;
        }

        for op in &atom.ops {
            match op {
                PathOp::Add {
                    path, blob, mode, ..
                }
                | PathOp::Modify {
                    path, blob, mode, ..
                }
                | PathOp::Replace {
                    path, blob, mode, ..
                } => {
                    let git_mode = GitMode::from_unix(*mode).map_err(EncodeError::Content)?;
                    #[allow(clippy::expect_used)]
                    let blob_mark = *blob_marks.get(blob).expect(
                        "every blob an op references was just emitted above, in this same atom",
                    );
                    writeln!(
                        out,
                        "M {} :{blob_mark} {}",
                        git_mode.git_mode_str(),
                        path_token(path)
                    )?;
                }
                PathOp::Delete { path, .. } => {
                    writeln!(out, "D {}", path_token(path))?;
                }
            }
        }
    }

    if !ir.atoms.is_empty() {
        for rf in &ir.refs {
            let Some(&mark) = atom_marks.get(&rf.target) else {
                return Err(EncodeError::Content(format!(
                    "ref {:?} targets an atom this encoder never wrote a mark for",
                    rf.name
                )));
            };
            write_ref(out, rf, mark)?;
        }
        writeln!(out, "reset {SCRATCH_REF}")?;
    }

    Ok(())
}

fn write_ref<W: Write>(out: &mut W, rf: &RefRecord, mark: u64) -> Result<(), EncodeError> {
    match (&rf.kind, &rf.annotation) {
        (RefKind::Tag, Some(annotation)) => {
            let mut supplied = Supplied::default();
            let mut altered = Altered::default();
            let tagger_line = identity_line(
                annotation.tagger.as_ref(),
                &mut supplied,
                &mut altered,
                "tagger",
            );
            let tagger_when = when(annotation.time, &mut supplied, "tag-time", "tag-time-zone");
            let mut message = annotation
                .message
                .as_ref()
                .map(|t| t.bytes.clone())
                .unwrap_or_default();
            // The same honesty rules as a commit message (review 091 F-1: "the same applies to the
            // tagger") — supplied/altered trailers, when either applies, after one blank line.
            let mut trailers = Vec::new();
            if let Some(line) = supplied.trailer() {
                trailers.push(line);
            }
            if let Some(line) = altered.trailer() {
                trailers.push(line);
            }
            if !trailers.is_empty() {
                if !message.ends_with(b"\n") {
                    message.push(b'\n');
                }
                message.push(b'\n');
                message.extend(trailers.join("\n").into_bytes());
            }
            writeln!(out, "tag {}", rf.name)?;
            writeln!(out, "from :{mark}")?;
            writeln!(out, "tagger {tagger_line} {tagger_when}")?;
            writeln!(out, "data {}", message.len())?;
            out.write_all(&message)?;
            writeln!(out)?;
        }
        (RefKind::Tag, None) => {
            writeln!(out, "reset refs/tags/{}", rf.name)?;
            writeln!(out, "from :{mark}")?;
        }
        _ => {
            // Branch, Bookmark, NamedBranch, and any `Other` kind are all flattened to `refs/heads/*`
            // (RFC 017 D-1 §2: "branches with reset") — this example does not invent a second
            // namespace per source ref-kind. Documented as a known limitation in README.md.
            writeln!(out, "reset refs/heads/{}", rf.name)?;
            writeln!(out, "from :{mark}")?;
        }
    }
    Ok(())
}

/// Everything [`encode`] can fail with.
#[derive(Debug)]
pub enum EncodeError {
    /// Writing the stream failed.
    Io(std::io::Error),
    /// The IR itself could not be encoded as asked: a mode this encoder refuses, a blob that would not
    /// read back, or a ref targeting an atom this encoder never reached.
    Content(String),
}

impl From<std::io::Error> for EncodeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Content(s) => write!(f, "{s}"),
        }
    }
}

#[cfg(test)]
mod tests;

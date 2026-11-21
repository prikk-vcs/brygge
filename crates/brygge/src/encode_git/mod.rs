//! `brygge encode git`: the built-in, supported encoder from brygge's IR to a `git fast-import` stream
//! (RFC 019, batch G1 — D-1, D-2, D-3, D-5, D-7).
//!
//! **Isolated to `brygge_ir` and `std` alone** (RFC 019's refinement at acceptance): this module and its
//! children use nothing from the decoders, `gix`, or the rest of the `brygge` crate, enforced by a
//! source-level check in `tools/check-ir-isolation.sh`. Finding and spawning `git` itself, creating and
//! renaming directories, and writing the `<dir>.brygge-report` sidecar file are the CLI's own job
//! (`commands.rs`, beside `encode`), not this module's — `git` is a trusted external tool (RFC 019 D-1),
//! and running it needs process-spawning this module may not have.
//!
//! Started from the retired `examples/encode-git` (RFC 017 batch G, 0.6.0's unsupported reference
//! encoder): the stream-writing and trailer logic carries over almost unchanged; what is new here is the
//! `Brygge-` trailer renaming (RFC 019 D-2), the real `refs/brygge/other/<name>` mapping for `Other` refs
//! (D-3; the example flattened everything into `refs/heads/*`), and collecting every pre-flight refusal
//! into one report instead of refusing on the first one found (D-7).

use std::collections::BTreeMap;
use std::io::Write;

use brygge_ir::{AtomId, BlobId, Ir, PathOp, RefKind, RefRecord, TransformRecord};

mod trailers;

use trailers::{Altered, Supplied};

/// The scratch ref every commit is written under, regardless of which real ref it ultimately belongs to
/// — the DAG shape (`from`/`merge`) is all `git fast-import` needs to place it. Every real ref is set
/// with its own `reset … from :<mark>` once every commit is written, and this ref is then itself `reset`
/// with no `from`, which removes it, so it never appears in the result.
const SCRATCH_REF: &str = "refs/heads/brygge-encode-git-scratch";

/// A Unix file mode this encoder knows how to write (RFC 019 D-7: "only `100644`, `100755` and `120000`
/// are accepted; `160000` cannot occur, since the IR has no submodule" — any other mode is refused by
/// name, never silently coerced).
fn is_writable_mode(mode: u32) -> bool {
    matches!(mode, 0o100_644 | 0o100_755 | 0o120_000)
}

fn git_mode_str(mode: u32) -> &'static str {
    match mode {
        0o100_644 => "100644",
        0o100_755 => "100755",
        0o120_000 => "120000",
        _ => unreachable!("is_writable_mode must be checked first"),
    }
}

/// One pre-flight refusal (RFC 019 D-7), named so the CLI can print "every refusal, one line each" and
/// exit 2, with nothing written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A path operation's mode is not one Git can write.
    BadMode {
        /// The atom's own source id (already formatted, `trailers::source_id_text`).
        atom_source_id: String,
        /// The path with the bad mode.
        path: String,
        /// The mode itself.
        mode: u32,
    },
    /// Two IR refs both flatten to the same Git ref.
    RefCollision {
        /// The Git ref both collide on.
        git_ref: String,
        /// The first IR ref seen: `(name, kind label)`.
        first: (String, String),
        /// The second IR ref seen, which collides with the first.
        second: (String, String),
    },
    /// An annotated tag's message is not UTF-8: `git fast-import` has no `encoding` command for a tag,
    /// unlike a commit.
    NonUtf8TagMessage {
        /// The tag's own name.
        name: String,
    },
    /// A ref's name does not pass Git's own naming rules (`git check-ref-format`). Found by the CLI,
    /// which alone can run `git` — this variant exists so it reports through the same one combined list.
    InvalidRefName {
        /// The Git ref name that failed.
        git_ref: String,
        /// `git check-ref-format`'s own reason, verbatim.
        reason: String,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMode {
                atom_source_id,
                path,
                mode,
            } => write!(
                f,
                "atom {atom_source_id}: path {path:?} has mode {mode:#o}, which Git cannot write (only \
                 100644, 100755 and 120000 are accepted)"
            ),
            Self::RefCollision {
                git_ref,
                first,
                second,
            } => write!(
                f,
                "two IR refs both map to Git ref {git_ref:?}: {:?} ({}) and {:?} ({})",
                first.0, first.1, second.0, second.1
            ),
            Self::NonUtf8TagMessage { name } => write!(
                f,
                "tag {name:?}: its annotation message is not UTF-8, which git fast-import cannot carry \
                 for a tag (only a commit message keeps a declared encoding)"
            ),
            Self::InvalidRefName { git_ref, reason } => {
                write!(f, "ref {git_ref:?} is not a valid Git ref name: {reason}")
            }
        }
    }
}

/// `rf.kind`, named for a refusal/collision message (not Git syntax).
fn ref_kind_label(kind: &RefKind) -> String {
    match kind {
        RefKind::Branch => "Branch".to_string(),
        RefKind::Tag => "Tag".to_string(),
        RefKind::Bookmark => "Bookmark".to_string(),
        RefKind::NamedBranch => "NamedBranch".to_string(),
        RefKind::Other(label) => format!("Other({label})"),
    }
}

/// Whether `kind` maps to `refs/heads/*` and so counts toward [`choose_default_branch`]'s search (RFC
/// 019 D-3: `Branch`, `Bookmark` and `NamedBranch` share that namespace; `Tag` and `Other` do not).
fn is_branch_like(kind: &RefKind) -> bool {
    matches!(
        kind,
        RefKind::Branch | RefKind::Bookmark | RefKind::NamedBranch
    )
}

/// The Git ref an IR ref maps to (RFC 019 D-3's table): `Tag` to `refs/tags/<name>`;
/// `Branch`/`Bookmark`/`NamedBranch` to `refs/heads/<name>`; `Other` to `refs/brygge/other/<name>`,
/// outside both the branch and tag namespaces, so it never reads as either (unlike the retired example,
/// which flattened `Other` into `refs/heads/*` too).
pub(crate) fn git_ref_path(rf: &RefRecord) -> String {
    match rf.kind {
        RefKind::Tag => format!("refs/tags/{}", rf.name),
        RefKind::Branch | RefKind::Bookmark | RefKind::NamedBranch => {
            format!("refs/heads/{}", rf.name)
        }
        RefKind::Other(_) => format!("refs/brygge/other/{}", rf.name),
    }
}

/// Every pair of IR refs that would collide on one Git ref (RFC 019 D-7/D-3): refused by name, never
/// picked between or renamed.
fn ref_collisions(refs: &[RefRecord]) -> Vec<Refusal> {
    let mut seen: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut out = Vec::new();
    for rf in refs {
        let path = git_ref_path(rf);
        let label = ref_kind_label(&rf.kind);
        if let Some(first) = seen.get(&path) {
            out.push(Refusal::RefCollision {
                git_ref: path,
                first: first.clone(),
                second: (rf.name.clone(), label),
            });
        } else {
            seen.insert(path, (rf.name.clone(), label));
        }
    }
    out
}

/// Every path operation whose mode Git cannot write (RFC 019 D-7).
fn bad_modes(ir: &Ir) -> Vec<Refusal> {
    let mut out = Vec::new();
    for atom in &ir.atoms {
        for op in &atom.ops {
            let (path, mode) = match op {
                PathOp::Add { path, mode, .. }
                | PathOp::Modify { path, mode, .. }
                | PathOp::Replace { path, mode, .. } => (path, *mode),
                PathOp::Delete { .. } => continue,
            };
            if !is_writable_mode(mode) {
                out.push(Refusal::BadMode {
                    atom_source_id: trailers::source_id_text(&atom.source.atom_id),
                    path: path.clone(),
                    mode,
                });
            }
        }
    }
    out
}

/// Every annotated tag whose message is not UTF-8 (RFC 019 D-7: `git fast-import` has no `encoding`
/// command for a tag).
fn non_utf8_tag_messages(ir: &Ir) -> Vec<Refusal> {
    let mut out = Vec::new();
    for rf in &ir.refs {
        if rf.kind != RefKind::Tag {
            continue;
        }
        if let Some(annotation) = &rf.annotation {
            if let Some(message) = &annotation.message {
                if message.as_utf8().is_none() {
                    out.push(Refusal::NonUtf8TagMessage {
                        name: rf.name.clone(),
                    });
                }
            }
        }
    }
    out
}

/// Every IR-level refusal (RFC 019 D-7), before anything is written: bad modes, ref collisions, and
/// non-UTF-8 annotated-tag messages. **Does not check ref name validity** — that needs `git
/// check-ref-format`, which only the CLI can run (`commands.rs` merges its own [`Refusal::InvalidRefName`]
/// entries into this same list before reporting).
#[must_use]
pub(crate) fn preflight(ir: &Ir) -> Vec<Refusal> {
    let mut out = bad_modes(ir);
    out.extend(ref_collisions(&ir.refs));
    out.extend(non_utf8_tag_messages(ir));
    out
}

/// The default branch `git init --bare --initial-branch=<name>` should use (RFC 019 D-3): the first of
/// `trunk`, `main`, `master`, `default` that the IR has as a branch-like ref; otherwise the first
/// branch-like ref in canonical (IR) order. `existed` is `false` only when the IR has no branch-like ref
/// at all (tags only) — `name` is then `"main"`, a `HEAD` that names a ref which will not exist, which
/// the caller must say so in the report.
pub(crate) struct DefaultBranch {
    /// The branch name to pass as `--initial-branch`.
    pub(crate) name: String,
    /// Whether the IR actually has a branch-like ref of this name.
    pub(crate) existed: bool,
}

#[must_use]
pub(crate) fn choose_default_branch(ir: &Ir) -> DefaultBranch {
    for candidate in ["trunk", "main", "master", "default"] {
        if ir
            .refs
            .iter()
            .any(|rf| is_branch_like(&rf.kind) && rf.name == candidate)
        {
            return DefaultBranch {
                name: candidate.to_string(),
                existed: true,
            };
        }
    }
    if let Some(rf) = ir.refs.iter().find(|rf| is_branch_like(&rf.kind)) {
        return DefaultBranch {
            name: rf.name.clone(),
            existed: true,
        };
    }
    DefaultBranch {
        name: "main".to_string(),
        existed: false,
    }
}

/// What [`write_stream`] wrote, for the CLI's `<dir>.brygge-report` (RFC 019 D-2).
#[derive(Debug, Default)]
pub(crate) struct Summary {
    /// Commits written (one per atom).
    pub(crate) commits: u64,
    /// Tags written (annotated or lightweight).
    pub(crate) tags: u64,
    /// Every ref written, tags included.
    pub(crate) refs: u64,
    /// Every `Brygge-Supplied` field, counted across every commit and tag.
    pub(crate) supplied: BTreeMap<String, u64>,
    /// Every `Brygge-Altered` field, counted across every commit and tag.
    pub(crate) altered: BTreeMap<String, u64>,
    /// A `Transformed` mark on a ref with nowhere to carry a trailer (a lightweight tag, or a
    /// branch/bookmark/named-branch/other ref — none of which has a message of its own): one line per
    /// such ref, for the report, never printed to stderr (the change from the retired example).
    pub(crate) unplaced_ref_transforms: Vec<String>,
}

/// What every trailer block needs, computed once per [`write_stream`] call and shared by every commit
/// and tag it writes (bundled so neither function call needs more than a handful of parameters).
struct Context<'a> {
    digest_hex: &'a str,
    brygge_version: &'a str,
    source_label: &'a str,
}

fn fold_u64_map<K: Ord>(into: &mut BTreeMap<K, u64>, from: BTreeMap<K, u64>) {
    for (k, v) in from {
        *into.entry(k).or_insert(0) += v;
    }
}

/// Folds a [`Supplied`] instance's per-field counts into `summary.supplied` (`BTreeMap<String, u64>`) —
/// [`Supplied::fold_into`] itself works over `&'static str` keys, so this converts them.
fn fold_supplied(summary: &mut Summary, supplied: &Supplied) {
    let mut totals: BTreeMap<&'static str, u64> = BTreeMap::new();
    supplied.fold_into(&mut totals);
    for (field, count) in totals {
        *summary.supplied.entry(field.to_string()).or_insert(0) += count;
    }
}

/// As [`fold_supplied`], for [`Altered`].
fn fold_altered(summary: &mut Summary, altered: &Altered) {
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    altered.fold_into(&mut totals);
    fold_u64_map(&mut summary.altered, totals);
}

/// Encodes `ir` as a `git fast-import` stream, written to `out`. **Assumes [`preflight`] (plus the
/// CLI's own `git check-ref-format` pass) has already found no refusals** — this is the actual writer,
/// not a second check of the same conditions, so a large history is walked once, not twice. The only
/// error it can return is a genuine I/O error from `out`.
///
/// Blobs are read one at a time from `ir.content` and written immediately, each exactly once, in
/// `store_order` (RFC 018 D-2) — never all buffered at once. Atoms are assumed to already be in
/// canonical order (parents before children, guaranteed by `verify`, which the CLI always runs first);
/// encoding does not re-sort them.
pub(crate) fn write_stream<W: Write>(
    ir: &Ir,
    digest: &[u8; 32],
    out: &mut W,
) -> std::io::Result<Summary> {
    let mut next_mark: u64 = 1;
    let mut blob_marks: BTreeMap<BlobId, u64> = BTreeMap::new();
    let mut atom_marks: BTreeMap<AtomId, u64> = BTreeMap::new();
    let digest_hex = trailers::to_hex(digest);
    let brygge_version = env!("CARGO_PKG_VERSION");

    let mut summary = Summary::default();
    // One IR, one import, so one source kind — computed once and used for every `Brygge-Encoded:` line,
    // on both commits and annotated tags (RFC 019 D-2: the same block goes on an annotated tag, with
    // only `Brygge-Source-Id` omitted, since a ref has no atom of its own).
    let source_label = trailers::source_kind_label(&ir.provenance.source.kind);
    let ctx = Context {
        digest_hex: &digest_hex,
        brygge_version,
        source_label: &source_label,
    };

    // Every blob, first, in `store_order` (RFC 018 D-2: the order that reads each of the artifact's
    // compressed frames exactly once), one blob's bytes in memory at a time, by mark.
    for blob in brygge_ir::artifact::store_order(ir) {
        let mark = next_mark;
        next_mark += 1;
        blob_marks.insert(blob, mark);
        let bytes = ir
            .content
            .get(&blob)
            .map_err(std::io::Error::other)?
            .ok_or_else(|| {
                std::io::Error::other(format!("blob {blob} indexed but not readable"))
            })?;
        writeln!(out, "blob")?;
        writeln!(out, "mark :{mark}")?;
        writeln!(out, "data {}", bytes.len())?;
        out.write_all(&bytes)?;
        writeln!(out)?;
    }

    for atom in &ir.atoms {
        let mut supplied = Supplied::default();
        let mut altered = Altered::default();
        let author_line = trailers::identity_line(
            atom.metadata.author.as_ref(),
            &mut supplied,
            &mut altered,
            "author",
        );
        let author_when = trailers::when(
            atom.metadata.author_time,
            &mut supplied,
            "author-time",
            "author-time-zone",
        );
        // The one-claim rule (RFC 011): no distinct committer means the source drew no distinction, so
        // Git's required committer line is supplied from the author's own claim, marked (Git's format
        // forces a value the IR itself did not separately state).
        let (committer_line, committer_when) =
            match (&atom.metadata.committer, atom.metadata.commit_time) {
                (None, None) => {
                    supplied.mark("committer");
                    supplied.mark("commit-time");
                    (author_line.clone(), author_when.clone())
                }
                (committer, commit_time) => {
                    let committer_line = if committer.is_some() {
                        trailers::identity_line(
                            committer.as_ref(),
                            &mut supplied,
                            &mut altered,
                            "committer",
                        )
                    } else {
                        supplied.mark("committer");
                        author_line.clone()
                    };
                    let committer_when = if commit_time.is_some() {
                        trailers::when(
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

        let source_id = trailers::source_id_text(&atom.source.atom_id);
        let mut trailer_lines = vec![
            format!("Brygge-Encoded: brygge {brygge_version} from {source_label}"),
            format!("Brygge-Source-Id: {source_id}"),
        ];
        if let Some(line) = supplied.trailer() {
            trailer_lines.push(line);
        }
        if let Some(line) = altered.trailer() {
            trailer_lines.push(line);
        }
        trailer_lines.extend(trailers::status_trailers(atom, &ir.transforms));
        trailer_lines.push(format!("Brygge-Artifact: sha256:{digest_hex}"));

        fold_supplied(&mut summary, &supplied);
        fold_altered(&mut summary, &altered);

        let mut body = message;
        if !body.ends_with(b"\n") {
            body.push(b'\n');
        }
        body.push(b'\n');
        body.extend(trailer_lines.join("\n").into_bytes());

        let mark = next_mark;
        next_mark += 1;
        atom_marks.insert(atom.id, mark);

        writeln!(out, "commit {SCRATCH_REF}")?;
        writeln!(out, "mark :{mark}")?;
        writeln!(out, "author {author_line} {author_when}")?;
        writeln!(out, "committer {committer_line} {committer_when}")?;
        // The message's own declared encoding, when the source stated one (`Text.encoding`), is written
        // as `git-fast-import(1)`'s own `encoding` command: the bytes below reach Git exactly as the
        // source gave them, tagged with what they are, never re-encoded.
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
            let parent_mark = atom_marks.get(first).copied().ok_or_else(|| {
                std::io::Error::other(format!(
                    "atom {first} is not yet encoded (not canonically ordered)"
                ))
            })?;
            writeln!(out, "from :{parent_mark}")?;
        }
        for other in parents {
            let parent_mark = atom_marks.get(other).copied().ok_or_else(|| {
                std::io::Error::other(format!(
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
                    #[allow(clippy::expect_used)]
                    let blob_mark = *blob_marks.get(blob).expect(
                        "every blob an op references was just emitted above, in this same atom",
                    );
                    writeln!(
                        out,
                        "M {} :{blob_mark} {}",
                        git_mode_str(*mode),
                        trailers::path_token(path)
                    )?;
                }
                PathOp::Delete { path, .. } => {
                    writeln!(out, "D {}", trailers::path_token(path))?;
                }
            }
        }
        summary.commits += 1;
    }

    if !ir.atoms.is_empty() {
        for rf in &ir.refs {
            #[allow(clippy::expect_used)]
            let mark = *atom_marks
                .get(&rf.target)
                .expect("verify already confirmed every ref targets a written atom");
            let is_tag = write_ref(out, rf, mark, &ir.transforms, &ctx, &mut summary)?;
            if is_tag {
                summary.tags += 1;
            }
            summary.refs += 1;
        }
        writeln!(out, "reset {SCRATCH_REF}")?;
    }

    // The CLI runs `git fast-import --done` (RFC 019 §1): that flag makes Git itself refuse a stream
    // that ends without this exact marker, so a truncated write (brygge killed, the pipe broken) is
    // caught by Git, not silently accepted as a shorter-than-intended import. Written last, after
    // everything else has gone out with no error.
    writeln!(out, "done")?;

    Ok(summary)
}

/// Writes one ref. Returns `true` if it was written as a tag (annotated or lightweight).
fn write_ref<W: Write>(
    out: &mut W,
    rf: &RefRecord,
    mark: u64,
    transforms: &[TransformRecord],
    ctx: &Context<'_>,
    summary: &mut Summary,
) -> std::io::Result<bool> {
    let transform_lines = if matches!(rf.status, brygge_ir::EpistemicStatus::Transformed { .. }) {
        trailers::status_chain_trailers(&format!("ref:{}", rf.name), &rf.status, transforms)
    } else {
        Vec::new()
    };
    match (&rf.kind, &rf.annotation) {
        (RefKind::Tag, Some(annotation)) => {
            let mut supplied = Supplied::default();
            let mut altered = Altered::default();
            let tagger_line = trailers::identity_line(
                annotation.tagger.as_ref(),
                &mut supplied,
                &mut altered,
                "tagger",
            );
            let tagger_when =
                trailers::when(annotation.time, &mut supplied, "tag-time", "tag-time-zone");
            let mut message = annotation
                .message
                .as_ref()
                .map(|t| t.bytes.clone())
                .unwrap_or_default();
            // The same block as a commit's (RFC 019 D-2), minus `Brygge-Source-Id` (a ref has no atom):
            // `Brygge-Encoded` and `Brygge-Artifact` always; supplied/altered/transform lines only when
            // they apply.
            let mut trailer_lines = vec![format!(
                "Brygge-Encoded: brygge {} from {}",
                ctx.brygge_version, ctx.source_label
            )];
            if let Some(line) = supplied.trailer() {
                trailer_lines.push(line);
            }
            if let Some(line) = altered.trailer() {
                trailer_lines.push(line);
            }
            trailer_lines.extend(transform_lines);
            trailer_lines.push(format!("Brygge-Artifact: sha256:{}", ctx.digest_hex));
            if !message.ends_with(b"\n") {
                message.push(b'\n');
            }
            message.push(b'\n');
            message.extend(trailer_lines.join("\n").into_bytes());
            fold_supplied(summary, &supplied);
            fold_altered(summary, &altered);
            writeln!(out, "tag {}", rf.name)?;
            writeln!(out, "from :{mark}")?;
            writeln!(out, "tagger {tagger_line} {tagger_when}")?;
            writeln!(out, "data {}", message.len())?;
            out.write_all(&message)?;
            writeln!(out)?;
            Ok(true)
        }
        (RefKind::Tag, None) => {
            // A lightweight tag has no message of its own: a Transformed mark here goes to the report
            // (RFC 019 D-2's change from the retired example, which printed it to stderr instead).
            for line in &transform_lines {
                summary
                    .unplaced_ref_transforms
                    .push(format!("ref {:?}: {line}", rf.name));
            }
            writeln!(out, "reset refs/tags/{}", rf.name)?;
            writeln!(out, "from :{mark}")?;
            Ok(true)
        }
        _ => {
            // Branch, Bookmark, NamedBranch and Other (RFC 019 D-3) — none has a message of its own
            // either.
            for line in &transform_lines {
                summary
                    .unplaced_ref_transforms
                    .push(format!("ref {:?}: {line}", rf.name));
            }
            let path = git_ref_path(rf);
            writeln!(out, "reset {path}")?;
            writeln!(out, "from :{mark}")?;
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests;

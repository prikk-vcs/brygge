//! The command implementations for the three-verb surface (handoff `cli-and-verify-handoff-v2.md`,
//! adapted to IR contract 0.2.0 by the RFC 011 handoff). Each returns a CL-08 exit code and prints its
//! own output; `main` maps the code to `std::process::exit`. Every string that can originate in a source
//! repository is routed through [`crate::display`] before it reaches stdout or stderr (CR-19).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::{Command as Subprocess, Stdio};

use brygge_decode_cvs::{Error as CvsError, Options as CvsOptions, Source as CvsSource};
use brygge_decode_git::Error as GitError;
use brygge_decode_hg::Error as HgError;
use brygge_decode_svn::{
    Error as SvnError, LayoutPolicy, Options as SvnOptions, Source as SvnSource,
};
use brygge_ir::model::{AtomId, ChangedPart, Content, FlagKind, PathOp, RefKind, Text};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};
use brygge_ir::{Decoded, Ir, LossClass};

use crate::authors;
use crate::cli::{AUTHORS_TRANSFORM, Format, GIT_TARGET, SourceKind};
use crate::display;
use crate::encode_git;
use crate::exit;
use crate::output;
use crate::plugin;

// RFC 019 D-6 (batch G2): `verify.against_target`[.detail] is a new machine key.
const VERIFY_VERSION: u32 = 5;
const INSPECT_VERSION: u32 = 4;
// RFC 019 D-4 (batch G3): `inspect --authors`'s own machine form, independent of `inspect`'s own report.
const INSPECT_AUTHORS_VERSION: u32 = 1;
// RFC 020 D-3 (batch L): `decode`'s own layout section (below `report_version`'s own lines, like
// `inspect_authors_version`'s own independence from `inspect_version`) — absent entirely from a decode
// that never names a layout (not svn, or svn without --reconstruct-refs/--branch-lines), so every
// existing decode's machine output is unchanged byte for byte.
const DECODE_SVN_LAYOUT_VERSION: u32 = 1;

// ---- small shared renderers -------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn short_hex(bytes: &[u8]) -> String {
    let h = hex(bytes);
    h.get(..12).unwrap_or(&h).to_string()
}

/// The fixed, lowercase label for a ref kind (never `{:?}` — CR-19: no Rust `Debug` output).
fn ref_kind_label(kind: &RefKind) -> &'static str {
    match kind {
        RefKind::Branch => "branch",
        RefKind::Tag => "tag",
        RefKind::Bookmark => "bookmark",
        RefKind::NamedBranch => "named-branch",
        RefKind::Other(_) => "other",
    }
}

/// The name inside a `RefKind::Other(name)`, for the machine form's companion key (`ref.N.kind_other`): the
/// label stays the fixed word `other`, and the name is carried separately rather than flattened away.
fn ref_kind_other(kind: &RefKind) -> Option<&str> {
    match kind {
        RefKind::Other(name) => Some(name),
        _ => None,
    }
}

/// The name inside a `DerivationKind::Other(name)` of a derived status, for the machine form's companion
/// key (`…status_other`).
fn status_other(s: &EpistemicStatus) -> Option<&str> {
    match s {
        EpistemicStatus::Derived(d) => match &d.kind {
            DerivationKind::Other(name) => Some(name),
            _ => None,
        },
        EpistemicStatus::Stated | EpistemicStatus::Transformed { .. } => None,
    }
}

/// The fixed, lowercase label for a loss class (never `{:?}`).
fn loss_class_label(class: LossClass) -> &'static str {
    match class {
        LossClass::Representation => "representation",
        LossClass::AdvisoryUnreliable => "advisory-unreliable",
        LossClass::Other => "other",
        LossClass::Transform => "transform",
    }
}

/// The fixed, lowercase label for a flag kind (never `{:?}`).
fn flag_kind_label(kind: FlagKind) -> &'static str {
    match kind {
        FlagKind::ConventionViolation => "convention-violation",
        FlagKind::BelowConfidenceFloor => "below-confidence-floor",
    }
}

/// `stated`, `derived:<kind>`, or `transformed:<step>` — machine-safe (every half is a fixed
/// Rust-enum-derived label or a plain integer, never source text). The chain's own prior/changed detail
/// is not repeated here (RFC 018 D-5): `inspect --atoms`'s machine form carries it, if wanted, through
/// the dedicated `transform.<k>.*` keys, not by growing this one key's own grammar.
fn status_label_machine(s: &EpistemicStatus) -> String {
    match s {
        EpistemicStatus::Stated => "stated".to_string(),
        EpistemicStatus::Derived(d) => format!("derived:{}", d.kind.label()),
        EpistemicStatus::Transformed { step, .. } => format!("transformed:{step}"),
    }
}

/// `stated` or `derived:<kind> {k=v, …}` with confidence where present (human form only — handoff
/// §3.4.2). Parameter values are neutralized; the kind label and parameter keys are fixed decoder-chosen
/// literals, never source text.
fn status_label_human(s: &EpistemicStatus) -> String {
    match s {
        EpistemicStatus::Stated => "stated".to_string(),
        EpistemicStatus::Transformed {
            step,
            changed,
            prior,
        } => {
            // RFC 018 D-5's own worked example: "transformed at 1 (author) ← stated" — a chain renders
            // as one arrow per prior step, oldest status last.
            let parts = changed
                .iter()
                .map(ChangedPart::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "transformed at {step} ({parts}) ← {}",
                status_label_human(prior)
            )
        }
        EpistemicStatus::Derived(d) => {
            let mut out = format!("derived:{}", d.kind.label());
            if d.params.is_empty() && d.confidence.is_none() {
                return out;
            }
            out.push_str(" {");
            let mut first = true;
            for (k, v) in &d.params {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                let _ = write!(out, "{k}={}", display::human(v));
            }
            if let Some(c) = d.confidence {
                if !first {
                    out.push_str(", ");
                }
                let _ = write!(out, "confidence={c}");
            }
            out.push('}');
            out
        }
    }
}

fn op_status(op: &PathOp) -> &EpistemicStatus {
    match op {
        PathOp::Add { status, .. }
        | PathOp::Modify { status, .. }
        | PathOp::Delete { status, .. }
        | PathOp::Replace { status, .. } => status,
    }
}

/// The first line of a `Text` message (empty when there is none or it is not valid UTF-8).
fn message_subject(message: Option<&Text>) -> String {
    message
        .and_then(Text::as_utf8)
        .and_then(|s| s.lines().next())
        .map(|s| display::human(s).into_owned())
        .unwrap_or_default()
}

/// Reads `path` through brygge-ir's streaming reader (RFC 015 D-4): the artifact is opened once and
/// read forward in one pass, never loaded whole into memory the way `std::fs::read` + `from_bytes`
/// used to (fish-shell's `inspect`/`verify`: 9.3 GiB peak before this, well under 1 GiB after).
fn read_ir(path: &Path) -> Result<Decoded, String> {
    brygge_ir::from_file(path).map_err(|e| format!("{}: {e}", path.display()))
}

// ---- provenance round-trip for `verify --against-source` -------------------------------------------

/// Whether the IR's provenance says its source was decoded with rename inference on.
fn infer_renames_from_provenance(ir: &Ir) -> bool {
    ir.provenance
        .params
        .get("infer_renames")
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// The `decode` source kind recorded in an IR's provenance, if this build has a decoder for it.
fn source_kind_of(ir: &Ir) -> Option<SourceKind> {
    match ir.provenance.source.kind {
        brygge_ir::SourceKind::Git => Some(SourceKind::Git),
        brygge_ir::SourceKind::Hg => Some(SourceKind::Hg),
        brygge_ir::SourceKind::Svn => Some(SourceKind::Svn),
        brygge_ir::SourceKind::Cvs => Some(SourceKind::Cvs),
        brygge_ir::SourceKind::Other(_) => None,
    }
}

/// The CVS clustering window recorded in the IR's provenance (default if absent/unparsable).
fn cvs_window_from_provenance(ir: &Ir) -> u64 {
    ir.provenance
        .params
        .get("window_secs")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| CvsOptions::default().window_secs)
}

/// The CVS confidence floor recorded in the IR's provenance (default if absent/unparsable).
fn cvs_floor_from_provenance(ir: &Ir) -> u8 {
    ir.provenance
        .params
        .get("confidence_floor")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| CvsOptions::default().confidence_floor)
}

/// Whether the IR's provenance says its SVN source was decoded with ref reconstruction on.
fn reconstruct_refs_from_provenance(ir: &Ir) -> bool {
    ir.provenance
        .params
        .get("reconstruct_refs")
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// Whether the IR's provenance says its SVN source was decoded with `--branch-lines`.
fn branch_lines_from_provenance(ir: &Ir) -> bool {
    ir.provenance
        .params
        .get("branch_lines")
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// The SVN layout policy recorded in the IR's provenance: `Ok` with the default if the `layout` param is
/// absent (an older artifact, or a non-SVN decode, where it never existed); `Ok` with the parsed policy
/// if present and readable; `Err` with the raw label if present but [`LayoutPolicy::from_label`] cannot
/// read it — never a silent fallback to the default for a label that *is* present (RFC 020 D-3 §2): the
/// caller decides what to do with an unreadable one (`verify --against-source` reports `not-checked`,
/// naming the label, rather than guessing).
fn layout_from_provenance(ir: &Ir) -> Result<LayoutPolicy, String> {
    match ir.provenance.params.get("layout") {
        None => Ok(LayoutPolicy::default()),
        Some(label) => LayoutPolicy::from_label(label).ok_or_else(|| label.clone()),
    }
}

/// The per-source knobs a decode needs: git uses `infer_renames`/`carry_extra_namespaces`; svn uses
/// `reconstruct_refs`/`layout`/`branch_lines`; cvs uses `reconstruct_refs`/`cvs_window`/`cvs_floor`.
/// Built from CLI flags (decode) or from the artifact's provenance (against-source, so a re-decode
/// reproduces the recorded import) — `carry_extra_namespaces` is the one field never driven by either:
/// it is never a CLI flag or a provenance param, only set by RFC 019 D-6's own check (below), which
/// needs the Git decoder to see `refs/brygge/other/*` too, to compare a `--branch-lines` dead line's
/// commits — nothing an ordinary `decode git` or `--against-source` re-derivation should carry.
struct SourceOpts {
    infer_renames: bool,
    reconstruct_refs: bool,
    branch_lines: bool,
    carry_extra_namespaces: Vec<String>,
    layout: LayoutPolicy,
    cvs_window: u64,
    cvs_floor: u8,
}

/// Runs `f`, converting an unexpected panic into a typed fault rather than an unclassified crash
/// (CR-16). Defense in depth only — it does not replace fixing a panic at its source; it exists so a
/// panic inside a decoder or a decoder dependency (e.g. gix) is still a documented outcome (exit
/// `FAILURE`), never a bare process abort (exit 101, not a CL-08 class). Relies on unwinding: the
/// workspace must not set `panic = "abort"` without revisiting this.
///
/// `AssertUnwindSafe`: `f` closes only over by-value/by-shared-reference decode inputs (a path, an
/// owned options struct) and produces an owned `Result`; nothing it captures is shared mutable state a
/// caller could observe half-mutated after a caught panic. That makes asserting unwind-safety sound here,
/// even though the closure's captured types are not provably `UnwindSafe` to the compiler in general
/// (e.g. a decoder's internal repository handle may use interior mutability the compiler cannot see
/// through the crate boundary).
fn guard_decoder<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|payload| {
        let detail = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned());
        match detail {
            Some(msg) => {
                format!("internal decoder fault: {msg} — this is a brygge bug; please report it")
            }
            None => "internal decoder fault — this is a brygge bug; please report it".to_string(),
        }
    })
}

/// The message every decoder's `ResourceLimit { what, ceiling }` maps to (RFC 010 §2.6, 2026-09-23
/// review 005 R-2): one place, shared by all four sources, and identical to each decoder's own `Display`
/// (a user reads the same sentence whichever way the error reaches them).
fn resource_limit_message(what: &str, ceiling: &str) -> String {
    format!("refused: {what} exceeds brygge's ceiling ({ceiling})")
}

fn map_cvs_error(e: CvsError) -> (i32, String) {
    match e {
        CvsError::FloorRefusal { feature, reason } => (
            exit::FLOOR_REFUSAL,
            format!("refused CVS feature '{feature}' below the floor: {reason}"),
        ),
        CvsError::ResourceLimit { what, ceiling } => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        // 0.8.1: `IrBuilder::add_atom`'s own ceiling (`max_ir_ops`), wrapped here like every other
        // decoder's native `ResourceLimit` — the builder's ceiling holds for every decoder.
        CvsError::Ir(brygge_ir::Error::ResourceLimit { what, ceiling }) => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        other => (exit::FAILURE, format!("decode failed: {other}")),
    }
}

fn map_svn_error(e: SvnError) -> (i32, String) {
    match e {
        SvnError::FloorRefusal { feature, reason } => (
            exit::FLOOR_REFUSAL,
            format!("refused Subversion feature '{feature}' below the floor: {reason}"),
        ),
        SvnError::UnsupportedFormat { what, reason } => (
            exit::FLOOR_REFUSAL,
            format!("unsupported Subversion dump form '{what}': {reason}"),
        ),
        SvnError::ResourceLimit { what, ceiling } => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        // 0.8.1: as CVS's own arm above.
        SvnError::Ir(brygge_ir::Error::ResourceLimit { what, ceiling }) => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        e @ SvnError::BranchLinesNoLayout => (exit::FLOOR_REFUSAL, e.to_string()),
        other => (exit::FAILURE, format!("decode failed: {other}")),
    }
}

fn map_git_error(e: GitError) -> (i32, String) {
    match e {
        GitError::FloorRefusal { feature, reason } => (
            exit::FLOOR_REFUSAL,
            format!("refused Git feature '{feature}' below the floor: {reason}"),
        ),
        GitError::ResourceLimit { what, ceiling } => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        // 0.8.1: as CVS's own arm above.
        GitError::Ir(brygge_ir::Error::ResourceLimit { what, ceiling }) => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        other => (exit::FAILURE, format!("decode failed: {other}")),
    }
}

fn map_hg_error(e: HgError) -> (i32, String) {
    match e {
        HgError::FloorRefusal { feature, reason } => (
            exit::FLOOR_REFUSAL,
            format!("refused Mercurial feature '{feature}' below the floor: {reason}"),
        ),
        HgError::UnsupportedFormat {
            requirement,
            reason,
        } => (
            exit::FLOOR_REFUSAL,
            format!("unsupported Mercurial format '{requirement}': {reason}"),
        ),
        HgError::ResourceLimit { what, ceiling } => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        // 0.8.1: as CVS's own arm above.
        HgError::Ir(brygge_ir::Error::ResourceLimit { what, ceiling }) => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        other => (exit::FAILURE, format!("decode failed: {other}")),
    }
}

/// `run_transform_authors`'s own `Edit::finish` error, mapped to a `(exit code, message)` exactly as a
/// decoder's `ResourceLimit` is above (review 159 C-2): the ceiling `IrBuilder::add_atom` raises from
/// inside `Edit::finish`'s own builder is the same kind of refusal `machine-output.md` §4's row for `20`
/// names, not a generic runtime failure, so it must give the same exit code here too.
fn map_transform_finish_err(e: brygge_ir::Error) -> (i32, String) {
    match e {
        brygge_ir::Error::ResourceLimit { what, ceiling } => {
            (exit::FLOOR_REFUSAL, resource_limit_message(&what, &ceiling))
        }
        other => (
            exit::FAILURE,
            format!("the transform's own output was refused: {other}"),
        ),
    }
}

/// Decode `repo` with the chosen source decoder, mapping decoder errors to a `(exit code, message)`.
/// A refused format/feature is a clean refusal (`FLOOR_REFUSAL`), not a generic failure. The decoder
/// invocation itself is panic-guarded (`guard_decoder`, CR-16) for all four sources, since this is the
/// single path both `decode` and `verify --against-source` use. `storage` (RFC 015 D-3) is never
/// recorded anywhere in the result; it only chooses where content lives while decoding.
fn decode_source(
    kind: SourceKind,
    repo: &Path,
    opts: &SourceOpts,
    storage: &brygge_ir::Storage,
) -> Result<Ir, (i32, String)> {
    match kind {
        SourceKind::Cvs => {
            let src = CvsSource::LocalRepo(repo.to_path_buf());
            let cvs = CvsOptions {
                window_secs: opts.cvs_window,
                confidence_floor: opts.cvs_floor,
                reconstruct_refs: opts.reconstruct_refs,
            };
            guard_decoder(|| brygge_decode_cvs::decode_with_storage(&src, &cvs, storage))
                .map_err(|fault| (exit::FAILURE, fault))?
                .map_err(map_cvs_error)
        }
        SourceKind::Svn => {
            // A directory is a repository (dumped read-only via `svnadmin dump`); a file is a dumpfile.
            let src = if repo.is_file() {
                SvnSource::DumpFile(repo.to_path_buf())
            } else {
                SvnSource::LocalRepo(repo.to_path_buf())
            };
            let svn = SvnOptions {
                reconstruct_refs: opts.reconstruct_refs,
                layout: opts.layout.clone(),
                branch_lines: opts.branch_lines,
            };
            guard_decoder(|| brygge_decode_svn::decode_with_storage(&src, &svn, storage))
                .map_err(|fault| (exit::FAILURE, fault))?
                .map_err(map_svn_error)
        }
        SourceKind::Git => {
            let git = brygge_decode_git::Options {
                infer_renames: opts.infer_renames,
                rename_threshold: 100,
                carry_extra_namespaces: opts.carry_extra_namespaces.clone(),
            };
            guard_decoder(|| brygge_decode_git::decode_with_storage(repo, &git, storage))
                .map_err(|fault| (exit::FAILURE, fault))?
                .map_err(map_git_error)
        }
        SourceKind::Hg => {
            // Mercurial records its renames, so there is nothing to infer and no inference option.
            let hg = brygge_decode_hg::Options::default();
            guard_decoder(|| brygge_decode_hg::decode_with_storage(repo, &hg, storage))
                .map_err(|fault| (exit::FAILURE, fault))?
                .map_err(map_hg_error)
        }
    }
}

// ---- decode (CL-01) ------------------------------------------------------------------------------

/// The before-the-run faithfulness statement (CR-21, FS-05/FS-06), printed to stderr before decoding
/// starts. One place, so a later addition (e.g. CVS's branch sentence) is one edit.
#[must_use]
pub fn faithfulness_statement(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Git => {
            "Git: content, history and messages are carried as the source recorded them. Renames are \
             inferred only with --infer-renames, and are then marked derived. Authorship is Unverifiable."
        }
        SourceKind::Hg => {
            "Mercurial: content, history and messages are carried as recorded, including renames the \
             source recorded. Authorship is Unverifiable. Secret and hidden (obsolete) changesets are \
             not imported: brygge imports what the repository would publish. Tags are carried as the \
             .hgtags file, not as refs."
        }
        SourceKind::Svn => {
            "Subversion: revisions are carried as recorded. Branches and tags are only a directory \
             convention; with --reconstruct-refs they are reconstructed and marked derived. Authorship is \
             Unverifiable."
        }
        SourceKind::Cvs => {
            "CVS has no atomic commits: every changeset is brygge's reconstruction (derived), and a \
             changeset cannot be checked against the source. File contents and per-file history are \
             carried as recorded. Authorship is Unverifiable. Branch history is imported only with \
             --reconstruct-refs (a branch is identified by its symbol name); otherwise the main line is."
        }
    }
}

/// Write a body atomically to `out`: a temp file in the same directory, written by `write_body`
/// (returning the number of bytes it wrote), flushed and `sync_all`'d, then renamed over `out`. The
/// temp file is created with `create_new`, so a pre-existing file or symlink at that path is an error —
/// never followed, never truncated, and never removed (it is not ours). On any later failure the temp
/// file we created is removed and `out` is untouched. `write_body` is handed the temp file directly
/// (not a buffer) so a caller can stream straight into it (RFC 015 D-2) rather than building the whole
/// body in memory first.
fn atomic_write(
    out: &Path,
    write_body: impl FnOnce(&mut std::fs::File) -> std::io::Result<u64>,
) -> std::io::Result<u64> {
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Built from the raw `OsStr` name, never a lossy conversion, so a non-UTF-8 output name still works.
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(out.file_name().unwrap_or_else(|| "artifact".as_ref()));
    tmp_name.push(format!(".brygge-tmp-{}", std::process::id()));
    let tmp = dir.join(tmp_name);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    let result = (|| -> std::io::Result<u64> {
        let len = write_body(&mut f)?;
        f.sync_all()?;
        std::fs::rename(&tmp, out)?;
        // The rename is atomic but not yet durable across a power loss until the directory entry
        // itself is synced. Best-effort: some platforms cannot open a directory as a file at all, and
        // the rename has already succeeded either way, so a failure here is not the write's failure.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(len)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `decode <git|hg|svn|cvs> <source> --out <artifact> [--spill-dir <DIR>]` (CL-01, FL-01/FL-10, RFC 015
/// D-3/D-3a). Content is always kept in a private spill file while decoding, never in memory: in
/// `spill_dir` if given, else `out`'s own directory. `spill_dir`, if given, must already exist and be a
/// directory — never created silently (a usage error, exit 2, otherwise).
#[allow(clippy::too_many_arguments)] // one parameter per `Command::Decode` field (cli.rs); no fewer
// would still say what each one means at the call site.
pub fn run_decode(
    kind: SourceKind,
    source: &Path,
    out: &Path,
    infer_renames: bool,
    reconstruct_refs: bool,
    branch_lines: bool,
    layout_trunk: Option<String>,
    layout_branches: Option<String>,
    layout_tags: Option<String>,
    spill_dir: Option<&Path>,
    format: Format,
) -> i32 {
    output::stderr(faithfulness_statement(kind));

    let dir = match spill_dir {
        Some(dir) => {
            if !dir.is_dir() {
                output::stderr(&format!(
                    "--spill-dir {} does not exist or is not a directory",
                    dir.display()
                ));
                return exit::USAGE;
            }
            dir.to_path_buf()
        }
        None => out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
    };
    let storage = brygge_ir::Storage::Spill(dir);

    // RFC 020 D-3: an option not given keeps its standard name; under `--layout-trunk .` (checked as a
    // usage error in `cli.rs` if `--layout-branches`/`--layout-tags` are also given), there is no
    // separate branches/tags container at all.
    let is_root = layout_trunk.as_deref() == Some(".");
    let layout = LayoutPolicy {
        trunk: layout_trunk.unwrap_or_else(|| "trunk".to_string()),
        branches: if is_root {
            None
        } else {
            Some(layout_branches.unwrap_or_else(|| "branches".to_string()))
        },
        tags: if is_root {
            None
        } else {
            Some(layout_tags.unwrap_or_else(|| "tags".to_string()))
        },
    };

    let default_cvs = brygge_decode_cvs::Options::default();
    let ir = match decode_source(
        kind,
        source,
        &SourceOpts {
            infer_renames,
            reconstruct_refs,
            branch_lines,
            carry_extra_namespaces: Vec::new(),
            layout: layout.clone(),
            cvs_window: default_cvs.window_secs,
            cvs_floor: default_cvs.confidence_floor,
        },
        &storage,
    ) {
        Ok(ir) => ir,
        Err((code, msg)) => {
            output::stderr(&display::human(&msg));
            return code;
        }
    };

    let written = atomic_write(out, |f| {
        let mut w = std::io::BufWriter::new(f);
        brygge_ir::artifact::write_to(&ir, &mut w).map_err(std::io::Error::other)?;
        w.flush()?;
        w.get_ref().metadata().map(|m| m.len())
    });
    let artifact_len = match written {
        Ok(len) => len,
        Err(e) => {
            output::stderr(&format!("cannot write {}: {e}", out.display()));
            return exit::FAILURE;
        }
    };
    output::stderr(&format!("wrote {} ({artifact_len} bytes)", out.display()));

    // The end-of-run report is `inspect`'s default report (FS-02): the exact same call.
    let report = brygge_ir::honesty::summary(&ir);
    let mut rendered = match format {
        Format::Human => report.render_human(),
        Format::Machine => report.render_machine(),
    };
    // RFC 020 D-3 §2: the decode report always names the layout used when reconstructing — appended
    // after the base report, exactly as `inspect`'s own transform/atom sections are (never folded into
    // `report_version`'s own fields, so a decode that never reconstructs is byte-for-byte unchanged).
    if kind == SourceKind::Svn && reconstruct_refs {
        rendered.push_str(&match format {
            Format::Human => render_svn_layout_human(&layout),
            Format::Machine => render_svn_layout_machine(&layout),
        });
    }
    if let Some(code) = output::stdout(&rendered) {
        return code;
    }
    fidelity_exit_class(&ir)
}

/// RFC 020 D-3 §2: the human-facing "what layout did this decode use" line, one line, whether the
/// values were given on the command line or are the standard ones.
///
/// Review 172 N-1: each value is quoted (`'…'`), the same way `cli.rs`'s own usage errors quote a
/// layout value — without it, a value containing `,` or `=` (the line's own separators) could be
/// misread as more fields than it is.
fn render_svn_layout_human(layout: &LayoutPolicy) -> String {
    if layout.trunk == "." {
        "layout: the whole repository as trunk\n".to_string()
    } else {
        let mut s = format!("layout: trunk='{}'", layout.trunk);
        if let Some(branches) = &layout.branches {
            let _ = write!(s, ", branches='{branches}'");
        }
        if let Some(tags) = &layout.tags {
            let _ = write!(s, ", tags='{tags}'");
        }
        s.push('\n');
        s
    }
}

/// RFC 020 D-3 §2: `decode.svn.layout.trunk`/`.branches`/`.tags` (the last two absent for the root form),
/// governed by its own [`DECODE_SVN_LAYOUT_VERSION`], independent of `report_version` — present only
/// when this function is actually called (svn, with `--reconstruct-refs`/`--branch-lines`), so every
/// other decode's machine output is unchanged.
fn render_svn_layout_machine(layout: &LayoutPolicy) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "decode_svn_layout_version={DECODE_SVN_LAYOUT_VERSION}");
    let _ = writeln!(
        s,
        "decode.svn.layout.trunk={}",
        display::machine_value(&layout.trunk)
    );
    if let Some(branches) = &layout.branches {
        let _ = writeln!(
            s,
            "decode.svn.layout.branches={}",
            display::machine_value(branches)
        );
    }
    if let Some(tags) = &layout.tags {
        let _ = writeln!(s, "decode.svn.layout.tags={}", display::machine_value(tags));
    }
    s
}

/// CL-08's fidelity exit class for `ir` alone: a flagged condition (RFC 011 D-8 — an unresolved SVN
/// layout, a CVS reconstruction under the confidence floor) takes precedence; else recorded loss if any
/// non-representation drop exists; else clean. `decode`'s own tail; `transform` (RFC 018 D-6 §3: "exit
/// with the fidelity class decode would give for the output artifact") reuses it rather than
/// re-deriving the same three-way rule a second time.
fn fidelity_exit_class(ir: &Ir) -> i32 {
    if !ir.flags.is_empty() {
        exit::CONVENTION_VIOLATION
    } else if ir
        .loss
        .dropped
        .iter()
        .any(|d| !matches!(d.class, LossClass::Representation))
    {
        exit::RECORDED_LOSS
    } else {
        exit::CLEAN
    }
}

// ---- inspect (CL-02) ------------------------------------------------------------------------------

/// `inspect <artifact> [--atoms]`: the default fidelity report (FS-02), with `--atoms` appending the
/// per-atom listing. Read-only; the artifact is read once.
pub fn run_inspect(artifact: &Path, atoms: bool, format: Format) -> i32 {
    let decoded = match read_ir(artifact) {
        Ok(d) => d,
        Err(e) => {
            output::stderr(&display::human(&e));
            return exit::FAILURE;
        }
    };
    if decoded.skipped_non_critical_fields > 0 {
        output::stderr(&format!(
            "note: {} field(s) from a newer contract were not understood and were skipped (none of \
             them can change what this artifact claims)",
            decoded.skipped_non_critical_fields
        ));
    }
    let ir = &decoded.ir;
    let report = brygge_ir::honesty::summary(ir)
        .with_skipped_non_critical_fields(decoded.skipped_non_critical_fields);
    let mut rendered = match format {
        Format::Human => report.render_human(),
        Format::Machine => report.render_machine(),
    };
    rendered.push_str(&match format {
        Format::Human => render_transforms_human(ir, atoms),
        Format::Machine => render_transforms_machine(ir),
    });
    if atoms {
        rendered.push_str(&match format {
            Format::Human => render_atoms_human(ir, decoded.contract),
            Format::Machine => render_atoms_machine(ir, decoded.contract),
        });
    }
    if let Some(code) = output::stdout(&rendered) {
        return code;
    }
    exit::CLEAN
}

/// `inspect --authors` (RFC 019 D-4 §1): every distinct identity the artifact carries, already in
/// map-file form — redirect it to a file and fill in each right-hand side for `transform authors`.
/// Replaces the default fidelity report entirely; unlike `inspect`'s own report, there is nothing to
/// decode beyond the artifact itself, so a read failure here is the only way this can fail.
pub fn run_inspect_authors(artifact: &Path, format: Format) -> i32 {
    let decoded = match read_ir(artifact) {
        Ok(d) => d,
        Err(e) => {
            output::stderr(&display::human(&e));
            return exit::FAILURE;
        }
    };
    let identities = authors::collect_identities(&decoded.ir);
    let rendered = match format {
        Format::Human => render_inspect_authors_human(&identities),
        Format::Machine => render_inspect_authors_machine(&identities),
    };
    if let Some(code) = output::stdout(&rendered) {
        return code;
    }
    exit::CLEAN
}

fn render_inspect_authors_human(identities: &[authors::IdentityCount]) -> String {
    let mut s = String::new();
    let n = identities.len();
    let _ = writeln!(
        s,
        "# brygge inspect --authors: {n} identit{}; fill in each right-hand side, delete lines you do \
         not want to map",
        if n == 1 { "y" } else { "ies" }
    );
    let prefixes: Vec<String> = identities
        .iter()
        .map(|ic| {
            format!(
                "{} =",
                display::human(&authors::identity_as_map_source(&ic.identity))
            )
        })
        .collect();
    let max_len = prefixes
        .iter()
        .map(|p| p.chars().count())
        .max()
        .unwrap_or(0);
    for (ic, prefix) in identities.iter().zip(&prefixes) {
        let pad = (max_len + 2).saturating_sub(prefix.chars().count());
        let mut counts = Vec::new();
        if ic.author > 0 {
            counts.push(format!("{} author", ic.author));
        }
        if ic.committer > 0 {
            counts.push(format!("{} committer", ic.committer));
        }
        if ic.tagger > 0 {
            counts.push(format!("{} tagger", ic.tagger));
        }
        let _ = writeln!(s, "{prefix}{}# {}", " ".repeat(pad), counts.join(", "));
    }
    s
}

fn render_inspect_authors_machine(identities: &[authors::IdentityCount]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "inspect_authors_version={INSPECT_AUTHORS_VERSION}");
    for (i, ic) in identities.iter().enumerate() {
        let _ = writeln!(
            s,
            "authors.{i}.name={}",
            display::machine_bytes(&ic.identity.name.bytes)
        );
        if let Some(email) = &ic.identity.email {
            let _ = writeln!(
                s,
                "authors.{i}.email={}",
                display::machine_bytes(&email.bytes)
            );
        }
        let _ = writeln!(s, "authors.{i}.count.author={}", ic.author);
        let _ = writeln!(s, "authors.{i}.count.committer={}", ic.committer);
        let _ = writeln!(s, "authors.{i}.count.tagger={}", ic.tagger);
    }
    s
}

/// RFC 018 D-5, `inspect` §5: the transform chain's own overview — shown whenever the chain is
/// non-empty, regardless of `--atoms` (unlike [`render_atoms_human`], which only runs under it).
/// `honesty::summary`'s own section (§3) carries only aggregate counts by design (a pure function of
/// the `Ir`, reused by `decode`/`verify` too); this one is `inspect`-specific, with each step's own
/// name/version/consumed digest. `show_params` adds each step's own parameters (`--atoms`).
fn render_transforms_human(ir: &Ir, show_params: bool) -> String {
    if ir.transforms.is_empty() {
        return String::new();
    }
    let report = brygge_ir::honesty::summary(ir);
    let mut s = String::new();
    let _ = writeln!(s, "transforms: {}", ir.transforms.len());
    for (k, t) in ir.transforms.iter().enumerate() {
        let k64 = k as u64;
        let _ = writeln!(
            s,
            "  {k}. {} {} — consumed sha256:{}; changed {} atom(s), {} ref(s); dropped {}",
            display::human(&t.name),
            display::human(&t.version),
            hex(&t.consumed),
            report.transformed_atoms.get(&k64).copied().unwrap_or(0),
            report.transformed_refs.get(&k64).copied().unwrap_or(0),
            report.transformed_drops.get(&k64).copied().unwrap_or(0),
        );
        if show_params {
            for (pk, pv) in &t.params {
                let _ = writeln!(s, "      {pk}={}", display::human(pv));
            }
        }
    }
    s
}

/// [`render_transforms_human`]'s machine form (RFC 018 D-5, `inspect` §5): `transforms=<N>`, then one
/// `transform.<k>.*` group per step. No parameters section — the machine form names only the keys the
/// handoff specifies.
fn render_transforms_machine(ir: &Ir) -> String {
    if ir.transforms.is_empty() {
        return String::new();
    }
    let report = brygge_ir::honesty::summary(ir);
    let mut s = String::new();
    let _ = writeln!(s, "transforms={}", ir.transforms.len());
    for (k, t) in ir.transforms.iter().enumerate() {
        let k64 = k as u64;
        let _ = writeln!(s, "transform.{k}.name={}", display::machine_value(&t.name));
        let _ = writeln!(
            s,
            "transform.{k}.version={}",
            display::machine_value(&t.version)
        );
        let _ = writeln!(s, "transform.{k}.consumed={}", hex(&t.consumed));
        let _ = writeln!(
            s,
            "transform.{k}.changed_atoms={}",
            report.transformed_atoms.get(&k64).copied().unwrap_or(0)
        );
        let _ = writeln!(
            s,
            "transform.{k}.changed_refs={}",
            report.transformed_refs.get(&k64).copied().unwrap_or(0)
        );
        let _ = writeln!(
            s,
            "transform.{k}.dropped={}",
            report.transformed_drops.get(&k64).copied().unwrap_or(0)
        );
    }
    s
}

/// How many of `atom`'s own ops carry a submodule (RFC 021 D-4, batch G refinement): `--atoms` prints
/// counts, not individual ops (review 189), so this is the one number both `render_atoms_human` and
/// `render_atoms_machine` need, computed once rather than twice.
fn count_submodule_ops(atom: &brygge_ir::ChangeAtom) -> usize {
    atom.ops
        .iter()
        .filter(|op| matches!(op.content(), Some(Content::Submodule { .. })))
        .count()
}

fn render_atoms_human(ir: &Ir, contract: brygge_ir::ContractVersion) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "IR (contract {}): {} atom(s), {} ref(s), {} blob(s)",
        contract,
        ir.atoms.len(),
        ir.refs.len(),
        ir.content.len()
    );
    if contract.minor == 2 {
        let _ = writeln!(
            s,
            "note: contract 0.2.0: an older contract; this build reads it, and writes {}",
            brygge_ir::version::CURRENT
        );
    }
    let _ = writeln!(s, "atoms (topological order):");
    for atom in &ir.atoms {
        let subject = message_subject(atom.metadata.message.as_ref());
        let submodule_ops = count_submodule_ops(atom);
        let submodule_suffix = if submodule_ops > 0 {
            format!(", {submodule_ops} submodule")
        } else {
            String::new()
        };
        let _ = writeln!(
            s,
            "  {}  [{}]  src:{}  {} op(s){submodule_suffix}  {}",
            short_hex(&atom.id.0),
            status_label_human(&atom.status),
            short_hex(&atom.source.atom_id),
            atom.ops.len(),
            subject,
        );
        for copy in &atom.copies {
            let label = if atom.is_move(copy) { "move" } else { "copy" };
            let _ = writeln!(
                s,
                "      {label} {}@{} -> {} [{}]",
                display::human(&copy.from),
                short_hex(&copy.from_atom.0),
                display::human(&copy.to),
                status_label_human(&copy.status)
            );
        }
        if !atom.source.signatures.is_empty() || !atom.source.extras.is_empty() {
            for sig in &atom.source.signatures {
                let _ = writeln!(
                    s,
                    "      signature {}: {} byte(s)",
                    display::human(&sig.label),
                    sig.bytes.len()
                );
            }
            for extra in &atom.source.extras {
                let _ = writeln!(
                    s,
                    "      extra {}: {} byte(s)",
                    display::human(&extra.label),
                    extra.bytes.len()
                );
            }
        }
    }
    if !ir.refs.is_empty() {
        let _ = writeln!(s, "refs:");
        for rf in &ir.refs {
            let _ = writeln!(
                s,
                "  {} ({}) -> {}  [{}]",
                display::human(&rf.name),
                ref_kind_label(&rf.kind),
                short_hex(&rf.target.0),
                status_label_human(&rf.status)
            );
        }
    }
    let _ = writeln!(s, "loss boundary:");
    if ir.loss.dropped.is_empty() {
        let _ = writeln!(s, "  (nothing dropped)");
    } else {
        for drop in &ir.loss.dropped {
            let _ = writeln!(
                s,
                "  [{}] {} — {}",
                loss_class_label(drop.class),
                display::human(&drop.what),
                display::human(&drop.reason)
            );
        }
    }
    let _ = writeln!(s, "flags:");
    if ir.flags.is_empty() {
        let _ = writeln!(s, "  (none)");
    } else {
        for flag in &ir.flags {
            let _ = writeln!(
                s,
                "  [{}] {} x{} — {}",
                flag_kind_label(flag.kind),
                display::human(&flag.what),
                flag.count,
                display::human(&flag.reason)
            );
        }
    }
    s
}

fn render_atoms_machine(ir: &Ir, contract: brygge_ir::ContractVersion) -> String {
    // `atoms=` is the fidelity report's line; this listing does not repeat it (one key, one line).
    // `contract_version` keeps its name from before RFC 018; its meaning is now the artifact's own
    // contract, not always this build's `CURRENT` (`docs/src/reference/machine-output.md`).
    let mut s = String::new();
    let _ = writeln!(s, "inspect_version={INSPECT_VERSION}");
    let _ = writeln!(s, "contract_version={contract}");
    for (i, atom) in ir.atoms.iter().enumerate() {
        let _ = writeln!(s, "atom.{i}.id={}", hex(&atom.id.0));
        let _ = writeln!(s, "atom.{i}.status={}", status_label_machine(&atom.status));
        if let Some(name) = status_other(&atom.status) {
            let _ = writeln!(s, "atom.{i}.status_other={}", display::machine_value(name));
        }
        let _ = writeln!(s, "atom.{i}.source_id={}", hex(&atom.source.atom_id));
        let _ = writeln!(s, "atom.{i}.ops={}", atom.ops.len());
        // RFC 021 D-4 (batch G refinement, review 189): `--atoms` prints counts, not individual ops, so
        // a submodule entry is this count alone — never a per-op `submodule <hex>` line.
        let _ = writeln!(s, "atom.{i}.submodule_ops={}", count_submodule_ops(atom));
        let _ = writeln!(s, "atom.{i}.parents={}", atom.parents.len());
        // The raw message bytes, percent-encoded once, like every other value: a consumer decodes to the
        // exact bytes (including non-UTF-8). Display escaping belongs to the human form only.
        let _ = writeln!(
            s,
            "atom.{i}.message={}",
            display::machine_bytes(
                atom.metadata
                    .message
                    .as_ref()
                    .map_or(&[][..], |m| m.bytes.as_slice())
            )
        );
        for (j, copy) in atom.copies.iter().enumerate() {
            let _ = writeln!(
                s,
                "atom.{i}.copy.{j}.from={}",
                display::machine_value(&copy.from)
            );
            let _ = writeln!(s, "atom.{i}.copy.{j}.from_atom={}", hex(&copy.from_atom.0));
            let _ = writeln!(
                s,
                "atom.{i}.copy.{j}.to={}",
                display::machine_value(&copy.to)
            );
            let _ = writeln!(
                s,
                "atom.{i}.copy.{j}.status={}",
                status_label_machine(&copy.status)
            );
            if let Some(name) = status_other(&copy.status) {
                let _ = writeln!(
                    s,
                    "atom.{i}.copy.{j}.status_other={}",
                    display::machine_value(name)
                );
            }
            let _ = writeln!(s, "atom.{i}.copy.{j}.move={}", atom.is_move(copy));
        }
    }
    for (i, rf) in ir.refs.iter().enumerate() {
        let _ = writeln!(s, "ref.{i}.name={}", display::machine_value(&rf.name));
        let _ = writeln!(s, "ref.{i}.kind={}", ref_kind_label(&rf.kind));
        if let Some(name) = ref_kind_other(&rf.kind) {
            let _ = writeln!(s, "ref.{i}.kind_other={}", display::machine_value(name));
        }
        let _ = writeln!(s, "ref.{i}.target={}", hex(&rf.target.0));
        let _ = writeln!(s, "ref.{i}.status={}", status_label_machine(&rf.status));
        if let Some(name) = status_other(&rf.status) {
            let _ = writeln!(s, "ref.{i}.status_other={}", display::machine_value(name));
        }
    }
    for (i, drop) in ir.loss.dropped.iter().enumerate() {
        let _ = writeln!(s, "loss.{i}.class={}", loss_class_label(drop.class));
        let _ = writeln!(s, "loss.{i}.what={}", display::machine_value(&drop.what));
    }
    for (i, flag) in ir.flags.iter().enumerate() {
        let _ = writeln!(s, "flag.{i}.kind={}", flag_kind_label(flag.kind));
        let _ = writeln!(s, "flag.{i}.what={}", display::machine_value(&flag.what));
        let _ = writeln!(s, "flag.{i}.count={}", flag.count);
    }
    s
}

// ---- verify (CL-04, CR-02) -------------------------------------------------------------------------

/// The outcome of one internal check: it can always fail, and `not-checked` is reserved for a check that
/// could not run (and says why) — never a way to hide a failure. It is the same word `against_source`
/// uses for "requested but could not run".
#[derive(Debug)]
enum CheckOutcome {
    Pass,
    Fail(String),
    NotChecked(String),
}

impl CheckOutcome {
    fn label(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail(_) => "fail",
            Self::NotChecked(_) => "not-checked",
        }
    }
    fn detail(&self) -> Option<&str> {
        match self {
            Self::Pass => None,
            Self::Fail(d) | Self::NotChecked(d) => Some(d),
        }
    }
    fn is_fail(&self) -> bool {
        matches!(self, Self::Fail(_))
    }
}

/// `structure`: every parent id names an atom appearing earlier; atom ids are unique; every ref target
/// is an atom in the artifact; `(name, kind)` is unique among refs; at most one op per path per atom.
fn check_structure(ir: &Ir) -> CheckOutcome {
    let mut seen: HashSet<AtomId> = HashSet::new();
    for (i, atom) in ir.atoms.iter().enumerate() {
        for p in &atom.parents {
            if !seen.contains(p) {
                return CheckOutcome::Fail(format!(
                    "atom at position {i} has a parent id not appearing earlier in the artifact"
                ));
            }
        }
        if !seen.insert(atom.id) {
            return CheckOutcome::Fail(format!("duplicate atom id at position {i}"));
        }
        let mut paths: HashSet<&str> = HashSet::new();
        for op in &atom.ops {
            if !paths.insert(op.path()) {
                return CheckOutcome::Fail(format!(
                    "atom at position {i} has more than one op for one path"
                ));
            }
        }
    }
    for rf in &ir.refs {
        if !seen.contains(&rf.target) {
            return CheckOutcome::Fail(
                "a ref targets an atom not present in the artifact".to_string(),
            );
        }
    }
    let mut ref_keys: HashSet<(&str, &'static str)> = HashSet::new();
    for rf in &ir.refs {
        if !ref_keys.insert((rf.name.as_str(), ref_kind_label(&rf.kind))) {
            return CheckOutcome::Fail("duplicate ref (name, kind)".to_string());
        }
    }
    CheckOutcome::Pass
}

/// The bounded-memory bookkeeping [`replay_ir`] reports: the peak number of trees retained at once, and
/// how many were actually cloned (review 003 R-3: a linear history should clone zero).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ReplayStats {
    peak: usize,
    clones: usize,
}

/// What [`replay_ir`]'s tree tracks at each path (RFC 021 D-1): not the content itself (the replay never
/// needs it), only which of the two kinds is there, so a `Modify` across kinds can be told from one that
/// merely moves a blob's bytes or a submodule's pinned commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathKind {
    Blob,
    Submodule,
}

impl PathKind {
    fn of(content: &Content) -> Self {
        match content {
            Content::Blob { .. } => Self::Blob,
            Content::Submodule { .. } => Self::Submodule,
        }
    }
}

/// `replay`: replay each atom's ops against its first parent's tree (a root replays against the empty
/// tree). `Add` needs the path absent; `Modify`/`Delete`/`Replace` need it present. Every copy's `to` is
/// checked as part of its atom's own ops (it is always produced by an `Add`/`Modify`/`Replace` in that
/// same atom); every copy's `from` must be present in `from_atom`'s replayed tree (RFC 011 §4). Memory-
/// and time-bounded (review 003 R-3, extended by RFC 011's handoff): a tree's retention count covers
/// both first-parent references and copy `from_atom` references, decremented as each is consumed, so a
/// tree is dropped the moment nothing later still needs it — a linear history with no copies therefore
/// clones nothing at all.
fn replay_ir(ir: &Ir) -> (CheckOutcome, ReplayStats) {
    let mut refcount: HashMap<AtomId, usize> = HashMap::new();
    for atom in &ir.atoms {
        if let Some(p0) = atom.parents.first() {
            *refcount.entry(*p0).or_insert(0) += 1;
        }
        for copy in &atom.copies {
            *refcount.entry(copy.from_atom).or_insert(0) += 1;
        }
    }
    let mut trees: HashMap<AtomId, BTreeMap<String, PathKind>> = HashMap::new();
    let mut stats = ReplayStats::default();

    macro_rules! fail {
        ($msg:expr) => {
            return (CheckOutcome::Fail($msg.to_string()), stats)
        };
    }

    for atom in &ir.atoms {
        let p0 = atom.parents.first().copied();
        let mut tree = match p0 {
            Some(p) => {
                let remaining = refcount.get_mut(&p).map_or(0, |c| {
                    *c -= 1;
                    *c
                });
                if remaining == 0 {
                    match trees.remove(&p) {
                        Some(t) => t,
                        None => fail!(
                            "an atom's first-parent tree is unavailable (a structural inconsistency)"
                        ),
                    }
                } else {
                    match trees.get(&p) {
                        Some(t) => {
                            stats.clones += 1;
                            t.clone()
                        }
                        None => fail!(
                            "an atom's first-parent tree is unavailable (a structural inconsistency)"
                        ),
                    }
                }
            }
            None => BTreeMap::new(),
        };

        // Copies are checked against the tree state *before* this atom's own ops apply, since
        // `from_atom` is always an earlier atom, never this one (RFC 011 §2.5).
        for copy in &atom.copies {
            let source_tree_has_it = if Some(copy.from_atom) == p0 {
                tree.contains_key(&copy.from)
            } else {
                match trees.get(&copy.from_atom) {
                    Some(t) => t.contains_key(&copy.from),
                    None => {
                        fail!("a copy's from_atom tree is unavailable (a structural inconsistency)")
                    }
                }
            };
            if !source_tree_has_it {
                fail!("a copy's 'from' path is not present in from_atom's replayed tree");
            }
            // The first-parent reference (if `from_atom == p0`) was already consumed above, when this
            // atom's own starting tree was obtained; only a *non*-first-parent copy reference needs its
            // own decrement here.
            if Some(copy.from_atom) != p0 {
                if let Some(c) = refcount.get_mut(&copy.from_atom) {
                    *c -= 1;
                    if *c == 0 {
                        trees.remove(&copy.from_atom);
                    }
                }
            }
        }

        for op in &atom.ops {
            match op {
                PathOp::Add { path, content, .. } => {
                    if tree.insert(path.clone(), PathKind::of(content)).is_some() {
                        fail!("an Add op names an already-present path");
                    }
                }
                PathOp::Modify { path, content, .. } => match tree.get(path) {
                    None => fail!("a Modify/Replace op names an absent path"),
                    Some(&existing) if existing != PathKind::of(content) => fail!(
                        "a Modify op changes a path's kind (a blob and a submodule); a change of \
                         kind is a Replace"
                    ),
                    Some(_) => {}
                },
                PathOp::Replace { path, content, .. } => {
                    if !tree.contains_key(path) {
                        fail!("a Modify/Replace op names an absent path");
                    }
                    tree.insert(path.clone(), PathKind::of(content));
                }
                PathOp::Delete { path, .. } => {
                    if tree.remove(path).is_none() {
                        fail!("a Delete op names an absent path");
                    }
                }
            }
        }
        if refcount.get(&atom.id).copied().unwrap_or(0) > 0 {
            trees.insert(atom.id, tree);
        }
        stats.peak = stats.peak.max(trees.len());
    }
    (CheckOutcome::Pass, stats)
}

fn check_replay(ir: &Ir) -> CheckOutcome {
    replay_ir(ir).0
}

/// `derivations`: every `Derived` record has non-empty `by`/`decoder_version`, `confidence <= 100` if
/// present, and the parameters its kind requires (RFC 011 D-10's registry). `ReconstructedChangeset`
/// requires `date_rule` and `confidence_rule` from the CVS decoder's own corrections handoff onward
/// (review 008 R-8: a confidence without the rule that produced it cannot be reviewed).
fn check_derivations(ir: &Ir) -> CheckOutcome {
    fn check_one(d: &Derivation) -> Result<(), String> {
        if d.by.is_empty() {
            return Err("a derivation's 'by' is empty".to_string());
        }
        if d.decoder_version.is_empty() {
            return Err("a derivation's 'decoder_version' is empty".to_string());
        }
        if let Some(c) = d.confidence {
            if c > 100 {
                return Err(format!("a derivation's confidence {c} exceeds 100"));
            }
        }
        match &d.kind {
            DerivationKind::InferredRename => {
                for key in ["rename_algorithm", "rename_threshold"] {
                    if !d.params.contains_key(key) {
                        return Err(format!(
                            "an InferredRename derivation is missing required param '{key}'"
                        ));
                    }
                }
            }
            DerivationKind::ReconstructedChangeset => {
                for key in [
                    "window_secs",
                    "cluster_keys",
                    "date_rule",
                    "confidence_rule",
                ] {
                    if !d.params.contains_key(key) {
                        return Err(format!(
                            "a ReconstructedChangeset derivation is missing required param '{key}'"
                        ));
                    }
                }
            }
            DerivationKind::ReconstructedBranch => {
                if !d.params.contains_key("layout") && !d.params.contains_key("source") {
                    return Err(
                        "a ReconstructedBranch derivation has neither 'layout' nor 'source' param"
                            .to_string(),
                    );
                }
            }
            DerivationKind::InferredMerge
            | DerivationKind::NormalizedMetadata
            | DerivationKind::Other(_) => {}
        }
        Ok(())
    }
    fn check_status(s: &EpistemicStatus) -> Result<(), String> {
        match s {
            EpistemicStatus::Stated => Ok(()),
            EpistemicStatus::Derived(d) => check_one(d),
            // RFC 018 D-5: a transform's own mark never hides what lies beneath it — a `Derived` prior
            // (at any depth) is still held to its required parameters, never treated as `Stated`.
            EpistemicStatus::Transformed { prior, .. } => check_status(prior),
        }
    }
    for atom in &ir.atoms {
        if let Err(e) = check_status(&atom.status) {
            return CheckOutcome::Fail(e);
        }
        for op in &atom.ops {
            if let Err(e) = check_status(op_status(op)) {
                return CheckOutcome::Fail(e);
            }
        }
        for copy in &atom.copies {
            if let Err(e) = check_status(&copy.status) {
                return CheckOutcome::Fail(e);
            }
        }
    }
    for rf in &ir.refs {
        if let Err(e) = check_status(&rf.status) {
            return CheckOutcome::Fail(e);
        }
    }
    CheckOutcome::Pass
}

/// Strips away any `Transformed` layers, down to the status a source actually stated or a decoder
/// originally derived (RFC 018 D-5: "look through `Transformed` to its prior chain" — a transform's own
/// mark never changes what the source originally stated or a decoder originally derived).
fn innermost_status(s: &EpistemicStatus) -> &EpistemicStatus {
    let mut current = s;
    while let EpistemicStatus::Transformed { prior, .. } = current {
        current = prior;
    }
    current
}

/// `source-invariants`: per-source honesty invariants that must hold if nothing stripped the marking
/// that makes an import honest (C-3a).
fn check_source_invariants(ir: &Ir) -> CheckOutcome {
    let kind = &ir.provenance.source.kind;
    let decoder_name: &str = match kind {
        brygge_ir::SourceKind::Git => "brygge-decode-git",
        brygge_ir::SourceKind::Hg => "brygge-decode-hg",
        brygge_ir::SourceKind::Svn => "brygge-decode-svn",
        brygge_ir::SourceKind::Cvs => "brygge-decode-cvs",
        brygge_ir::SourceKind::Other(_) => {
            return CheckOutcome::NotChecked(
                "no invariants known for this source kind".to_string(),
            );
        }
    };
    if ir.provenance.decoder != decoder_name {
        return CheckOutcome::Fail(format!(
            "provenance.decoder is '{}', expected '{decoder_name}' for this source kind",
            ir.provenance.decoder
        ));
    }
    for atom in &ir.atoms {
        if &atom.source.kind != kind {
            return CheckOutcome::Fail(
                "an atom's source.kind does not match provenance.source.kind".to_string(),
            );
        }
    }
    match kind {
        brygge_ir::SourceKind::Cvs => {
            for atom in &ir.atoms {
                // A reconstructed changeset, or (RFC 013 D-3) a branch-point atom, which is brygge's
                // construction of a branch's tree at its cut (`Derived(ReconstructedBranch)`). Nothing in a
                // CVS import is ever `Stated` at the atom level.
                let ok = matches!(
                    innermost_status(&atom.status),
                    EpistemicStatus::Derived(d)
                        if d.kind == DerivationKind::ReconstructedChangeset
                            || d.kind == DerivationKind::ReconstructedBranch
                );
                if !ok {
                    return CheckOutcome::Fail(
                        "a CVS atom is not Derived(ReconstructedChangeset) or Derived(ReconstructedBranch)"
                            .to_string(),
                    );
                }
                if !atom.copies.is_empty() {
                    return CheckOutcome::Fail(
                        "a CVS atom carries a copy record; CVS records no renames".to_string(),
                    );
                }
            }
            for rf in &ir.refs {
                if !innermost_status(&rf.status).is_derived() {
                    return CheckOutcome::Fail("a CVS ref is not Derived".to_string());
                }
            }
        }
        brygge_ir::SourceKind::Svn => {
            // RFC 019 Amendment A: with `--branch-lines`, every atom is one root's own line, a
            // reconstructed judgment of which revisions belong to it — `Derived(ReconstructedBranch)`,
            // marked `line: "true"` — rather than the single whole-repository spine's `Stated` atoms.
            let branch_lines = ir
                .provenance
                .params
                .get("branch_lines")
                .map(|v| v == "true")
                .unwrap_or(false);
            for atom in &ir.atoms {
                let ok = if branch_lines {
                    matches!(
                        innermost_status(&atom.status),
                        EpistemicStatus::Derived(d)
                            if d.kind == DerivationKind::ReconstructedBranch
                                && d.params.get("line").map(String::as_str) == Some("true")
                    )
                } else {
                    !innermost_status(&atom.status).is_derived()
                };
                if !ok {
                    return CheckOutcome::Fail(if branch_lines {
                        "a --branch-lines SVN atom is not Derived(ReconstructedBranch, line=true)"
                            .to_string()
                    } else {
                        "an SVN atom is Derived; SVN's revision spine is always Stated".to_string()
                    });
                }
                if atom.parents.len() > 1 {
                    return CheckOutcome::Fail(
                        "an SVN atom has more than one parent; SVN has no DAG".to_string(),
                    );
                }
            }
            for rf in &ir.refs {
                let ok = matches!(
                    innermost_status(&rf.status),
                    EpistemicStatus::Derived(d) if d.kind == DerivationKind::ReconstructedBranch
                );
                if !ok {
                    return CheckOutcome::Fail(
                        "an SVN ref is not Derived(ReconstructedBranch)".to_string(),
                    );
                }
            }
        }
        brygge_ir::SourceKind::Git => {
            for atom in &ir.atoms {
                for copy in &atom.copies {
                    if !innermost_status(&copy.status).is_derived() {
                        return CheckOutcome::Fail(
                            "a Git copy record is Stated; Git never states a rename".to_string(),
                        );
                    }
                }
            }
        }
        brygge_ir::SourceKind::Hg => {
            for atom in &ir.atoms {
                if atom.parents.len() > 2 {
                    return CheckOutcome::Fail("an hg atom has more than two parents".to_string());
                }
            }
        }
        brygge_ir::SourceKind::Other(_) => unreachable!("handled above"),
    }
    CheckOutcome::Pass
}

/// `provenance`: `decoder`, `decoder_version` and `brygge_version` are non-empty.
fn check_provenance(ir: &Ir) -> CheckOutcome {
    if ir.provenance.decoder.is_empty() {
        return CheckOutcome::Fail("provenance.decoder is empty".to_string());
    }
    if ir.provenance.decoder_version.is_empty() {
        return CheckOutcome::Fail("provenance.decoder_version is empty".to_string());
    }
    if ir.provenance.brygge_version.is_empty() {
        return CheckOutcome::Fail("provenance.brygge_version is empty".to_string());
    }
    CheckOutcome::Pass
}

/// `loss-boundary`: every drop record has a non-empty `what` and `reason`.
fn check_loss_boundary(ir: &Ir) -> CheckOutcome {
    for drop in &ir.loss.dropped {
        if drop.what.is_empty() {
            return CheckOutcome::Fail(
                "a loss-boundary drop record has an empty 'what'".to_string(),
            );
        }
        if drop.reason.is_empty() {
            return CheckOutcome::Fail(
                "a loss-boundary drop record has an empty 'reason'".to_string(),
            );
        }
    }
    CheckOutcome::Pass
}

/// The result of the optional `--against-source` re-derivation (VF-2), independent of the internal
/// checks (VF-4): the two claims are never merged into one verdict. `NotChecked` (review 003 R-1) is
/// distinct from a genuine mismatch: "could not be checked" is not "does not correspond" — the latter is
/// a claim of tampering, the former a claim that nothing was compared at all.
#[derive(Debug)]
enum AgainstSourceOutcome {
    /// `Some(note)` carries an informational note even on success (e.g. CR-07.6: the two decodes used
    /// different `svnadmin` versions, or different brygge versions, but the history is identical) —
    /// never a reason to doubt the result.
    Corresponds(Option<String>),
    DoesNotCorrespond(String),
    /// As [`Self::Corresponds`], CVS's own "reproduces" wording.
    Reproduces(Option<String>),
    DoesNotReproduce(String),
    /// RFC 018 D-5: an artifact with a transform chain is never compared by whole-`Ir` equality — only
    /// its chain's own *root* (`transforms[0].consumed`) can ever correspond to a fresh decode, since
    /// nothing re-runs the chain itself. The `String` is the "`N` transform(s) after it were not
    /// re-run" note.
    RootCorresponds(String),
    /// As [`Self::RootCorresponds`], CVS's own "reproduces" wording.
    RootReproduces(String),
    NotChecked(String),
    NotRun,
}

impl AgainstSourceOutcome {
    fn machine_label(&self) -> &'static str {
        match self {
            Self::Corresponds(_) => "corresponds",
            Self::DoesNotCorrespond(_) => "does-not-correspond",
            Self::Reproduces(_) => "reproduces",
            Self::DoesNotReproduce(_) => "does-not-reproduce",
            Self::RootCorresponds(_) => "root-corresponds",
            Self::RootReproduces(_) => "root-reproduces",
            Self::NotChecked(_) => "not-checked",
            Self::NotRun => "not-run",
        }
    }
    fn human_label(&self) -> &'static str {
        match self {
            Self::Corresponds(_) => "corresponds",
            Self::DoesNotCorrespond(_) => "does not correspond",
            Self::Reproduces(_) => "reproduces",
            Self::DoesNotReproduce(_) => "does not reproduce",
            Self::RootCorresponds(_) => "the chain's root corresponds to the source",
            Self::RootReproduces(_) => "the chain's root reproduces the source",
            Self::NotChecked(_) => "could not be checked",
            Self::NotRun => "not run",
        }
    }
    fn detail(&self) -> Option<&str> {
        match self {
            Self::DoesNotCorrespond(d) | Self::DoesNotReproduce(d) | Self::NotChecked(d) => Some(d),
            _ => None,
        }
    }
    /// An informational note on a *successful* comparison (review 010 F-4): shown under its own key,
    /// never as a failure `detail`, so a consumer cannot read "corresponds" plus a detail as a problem.
    /// Also carries [`Self::RootCorresponds`]/[`Self::RootReproduces`]'s own "not re-run" count (RFC 018
    /// D-5) — that, too, is informational, not a reason to doubt the result.
    fn note(&self) -> Option<&str> {
        match self {
            Self::Corresponds(Some(n))
            | Self::Reproduces(Some(n))
            | Self::RootCorresponds(n)
            | Self::RootReproduces(n) => Some(n),
            _ => None,
        }
    }
    /// A genuine mismatch: a real check ran and found a divergence. Distinct from [`Self::NotChecked`],
    /// where no comparison happened at all (review 003 R-1).
    fn is_mismatch(&self) -> bool {
        matches!(self, Self::DoesNotCorrespond(_) | Self::DoesNotReproduce(_))
    }
    fn is_not_checked(&self) -> bool {
        matches!(self, Self::NotChecked(_))
    }
}

/// Re-derive `ir1` from `repo` using its own recorded parameters and compare (VF-2), without trusting
/// the earlier run. CVS has no atomic source atom (SRC-C3), so its claim is `reproduces` (per-file
/// content + deterministic reproduction), never `corresponds` (changeset-level correspondence, which
/// does not exist to check). A source that cannot be re-decoded at all is `NotChecked`, never a claimed
/// mismatch (review 003 R-1).
fn run_against_source(ir1: &Ir, repo: &Path, storage: &brygge_ir::Storage) -> AgainstSourceOutcome {
    let Some(kind) = source_kind_of(ir1) else {
        return AgainstSourceOutcome::NotChecked(
            "against-source verify is not implemented for this IR's source kind".to_string(),
        );
    };
    // CR-07.6 (SVN): a dumpfile and a live repository are different forms of the same history; comparing
    // across forms is not a meaningful check (a live repository's `svnadmin dump` can differ from a
    // hand-supplied dumpfile in ways that carry no signal about the artifact's own correctness).
    if kind == SourceKind::Svn {
        if let Some(mismatch) = svn_source_form_mismatch(ir1, repo) {
            return AgainstSourceOutcome::NotChecked(mismatch);
        }
    }
    // RFC 020 D-3 §2: a `layout` param that is present but unreadable is reported `not-checked`, naming
    // the label — never silently re-derived against the default, which would compare against the wrong
    // convention and risk a false mismatch.
    let layout = match layout_from_provenance(ir1) {
        Ok(l) => l,
        Err(label) => {
            return AgainstSourceOutcome::NotChecked(format!(
                "the recorded layout ('{label}') could not be parsed"
            ));
        }
    };
    let opts = SourceOpts {
        // Only Git infers renames; no other artifact records the parameter, so none is read.
        infer_renames: kind == SourceKind::Git && infer_renames_from_provenance(ir1),
        reconstruct_refs: reconstruct_refs_from_provenance(ir1),
        branch_lines: kind == SourceKind::Svn && branch_lines_from_provenance(ir1),
        // Re-deriving the *source*, never a Git target: the recorded import never carried this.
        carry_extra_namespaces: Vec::new(),
        layout,
        cvs_window: cvs_window_from_provenance(ir1),
        cvs_floor: cvs_floor_from_provenance(ir1),
    };
    let is_cvs = kind == SourceKind::Cvs;
    // RFC 015 D-4: the re-derived `ir2` spills too, as `decode`'s own always-spill does — `ir1 == &ir2`
    // only ever compares blob id sets either way, never bytes, so which backing either side uses can
    // never change the verdict.
    let ir2 = match decode_source(kind, repo, &opts, storage) {
        Ok(ir) => ir,
        Err((_, msg)) => {
            return AgainstSourceOutcome::NotChecked(format!("cannot re-decode source: {msg}"));
        }
    };
    // CR-07.6, review 010 R-1: `svnadmin`'s own version is a fact about the tool, not the history, so an
    // upgrade between the two decodes must never by itself make this comparison fail. The version-mismatch
    // note (if any) is read from `ir2` before it is aligned to `ir1`'s version in place — `Ir` is not
    // `Clone` (RFC 015 D-3: its content store may be a spill file), so there is no separate "original"
    // copy to keep; the note already carries everything the original value was needed for.
    let versions_differ = svn_version_note(ir1, &ir2);
    let ir2 = align_svnadmin_version(ir1, ir2);
    // Review 140: a decoder's floor is a fact about the decoder, not the history — aligned the same way,
    // before anything compares `ir1` and `ir2`, so a floor change (071 added one to CVS's) can never by
    // itself make an otherwise-unchanged history read as tampering.
    let floor_differs = decoder_floor_note(ir1, &ir2);
    let ir2 = align_decoder_floor(ir1, ir2);

    // Review 106 V-1: which brygge *decoded* ir1 and ir2 is, like `svnadmin`'s own version, a fact
    // about the tool, never itself evidence of tampering (CR-07.6's pattern, generalized beyond
    // `svnadmin`) — until this fix, every artifact from an earlier brygge failed every later one's
    // `--against-source` with `does-not-correspond`, a false claim of tampering (review 003 R-1).
    // Captured before `ir2` is consumed by `restamped`, for the messages below.
    let this_build_brygge_version = ir2.provenance.brygge_version.clone();
    let brygge_versions_differ = ir1.provenance.brygge_version != this_build_brygge_version
        || ir1.provenance.decoder_version != ir2.provenance.decoder_version;
    let ir2 = if brygge_versions_differ {
        match ir2.restamped(
            &ir1.provenance.brygge_version,
            &ir1.provenance.decoder_version,
        ) {
            Ok(r) => r,
            Err(e) => {
                return AgainstSourceOutcome::NotChecked(format!(
                    "could not compare across brygge versions: {e}"
                ));
            }
        }
    } else {
        ir2
    };
    let version_note =
        || brygge_version_note(&ir1.provenance.brygge_version, &this_build_brygge_version);
    let version_detail =
        || brygge_version_detail(&ir1.provenance.brygge_version, &this_build_brygge_version);

    if let Some(first) = ir1.transforms.first() {
        // RFC 018 D-5: an artifact with a chain is never compared by whole-`Ir` equality — nothing re-
        // runs the chain itself here (that is batch P's own job), so the only thing a fresh decode of
        // the source can ever correspond to is the chain's own *root*: what `transforms[0].consumed`
        // names. `digest_of` is the exact digest `write_to` would give that fresh decode, without
        // writing it. Review 106 V-1: the same brygge-version rule applies to this root comparison —
        // `ir2` above is already restamped to `ir1`'s versions when they differ.
        return match brygge_ir::digest_of(&ir2) {
            Ok(digest) => {
                let rerun_note = format!(
                    "{} transform(s) after it were not re-run",
                    ir1.transforms.len()
                );
                if digest == first.consumed {
                    let mut note = rerun_note;
                    if brygge_versions_differ {
                        note = format!("{note}; {}", version_note());
                    }
                    if let Some(f) = &floor_differs {
                        note = format!("{note}; {f}");
                    }
                    if is_cvs {
                        AgainstSourceOutcome::RootReproduces(note)
                    } else {
                        AgainstSourceOutcome::RootCorresponds(note)
                    }
                } else if brygge_versions_differ {
                    // Never a claim of tampering: a difference across brygge versions proves nothing
                    // about the chain's root by itself.
                    AgainstSourceOutcome::NotChecked(version_detail())
                } else {
                    let detail = "the chain's root is not this source's decode".to_string();
                    if is_cvs {
                        AgainstSourceOutcome::DoesNotReproduce(detail)
                    } else {
                        AgainstSourceOutcome::DoesNotCorrespond(detail)
                    }
                }
            }
            Err(e) => AgainstSourceOutcome::NotChecked(format!(
                "could not compute the re-decode's own digest: {e}"
            )),
        };
    }

    // Nothing else in the IR is provenance-only-and-volatile any more (import_time was removed by
    // RFC 011), so the aligned decodes are compared directly.
    let corresponds = ir1 == &ir2;
    if !corresponds && brygge_versions_differ {
        // Review 106 V-1: a difference across brygge versions is not evidence of tampering — never
        // `does-not-correspond`/`does-not-reproduce` here, whatever `corresponds` says.
        return AgainstSourceOutcome::NotChecked(version_detail());
    }
    let detail = || {
        let d = divergence(ir1, &ir2);
        match &versions_differ {
            Some(v) => format!("{d} (svnadmin versions differ: {v})"),
            None => d,
        }
    };
    match (is_cvs, corresponds) {
        (true, true) => AgainstSourceOutcome::Reproduces(combine_notes(vec![
            brygge_versions_differ.then(version_note),
            floor_differs.clone(),
        ])),
        (true, false) => AgainstSourceOutcome::DoesNotReproduce(detail()),
        (false, true) => {
            let svn_note = versions_differ
                .map(|v| format!("svnadmin versions differ ({v}); the history is identical"));
            let note = combine_notes(vec![
                brygge_versions_differ.then(version_note),
                svn_note,
                floor_differs.clone(),
            ]);
            AgainstSourceOutcome::Corresponds(note)
        }
        (false, false) => AgainstSourceOutcome::DoesNotCorrespond(detail()),
    }
}

/// Review 106 V-1: an informational note that two decodes used different brygge versions, but the
/// history is identical — never a reason to doubt a `corresponds`/`reproduces`/`root-corresponds`
/// result, exactly as [`svn_version_note`]'s note is for `svnadmin`.
fn brygge_version_note(a: &str, b: &str) -> String {
    format!("decoded by brygge {a}, re-derived by {b}; the history is identical")
}

/// Review 106 V-1: the detail for `not-checked` when two different brygge versions decode `a`'s
/// history differently — never a claim of tampering, which a version difference alone can never be.
fn brygge_version_detail(a: &str, b: &str) -> String {
    format!(
        "decoded by brygge {a}; this build ({b}) decodes differently, which a newer decoder may do \
         legitimately; run `verify --against-source` with brygge {a} for a definite answer"
    )
}

/// `Some(reason)` when `repo`'s form (a dumpfile vs a live repository) differs from what `ir1`'s
/// provenance recorded (CR-07.6). `None` when there is nothing recorded to compare (an artifact from
/// before this handoff) or the forms already match.
fn svn_source_form_mismatch(ir1: &Ir, repo: &Path) -> Option<String> {
    let recorded = ir1.provenance.params.get("source_form")?;
    // A path that does not exist has no form: it is not a "repository" for want of being a file. The
    // re-decode says `source not found`, as it does for every source kind.
    if matches!(std::fs::symlink_metadata(repo), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return None;
    }
    let given = if repo.is_file() {
        "dumpfile"
    } else {
        "svnadmin-dump"
    };
    if recorded == given {
        return None;
    }
    Some(format!(
        "the artifact was made from a {recorded}; verify against the same form"
    ))
}

/// Set `ir2`'s `svnadmin_version` provenance param to `ir1`'s (CR-07.6, review 010 R-1), so the two are
/// comparable regardless of which `svnadmin` build produced the re-decode. A no-op unless both have the
/// param (i.e. both are the `svnadmin-dump` form).
fn align_svnadmin_version(ir1: &Ir, mut ir2: Ir) -> Ir {
    if let (Some(a), Some(_)) = (
        ir1.provenance.params.get("svnadmin_version"),
        ir2.provenance.params.get("svnadmin_version"),
    ) {
        let a = a.clone();
        ir2.provenance
            .params
            .insert("svnadmin_version".to_string(), a);
    }
    ir2
}

/// `Some("<a> vs <b>")` when both decodes are the `svnadmin-dump` form and their recorded versions
/// actually differ (CR-07.6) — a hint at *why* two decodes of what should be the same history might
/// diverge, not a claim that the version caused it, and not itself a reason to fail when the histories
/// otherwise match (see [`align_svnadmin_version`]).
fn svn_version_note(ir1: &Ir, ir2: &Ir) -> Option<String> {
    let a = ir1.provenance.params.get("svnadmin_version")?;
    let b = ir2.provenance.params.get("svnadmin_version")?;
    if a == b {
        return None;
    }
    Some(format!("{a} vs {b}"))
}

/// Set `ir2`'s `floor` provenance param to `ir1`'s (review 140, a refinement of C-3c): a decoder's floor
/// — the list of features it refuses rather than approximates — is a fact about the decoder, exactly
/// like `svnadmin`'s version (CR-07.6), never about the history it decoded. Two decodes that both
/// *succeeded* prove the source holds nothing either floor refuses: an item added since the first decode
/// did not fire on the re-decode, or the re-decode would have been refused outright; an item removed
/// could only matter for a feature the first decode would itself have refused, and it did not. So a
/// floor difference alone can never be evidence of tampering. A no-op unless both carry the param (every
/// decoder that has a floor records one — `git`, `hg`, `svn` and `cvs` all do).
fn align_decoder_floor(ir1: &Ir, mut ir2: Ir) -> Ir {
    if let (Some(a), Some(_)) = (
        ir1.provenance.params.get("floor"),
        ir2.provenance.params.get("floor"),
    ) {
        let a = a.clone();
        ir2.provenance.params.insert("floor".to_string(), a);
    }
    ir2
}

/// `Some(note)` when both decodes recorded a `floor` param and the sets of items actually differ — a
/// hint at *why* two decodes might diverge, never a claim that the floor caused it, and not itself a
/// reason to fail when the histories otherwise match (see [`align_decoder_floor`]).
fn decoder_floor_note(ir1: &Ir, ir2: &Ir) -> Option<String> {
    let a = ir1.provenance.params.get("floor")?;
    let b = ir2.provenance.params.get("floor")?;
    if a == b {
        return None;
    }
    let a_items: std::collections::BTreeSet<&str> =
        a.split(',').filter(|s| !s.is_empty()).collect();
    let b_items: std::collections::BTreeSet<&str> =
        b.split(',').filter(|s| !s.is_empty()).collect();
    let added: Vec<&str> = b_items.difference(&a_items).copied().collect();
    let removed: Vec<&str> = a_items.difference(&b_items).copied().collect();
    let name = |items: &[&str]| {
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(", ")
        }
    };
    Some(format!(
        "the decoder's floor changed between the two decodes: added {}; removed {}",
        name(&added),
        name(&removed)
    ))
}

/// Join every present note with `"; "`, in order; `None` if none are present (review 106 V-1 and review
/// 140 both add an informational note that never changes a success verdict by itself).
fn combine_notes(parts: Vec<Option<String>>) -> Option<String> {
    let present: Vec<String> = parts.into_iter().flatten().collect();
    if present.is_empty() {
        None
    } else {
        Some(present.join("; "))
    }
}

// ---- `verify --against-target` (RFC 019 D-6, batch G2) ----------------------------------------------

/// The result of `--against-target git <repository>` (D-6), independent of `--against-source` — the
/// CLI refuses the two together (one question per run). `NotChecked` is never a claim of tampering
/// (review 003 R-1's rule, extended to this comparison): the comparison itself could not run.
#[derive(Debug)]
enum AgainstTargetOutcome {
    Corresponds,
    DoesNotCorrespond(Vec<String>),
    NotChecked(String),
    NotRun,
}

impl AgainstTargetOutcome {
    fn machine_label(&self) -> &'static str {
        match self {
            Self::Corresponds => "corresponds",
            Self::DoesNotCorrespond(_) => "does-not-correspond",
            Self::NotChecked(_) => "not-checked",
            Self::NotRun => "not-run",
        }
    }
    fn human_label(&self) -> &'static str {
        match self {
            Self::Corresponds => "corresponds",
            Self::DoesNotCorrespond(_) => "does not correspond",
            Self::NotChecked(_) => "could not be checked",
            Self::NotRun => "not run",
        }
    }
    fn is_mismatch(&self) -> bool {
        matches!(self, Self::DoesNotCorrespond(_))
    }
    fn is_not_checked(&self) -> bool {
        matches!(self, Self::NotChecked(_))
    }
}

/// `git for-each-ref` plus `HEAD` (RFC 019 D-6 §1: "listed with the §1 environment of G1", read-only, no
/// user configuration), the ground truth [`encode_git::compare_git_target`] checks `refs/brygge/other/*`
/// and `HEAD` against — namespaces `brygge_decode_git` itself never carries.
fn list_git_refs(git: &Path, env: &GitEnv, repo: &Path) -> Result<Vec<encode_git::GitRef>, String> {
    let mut cmd = Subprocess::new(git);
    env.apply(&mut cmd);
    cmd.current_dir(repo);
    cmd.args(["for-each-ref", "--format=%(objectname) %(refname)"]);
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    let output = cmd
        .output()
        .map_err(|e| format!("cannot run git for-each-ref: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git for-each-ref failed (exit {:?})",
            output.status.code()
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "git for-each-ref printed non-UTF-8 output".to_string())?;
    let mut refs = Vec::new();
    for line in text.lines() {
        let Some((oid_hex, name)) = line.split_once(' ') else {
            continue;
        };
        let oid = hex20(oid_hex).ok_or_else(|| {
            format!("git for-each-ref printed a malformed object id: {oid_hex:?}")
        })?;
        refs.push(encode_git::GitRef {
            name: name.to_string(),
            target: encode_git::GitRefTarget::Oid(oid),
        });
    }

    let mut head_cmd = Subprocess::new(git);
    env.apply(&mut head_cmd);
    head_cmd.current_dir(repo);
    head_cmd.args(["symbolic-ref", "--quiet", "HEAD"]);
    head_cmd.stdin(Stdio::null()).stderr(Stdio::null());
    let head_output = head_cmd
        .output()
        .map_err(|e| format!("cannot run git symbolic-ref: {e}"))?;
    let head_target = if head_output.status.success() {
        let name = String::from_utf8(head_output.stdout)
            .map_err(|_| "git symbolic-ref printed non-UTF-8 output".to_string())?;
        encode_git::GitRefTarget::Symbolic(name.trim().to_string())
    } else {
        let mut rev_cmd = Subprocess::new(git);
        env.apply(&mut rev_cmd);
        rev_cmd.current_dir(repo);
        rev_cmd.args(["rev-parse", "--verify", "HEAD"]);
        rev_cmd.stdin(Stdio::null()).stderr(Stdio::null());
        let rev_output = rev_cmd
            .output()
            .map_err(|e| format!("cannot run git rev-parse: {e}"))?;
        if rev_output.status.success() {
            let hex_str = String::from_utf8(rev_output.stdout)
                .map_err(|_| "git rev-parse printed non-UTF-8 output".to_string())?;
            let oid = hex20(hex_str.trim())
                .ok_or_else(|| "git rev-parse printed a malformed object id".to_string())?;
            encode_git::GitRefTarget::Oid(oid)
        } else {
            return Err("HEAD names no ref and resolves to no commit (unborn)".to_string());
        }
    };
    refs.push(encode_git::GitRef {
        name: "HEAD".to_string(),
        target: head_target,
    });
    Ok(refs)
}

fn hex20(s: &str) -> Option<Vec<u8>> {
    if s.len() != 40 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(20);
    for pair in bytes.chunks_exact(2) {
        let hi = (*pair.first()? as char).to_digit(16)?;
        let lo = (*pair.get(1)? as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// Checks `ir` against a real Git repository someone kept (RFC 019 D-6): lists its refs, decodes it with
/// `brygge-decode-git` (the CLI's own decode path, under `storage`'s spill rules — never all in
/// memory), and runs the one shared comparison [`encode_git::compare_git_target`]. Shared with
/// `encode_git`'s own in-process check (`run_encode_git_with_check`), so the two can never see a
/// different rule for the same question.
fn run_against_target(
    ir: &Ir,
    digest: &[u8; 32],
    repo: &Path,
    storage: &brygge_ir::Storage,
) -> AgainstTargetOutcome {
    let Some(git) = plugin::find_tool("git") else {
        return AgainstTargetOutcome::NotChecked("git is not on PATH".to_string());
    };
    let env = match GitEnv::new() {
        Ok(e) => e,
        Err(e) => {
            return AgainstTargetOutcome::NotChecked(format!(
                "cannot prepare git's environment: {e}"
            ));
        }
    };
    let result_refs = match list_git_refs(&git, &env, repo) {
        Ok(r) => r,
        Err(e) => {
            return AgainstTargetOutcome::NotChecked(format!(
                "cannot list {}'s refs: {e}",
                repo.display()
            ));
        }
    };
    let opts = SourceOpts {
        infer_renames: false,
        reconstruct_refs: false,
        branch_lines: false,
        // RFC 019 D-6's check (review 147 F-1): the Git decoder must see a `--branch-lines` dead
        // line's commits too, under refs/brygge/other/*, to compare them like any other commit —
        // never exposed on the CLI, only this internal re-decode asks for it.
        carry_extra_namespaces: vec![encode_git::OTHER_NAMESPACE.to_string()],
        layout: LayoutPolicy::default(),
        cvs_window: 0,
        cvs_floor: 0,
    };
    let result_ir = match decode_source(SourceKind::Git, repo, &opts, storage) {
        Ok(ir) => ir,
        Err((_, msg)) => {
            return AgainstTargetOutcome::NotChecked(format!(
                "cannot decode {}: {msg}",
                repo.display()
            ));
        }
    };
    let default_branch_ref = format!("refs/heads/{}", encode_git::choose_default_branch(ir).name);
    match encode_git::compare_git_target(ir, digest, &result_ir, &result_refs, &default_branch_ref)
    {
        encode_git::Comparison::Corresponds => AgainstTargetOutcome::Corresponds,
        encode_git::Comparison::DoesNotCorrespond(diffs) => {
            AgainstTargetOutcome::DoesNotCorrespond(diffs)
        }
        encode_git::Comparison::NotChecked(reason) => AgainstTargetOutcome::NotChecked(reason),
    }
}

fn divergence(a: &Ir, b: &Ir) -> String {
    if a.atoms.len() != b.atoms.len() {
        return format!(
            "atom count differs: artifact {} vs source {}",
            a.atoms.len(),
            b.atoms.len()
        );
    }
    for (x, y) in a.atoms.iter().zip(b.atoms.iter()) {
        if x != y {
            return format!(
                "first divergence at source atom {}",
                short_hex(&x.source.atom_id)
            );
        }
    }
    if a.refs != b.refs {
        return "refs differ".to_string();
    }
    if a.content != b.content {
        return "content store differs".to_string();
    }
    "provenance or loss boundary differs".to_string()
}

const CHECK_NAMES: [&str; 7] = [
    "integrity",
    "structure",
    "replay",
    "derivations",
    "source-invariants",
    "provenance",
    "loss-boundary",
];

/// The three-valued verdict (review 003 R-5; mandatory from RFC 011's handoff): `pass` when everything
/// requested ran and held; `fail` when something that ran did not hold (takes precedence, exit 50);
/// `incomplete` when a requested `--against-source` could not be checked and nothing that ran failed
/// (exit 1) — never reported as `pass` when the exit code is not 0.
enum Verdict {
    Pass,
    Fail,
    Incomplete,
}

impl Verdict {
    /// `against_target` is `--against-source`'s own CLI-enforced mutual exclusion with
    /// `--against-target` (RFC 019 D-6: "one question per run") made visible here too: at most one of
    /// the two is ever not [`AgainstSourceOutcome::NotRun`]/[`AgainstTargetOutcome::NotRun`] in the same
    /// call.
    fn from(
        internal_pass: bool,
        against: &AgainstSourceOutcome,
        against_target: &AgainstTargetOutcome,
    ) -> Self {
        if !internal_pass || against.is_mismatch() || against_target.is_mismatch() {
            Self::Fail
        } else if against.is_not_checked() || against_target.is_not_checked() {
            Self::Incomplete
        } else {
            Self::Pass
        }
    }
    fn machine_label(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Incomplete => "incomplete",
        }
    }
    fn human_line(&self) -> &'static str {
        match self {
            Self::Pass => "  => PASS",
            Self::Fail => "  => FAIL",
            Self::Incomplete => "  => INCOMPLETE (a requested check could not run)",
        }
    }
    fn exit_code(&self) -> i32 {
        match self {
            Self::Pass => exit::CLEAN,
            Self::Fail => exit::VERIFY_FAILED,
            Self::Incomplete => exit::FAILURE,
        }
    }
}

/// Every internal check (never `--against-source`), and whether they all passed. Shared by `verify`
/// itself and by [`verify_internal_report`] (`encode`'s mandatory verify-first gate, OQ-4), so the two
/// can never see a different set of checks for the same artifact.
fn compute_internal_checks(
    parsed: &Result<Decoded, brygge_ir::Error>,
) -> (Vec<(&'static str, CheckOutcome)>, bool) {
    let mut checks: Vec<(&'static str, CheckOutcome)> = Vec::with_capacity(CHECK_NAMES.len());
    let ir_opt = match parsed {
        Ok(decoded) => {
            checks.push(("integrity", CheckOutcome::Pass));
            Some(&decoded.ir)
        }
        Err(e) => {
            checks.push(("integrity", CheckOutcome::Fail(e.to_string())));
            None
        }
    };
    if let Some(ir) = ir_opt {
        checks.push(("structure", check_structure(ir)));
        checks.push(("replay", check_replay(ir)));
        checks.push(("derivations", check_derivations(ir)));
        checks.push(("source-invariants", check_source_invariants(ir)));
        checks.push(("provenance", check_provenance(ir)));
        checks.push(("loss-boundary", check_loss_boundary(ir)));
    } else {
        for name in &CHECK_NAMES[1..] {
            checks.push((
                name,
                CheckOutcome::NotChecked(
                    "the artifact failed to decode; this check cannot run".to_string(),
                ),
            ));
        }
    }
    let internal_pass = !checks.iter().any(|(_, o)| o.is_fail());
    (checks, internal_pass)
}

/// Runs every internal check (never `--against-source`) and renders the report, without writing it
/// anywhere: `encode`'s verify-first gate (OQ-4) prints this to stderr itself (brygge's own stdout is
/// reserved for the plugin it may go on to run), never to stdout the way the standalone `verify` command
/// does. An unreadable artifact short-circuits exactly as `verify` itself does, before any check runs.
pub(crate) fn verify_internal_report(artifact: &Path, format: Format) -> (String, i32) {
    let parsed = brygge_ir::from_file(artifact);
    if let Err(brygge_ir::Error::Io(msg)) = &parsed {
        return (
            format!("cannot read {}: {msg}\n", artifact.display()),
            exit::FAILURE,
        );
    }
    let (checks, internal_pass) = compute_internal_checks(&parsed);
    let against = AgainstSourceOutcome::NotRun;
    let against_target = AgainstTargetOutcome::NotRun;
    let verdict = Verdict::from(internal_pass, &against, &against_target);
    let skipped = parsed
        .as_ref()
        .map_or(0, |decoded| decoded.skipped_non_critical_fields);
    let rendered = format_verify_report(
        format,
        &checks,
        internal_pass,
        &against,
        &against_target,
        &verdict,
        skipped,
    );
    (rendered, verdict.exit_code())
}

/// `verify <artifact> [--against-source <source> | --against-target git <repository>] [--spill-dir
/// <DIR>]` (CL-04, CR-02; `--against-target` is RFC 019 D-6): the internal checks always run; each can
/// fail. With `--against-source` or `--against-target`, additionally re-derive and report that
/// comparison separately (VF-4) — one question per run, enforced by the CLI parser, never both. The
/// re-decode spills file content to `spill_dir` (or, by default, the artifact's own directory), the
/// same rule `decode`'s own `--spill-dir` follows (RFC 015 D-4), checked before any work starts. Exit
/// `50` if anything fails; exit `1` if a requested comparison could not be checked at all and nothing
/// that ran failed (the three-valued verdict, review 003 R-5).
pub fn run_verify(
    artifact: &Path,
    against_source: Option<&Path>,
    against_target: Option<&Path>,
    spill_dir: Option<&Path>,
    format: Format,
) -> i32 {
    let storage = if against_source.is_some() || against_target.is_some() {
        let dir = match spill_dir {
            Some(dir) => {
                if !dir.is_dir() {
                    output::stderr(&format!(
                        "--spill-dir {} does not exist or is not a directory",
                        dir.display()
                    ));
                    return exit::USAGE;
                }
                dir.to_path_buf()
            }
            None => artifact
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
        };
        Some(brygge_ir::Storage::Spill(dir))
    } else {
        None
    };

    // RFC 015 D-4: a streaming, single-pass read — never the whole artifact loaded into one buffer the
    // way `std::fs::read` + `from_bytes` used to (fish-shell: 9.3 GiB peak before this).
    let parsed = brygge_ir::from_file(artifact);
    if let Err(brygge_ir::Error::Io(msg)) = &parsed {
        output::stderr(&format!("cannot read {}: {msg}", artifact.display()));
        return exit::FAILURE;
    }
    let (checks, internal_pass) = compute_internal_checks(&parsed);
    let ir_opt = parsed.as_ref().ok().map(|decoded| &decoded.ir);

    let against = match (against_source, ir_opt, &storage) {
        (Some(src), Some(ir), Some(storage)) => run_against_source(ir, src, storage),
        (Some(_), None, _) => AgainstSourceOutcome::NotChecked(
            "the artifact failed to decode; there is nothing to re-derive against".to_string(),
        ),
        (None, _, _) => AgainstSourceOutcome::NotRun,
        // `storage` is `Some` exactly when `against_source` is `Some` (built together, above) — this
        // arm is unreachable, kept only so the match stays exhaustive without a wildcard.
        (Some(_), Some(_), None) => unreachable!("storage is set whenever against_source is"),
    };
    let against_target = match (against_target, ir_opt, &storage) {
        (Some(repo), Some(ir), Some(storage)) => {
            #[allow(clippy::expect_used)]
            let digest = parsed
                .as_ref()
                .expect("ir_opt is Some only when parsed is Ok")
                .digest;
            run_against_target(ir, &digest, repo, storage)
        }
        (Some(_), None, _) => AgainstTargetOutcome::NotChecked(
            "the artifact failed to decode; there is nothing to check against a repository"
                .to_string(),
        ),
        (None, _, _) => AgainstTargetOutcome::NotRun,
        (Some(_), Some(_), None) => unreachable!("storage is set whenever against_target is"),
    };
    let verdict = Verdict::from(internal_pass, &against, &against_target);

    let skipped = parsed
        .as_ref()
        .map_or(0, |decoded| decoded.skipped_non_critical_fields);
    if let Some(code) = render_verify(
        format,
        &checks,
        internal_pass,
        &against,
        &against_target,
        &verdict,
        skipped,
    ) {
        return code;
    }
    verdict.exit_code()
}

/// A check name as a machine-output key segment: keys are `snake_case` (`source_invariants`); the names
/// themselves, shown to humans, stay `kebab-case`.
fn check_key(name: &str) -> String {
    name.replace('-', "_")
}

/// The machine-format `verify` lines (a pure function so the exact keys are testable). A check's detail
/// belongs to that check: `verify.check.<name>.detail` follows its line, and only when it is not `pass`.
/// The source comparison's explanation is `verify.against_source.detail`, and an informational note that
/// is not a failure is `verify.against_source.note`.
fn verify_machine_lines(
    checks: &[(&'static str, CheckOutcome)],
    internal_pass: bool,
    against: &AgainstSourceOutcome,
    against_target: &AgainstTargetOutcome,
    verdict: &Verdict,
    skipped_non_critical_fields: u64,
) -> Vec<String> {
    let mut out = vec![
        format!("verify_version={VERIFY_VERSION}"),
        format!("verify.skipped_non_critical_fields={skipped_non_critical_fields}"),
    ];
    for (name, outcome) in checks {
        let key = check_key(name);
        out.push(format!("verify.check.{key}={}", outcome.label()));
        if let Some(d) = outcome.detail() {
            out.push(format!(
                "verify.check.{key}.detail={}",
                display::machine_value(d)
            ));
        }
    }
    out.push("verify.authorship=unverifiable".to_string());
    out.push(format!(
        "verify.internal={}",
        if internal_pass { "pass" } else { "fail" }
    ));
    out.push(format!("verify.against_source={}", against.machine_label()));
    if let Some(d) = against.detail() {
        out.push(format!(
            "verify.against_source.detail={}",
            display::machine_value(d)
        ));
    }
    if let Some(n) = against.note() {
        out.push(format!(
            "verify.against_source.note={}",
            display::machine_value(n)
        ));
    }
    out.push(format!(
        "verify.against_target={}",
        against_target.machine_label()
    ));
    if let AgainstTargetOutcome::NotChecked(d) = against_target {
        out.push(format!(
            "verify.against_target.detail={}",
            display::machine_value(d)
        ));
    }
    if let AgainstTargetOutcome::DoesNotCorrespond(diffs) = against_target {
        out.push(format!(
            "verify.against_target.detail={}",
            display::machine_value(&target_diff_summary(diffs))
        ));
    }
    out.push(format!("verify.result={}", verdict.machine_label()));
    out
}

/// At most 20 of `diffs`, then a count of the rest (RFC 019 D-6 §1), as one line for a `.detail` value.
fn target_diff_summary(diffs: &[String]) -> String {
    const SHOWN: usize = 20;
    let Some(shown) = diffs.get(..SHOWN) else {
        return diffs.join("; ");
    };
    format!("{}; and {} more", shown.join("; "), diffs.len() - SHOWN)
}

/// Renders the report into one string, then writes it in a single call (so a reader that closes the
/// pipe mid-report is no different from one that closes it before the first byte: either way, at most
/// one write is attempted). `Some(exit::FAILURE)` only for a genuine write error, not `BrokenPipe`; the
/// caller must then return it instead of the exit code it was about to compute.
fn render_verify(
    format: Format,
    checks: &[(&'static str, CheckOutcome)],
    internal_pass: bool,
    against: &AgainstSourceOutcome,
    against_target: &AgainstTargetOutcome,
    verdict: &Verdict,
    skipped_non_critical_fields: u64,
) -> Option<i32> {
    let rendered = format_verify_report(
        format,
        checks,
        internal_pass,
        against,
        against_target,
        verdict,
        skipped_non_critical_fields,
    );
    output::stdout(&rendered)
}

/// The body of [`render_verify`], as a string — shared with [`verify_internal_report`], which writes it
/// to stderr instead of stdout.
fn format_verify_report(
    format: Format,
    checks: &[(&'static str, CheckOutcome)],
    internal_pass: bool,
    against: &AgainstSourceOutcome,
    against_target: &AgainstTargetOutcome,
    verdict: &Verdict,
    skipped_non_critical_fields: u64,
) -> String {
    let mut s = String::new();
    match format {
        Format::Machine => {
            for line in verify_machine_lines(
                checks,
                internal_pass,
                against,
                against_target,
                verdict,
                skipped_non_critical_fields,
            ) {
                let _ = writeln!(s, "{line}");
            }
        }
        Format::Human => {
            let _ = writeln!(
                s,
                "verify — the honesty checks any reader can run with no source (always run):"
            );
            for (name, outcome) in checks {
                let mark = match outcome {
                    CheckOutcome::Pass => "ok",
                    CheckOutcome::Fail(_) => "FAIL",
                    CheckOutcome::NotChecked(_) => "not-checked",
                };
                let _ = writeln!(s, "  [{mark}] {name}");
                if let Some(d) = outcome.detail() {
                    let _ = writeln!(s, "      {}", display::human(d));
                }
            }
            let _ = writeln!(
                s,
                "  authorship: Unverifiable by construction (no IR field can express target \
                 verification)"
            );
            let _ = writeln!(
                s,
                "  internal: {}",
                if internal_pass { "pass" } else { "fail" }
            );
            if let AgainstSourceOutcome::NotChecked(reason) = against {
                let _ = writeln!(
                    s,
                    "  against-source: could not be checked ({})",
                    display::human(reason)
                );
            } else if !matches!(against, AgainstSourceOutcome::NotRun) {
                let _ = writeln!(s, "  against-source: {}", against.human_label());
                if let Some(d) = against.detail().or_else(|| against.note()) {
                    let _ = writeln!(s, "      {}", display::human(d));
                }
            }
            match against_target {
                AgainstTargetOutcome::NotRun => {}
                AgainstTargetOutcome::NotChecked(reason) => {
                    let _ = writeln!(
                        s,
                        "  against-target: could not be checked ({})",
                        display::human(reason)
                    );
                }
                AgainstTargetOutcome::Corresponds => {
                    let _ = writeln!(s, "  against-target: {}", against_target.human_label());
                }
                AgainstTargetOutcome::DoesNotCorrespond(diffs) => {
                    let _ = writeln!(s, "  against-target: {}", against_target.human_label());
                    let _ = writeln!(s, "      {}", display::human(&target_diff_summary(diffs)));
                }
            }
            let _ = writeln!(s, "{}", verdict.human_line());
        }
    }
    s
}

// ---- encode (RFC 017 D-3) --------------------------------------------------------------------------

// RFC 019 D-1: `git` is now a reserved, built-in target, so a `brygge-encode-git` plugin on PATH is
// never run; it still appears here (plugin::list has no notion of the reservation), marked shadowed by
// the built-in. That is a new meaning for `shadowed`/a new key, so this version bumps.
const ENCODE_LIST_VERSION: u32 = 2;
const ENCODE_VERSION: u32 = 1;
// RFC 019 D-6 (batch G2): `encode_git.check` is a new report key.
const ENCODE_GIT_VERSION: u32 = 2;

/// `encode --list [--format …]`: every `brygge-encode-*` plugin [`plugin::list`] finds on `PATH`.
pub fn run_encode_list(format: Format) -> i32 {
    let plugins = plugin::list(plugin::Kind::Encode);
    let rendered = match format {
        Format::Human => render_encode_list_human(&plugins),
        Format::Machine => render_encode_list_machine(&plugins),
    };
    if let Some(code) = output::stdout(&rendered) {
        return code;
    }
    exit::CLEAN
}

fn render_encode_list_human(plugins: &[plugin::Listed]) -> String {
    let mut s = String::new();
    if plugins.is_empty() {
        let _ = writeln!(s, "no brygge-encode-* plugins found on PATH");
        return s;
    }
    for p in plugins {
        let _ = write!(s, "{}  {}", p.name, p.path.display());
        if p.name == GIT_TARGET {
            let _ = write!(s, "  (shadowed by the built-in)");
        } else if p.shadowed {
            let _ = write!(s, "  (shadowed)");
        }
        let _ = writeln!(s);
    }
    s
}

fn render_encode_list_machine(plugins: &[plugin::Listed]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "encode_list_version={ENCODE_LIST_VERSION}");
    for (i, p) in plugins.iter().enumerate() {
        let _ = writeln!(s, "encoder.{i}.target={}", display::machine_value(&p.name));
        let _ = writeln!(
            s,
            "encoder.{i}.path={}",
            display::machine_value(&p.path.display().to_string())
        );
        // `git` is always shadowed (the built-in always wins), regardless of plugin::list's own
        // PATH-position-based `shadowed` computation, which knows nothing of the reservation.
        let _ = writeln!(
            s,
            "encoder.{i}.shadowed={}",
            p.shadowed || p.name == GIT_TARGET
        );
        if p.name == GIT_TARGET {
            let _ = writeln!(s, "encoder.{i}.shadowed_by=built_in");
        }
    }
    s
}

/// The mandatory stderr banner, printed before the plugin runs (RFC 017 D-3 §4): always, in both
/// formats, and in the machine format as a versioned key=value block — brygge's own stdout is reserved
/// for the plugin it is about to run, so unlike every other command's machine output, `encode`'s own
/// status lines (this banner, and [`encode_exit_text`]/[`encode_signal_text`] after) go to stderr, keyed
/// and versioned the same way so a script can still read them without parsing prose.
fn encode_banner_text(format: Format, target: &str, path: &Path) -> String {
    match format {
        Format::Human => format!(
            "encoding with the external plugin `brygge-encode-{target}` (`{}`): not part of brygge; \
             brygge verified the artifact, not the plugin's output",
            path.display()
        ),
        Format::Machine => format!(
            "encode_version={ENCODE_VERSION}\nencode.target={}\nencode.plugin_path={}\n\
             encode.banner=not_part_of_brygge",
            display::machine_value(target),
            display::machine_value(&path.display().to_string()),
        ),
    }
}

fn encode_exit_text(format: Format, target: &str, code: i32) -> String {
    match format {
        Format::Human => format!("`brygge-encode-{target}` exited with {code}"),
        Format::Machine => format!("encode.exit_code={code}"),
    }
}

fn encode_signal_text(format: Format, target: &str, signal: i32) -> String {
    match format {
        Format::Human => match plugin::signal_name(signal) {
            Some(name) => {
                format!("`brygge-encode-{target}` was killed by signal {signal} ({name})")
            }
            None => format!("`brygge-encode-{target}` was killed by signal {signal}"),
        },
        Format::Machine => format!("encode.signal={signal}"),
    }
}

/// `encode <target> <artifact> [-- <plugin arguments>…]` (RFC 017 D-3, OQ-1/OQ-4). Resolves
/// `brygge-encode-<target>` on `PATH` ([`plugin::find`] — never the bare name), verifies the artifact
/// first and refuses to run the plugin on anything but a clean `pass` (exiting with `verify`'s own exit
/// code), then spawns the plugin with the mandatory banner. The plugin's stdin/stdout/stderr are
/// inherited: its own stdout is the user's output, never touched by brygge.
pub fn run_encode(target: &str, artifact: &Path, plugin_args: &[String], format: Format) -> i32 {
    let Some(path) = plugin::find(plugin::Kind::Encode, target) else {
        output::stderr(&format!(
            "no encoder for '{target}': brygge-encode-{target} is not on PATH (see `brygge encode \
             --list`)"
        ));
        return exit::USAGE;
    };

    let (report, code) = verify_internal_report(artifact, format);
    if code != exit::CLEAN {
        output::stderr(&report);
        return code;
    }

    output::stderr(&encode_banner_text(format, target, &path));

    let mut argv: Vec<std::ffi::OsString> =
        vec!["--artifact".into(), artifact.as_os_str().to_os_string()];
    argv.extend(plugin_args.iter().map(std::ffi::OsString::from));

    match plugin::run(&path, &argv) {
        plugin::Ran::Exited(code) => {
            output::stderr(&encode_exit_text(format, target, code));
            code
        }
        plugin::Ran::Signaled(signal) => {
            output::stderr(&encode_signal_text(format, target, signal));
            exit::FAILURE
        }
        plugin::Ran::CannotStart(e) => {
            output::stderr(&format!(
                "cannot start brygge-encode-{target} ({}): {e}",
                path.display()
            ));
            exit::FAILURE
        }
    }
}

// ---- encode git (RFC 019, batch G1) ------------------------------------------------------------------

/// A unique path under the system temp directory, named `brygge-encode-git-<label>-<pid>-<nanos>`. Not
/// itself created — the caller creates the directory or file it names, after checking the name is free
/// (the same pid+nanos+attempt scheme `create_transform_temp` uses for a file, adapted here for a
/// directory too).
fn unique_temp_path(label: &str, attempt: u32) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("brygge-encode-git-{label}-{pid}-{nanos}-{attempt}"))
}

/// The fixed environment `git` runs under for `encode git` (RFC 019 D-1): no user configuration,
/// template, or hook can reach the result. `HOME` and `GIT_CONFIG_GLOBAL` are a throwaway directory and
/// file, removed when this value is dropped.
struct GitEnv {
    home: PathBuf,
    global_config: PathBuf,
}

impl GitEnv {
    fn new() -> std::io::Result<Self> {
        const MAX_ATTEMPTS: u32 = 64;
        let mut home = None;
        for attempt in 0..MAX_ATTEMPTS {
            let p = unique_temp_path("home", attempt);
            match std::fs::create_dir(&p) {
                Ok(()) => {
                    home = Some(p);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        let home = home.ok_or_else(|| std::io::Error::other("could not create a temp HOME"))?;

        let mut global_config = None;
        for attempt in 0..MAX_ATTEMPTS {
            let p = unique_temp_path("gitconfig", attempt);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
            {
                Ok(_) => {
                    global_config = Some(p);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&home);
                    return Err(e);
                }
            }
        }
        let Some(global_config) = global_config else {
            let _ = std::fs::remove_dir_all(&home);
            return Err(std::io::Error::other(
                "could not create a temp GIT_CONFIG_GLOBAL file",
            ));
        };
        Ok(Self {
            home,
            global_config,
        })
    }

    /// Applies the fixed environment to `cmd`: `env_clear()`, then exactly `PATH` (the real value,
    /// unchanged — `git` itself must still be found by the same `PATH` brygge resolved it from),
    /// `HOME`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL`, `LC_ALL=C`, and, on Windows, `SystemRoot`
    /// (Windows's own DLL loader needs it).
    fn apply(&self, cmd: &mut Subprocess) {
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", &self.home);
        cmd.env("GIT_CONFIG_NOSYSTEM", "1");
        cmd.env("GIT_CONFIG_GLOBAL", &self.global_config);
        cmd.env("LC_ALL", "C");
        if cfg!(windows) {
            if let Some(system_root) = std::env::var_os("SystemRoot") {
                cmd.env("SystemRoot", system_root);
            }
        }
    }
}

impl Drop for GitEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
        let _ = std::fs::remove_file(&self.global_config);
    }
}

/// `git check-ref-format <name>`, under the same fixed environment. `Ok(())` if Git accepts the name;
/// `Err(reason)` otherwise — `check-ref-format` itself prints nothing explaining why, so the reason is
/// brygge's own, naming the exit code.
fn check_ref_format(git: &Path, env: &GitEnv, git_ref: &str) -> Result<(), String> {
    let mut cmd = Subprocess::new(git);
    env.apply(&mut cmd);
    cmd.args(["check-ref-format", git_ref]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "git check-ref-format rejected it (exit {})",
            status.code().map_or("?".to_string(), |c| c.to_string())
        )),
        Err(e) => Err(format!("cannot run git check-ref-format: {e}")),
    }
}

/// Every pre-flight refusal (RFC 019 D-7): [`encode_git::preflight`]'s IR-level refusals, plus one
/// `git check-ref-format` check per distinct Git ref name (the CLI's own job — it alone can run `git`).
fn encode_git_preflight(ir: &Ir, git: &Path, env: &GitEnv) -> Vec<encode_git::Refusal> {
    let mut refusals = encode_git::preflight(ir);
    let mut checked: BTreeSet<String> = BTreeSet::new();
    for rf in &ir.refs {
        let path = encode_git::git_ref_path(rf);
        if !checked.insert(path.clone()) {
            continue;
        }
        if let Err(reason) = check_ref_format(git, env, &path) {
            refusals.push(encode_git::Refusal::InvalidRefName {
                git_ref: path,
                reason,
            });
        }
    }
    refusals
}

fn render_encode_git_refusals(format: Format, refusals: &[encode_git::Refusal]) -> String {
    match format {
        Format::Human => {
            let mut s = String::new();
            let _ = writeln!(s, "the artifact cannot be written as Git:");
            for r in refusals {
                let _ = writeln!(s, "  {}", display::human(&r.to_string()));
            }
            s
        }
        Format::Machine => {
            let mut s = String::new();
            let _ = writeln!(s, "encode_git_version={ENCODE_GIT_VERSION}");
            for (i, r) in refusals.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "encode_git.refusal.{i}={}",
                    display::machine_value(&r.to_string())
                );
            }
            s
        }
    }
}

/// The `<dir>.brygge-report` sidecar file (RFC 019 D-2, OQ-1): always in the machine format
/// (`machine-output.md`), regardless of `--format` — a sidecar meant for later reading and tooling, not
/// this run's own stderr/stdout.
fn render_encode_git_report(
    digest: &[u8; 32],
    default_branch: &encode_git::DefaultBranch,
    summary: &encode_git::Summary,
    fidelity: &brygge_ir::honesty::FidelityReport,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "encode_git_version={ENCODE_GIT_VERSION}");
    let _ = writeln!(s, "encode_git.artifact_digest=sha256:{}", hex(digest));
    let _ = writeln!(s, "encode_git.brygge_version={}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        s,
        "encode_git.default_branch={}",
        display::machine_value(&default_branch.name)
    );
    let _ = writeln!(
        s,
        "encode_git.default_branch_existed={}",
        default_branch.existed
    );
    let _ = writeln!(s, "encode_git.commits={}", summary.commits);
    let _ = writeln!(s, "encode_git.tags={}", summary.tags);
    let _ = writeln!(s, "encode_git.refs={}", summary.refs);
    // RFC 019 D-6 (batch G2): this report is only ever written after the in-process check has already
    // passed — `run_encode_git_with_check` refuses to write anything, this sidecar included, otherwise.
    let _ = writeln!(s, "encode_git.check=corresponds");
    for (class, count) in &fidelity.dropped {
        let _ = writeln!(s, "encode_git.dropped.{}={count}", key_segment(class));
    }
    for (kind, count) in &fidelity.flagged {
        let _ = writeln!(s, "encode_git.flagged.{}={count}", key_segment(kind));
    }
    // RFC 021 D-4 (batch G): absent entirely when the artifact holds no submodule, like `decode`'s own
    // `decode.submodules.*` (`honesty.rs`'s `FidelityReport`, which this report already reuses above).
    if fidelity.submodule_paths > 0 {
        let _ = writeln!(
            s,
            "encode_git.submodules.paths={}",
            fidelity.submodule_paths
        );
        let _ = writeln!(
            s,
            "encode_git.submodules.at_tips={}",
            fidelity.submodule_at_tips
        );
    }
    for (field, count) in &summary.supplied {
        let _ = writeln!(s, "encode_git.supplied.{}={count}", key_segment(field));
    }
    for (field, count) in &summary.altered {
        let _ = writeln!(s, "encode_git.altered.{}={count}", key_segment(field));
    }
    for (i, line) in summary.unplaced_ref_transforms.iter().enumerate() {
        let _ = writeln!(
            s,
            "encode_git.unplaced_ref_transform.{i}={}",
            display::machine_value(line)
        );
    }
    s
}

/// A label as a machine-output key segment (matching `brygge_ir::honesty`'s own convention): keys are
/// `snake_case`, values (labels, field names) stay as they are given.
fn key_segment(label: &str) -> String {
    label.replace('-', "_")
}

/// `encode git <artifact> --out <dir>` (RFC 019, batch G1): the built-in, supported encoder. `<dir>`
/// must not exist. Verifies the artifact first, then runs the pre-flight (D-7): every refusal in one
/// report, exit 2, nothing written. On success, `<dir>` is a new bare Git repository, and
/// `<dir>.brygge-report` its sidecar record (D-2, OQ-1). On any failure after that point, nothing is
/// left at `<dir>` (C-7): a temporary sibling directory is used throughout, renamed into place only once
/// every check has passed.
///
/// RFC 019 D-6 (batch G2): after `fast-import` succeeds and before the partial directory is renamed
/// into place, the result is decoded in-process and checked against the artifact
/// ([`encode_git::compare_git_target`]); any difference refuses the write, exactly as a pre-flight
/// refusal does, and nothing is left at `<dir>`.
pub fn run_encode_git(artifact: &Path, out: &Path, format: Format) -> i32 {
    run_encode_git_with_check(artifact, out, format, real_encode_git_check)
}

/// The real D-6 check: lists the partial repository's refs, decodes it in-process with
/// `brygge-decode-git` under a throwaway spill directory (the CLI's own spill rules — never all in
/// memory), and runs the one shared comparison. Factored out from [`run_encode_git_with_check`] so a
/// test can inject a different closure here (RFC 019 D-6 §3: "an internal seam… never an environment
/// variable or a flag in the shipped product") — [`run_encode_git`] always passes this one.
fn real_encode_git_check(
    ir: &Ir,
    digest: &[u8; 32],
    partial: &Path,
    git: &Path,
    env: &GitEnv,
    default_branch_ref: &str,
) -> encode_git::Comparison {
    let result_refs = match list_git_refs(git, env, partial) {
        Ok(r) => r,
        Err(e) => {
            return encode_git::Comparison::NotChecked(format!(
                "cannot list the result's refs: {e}"
            ));
        }
    };
    let spill_dir = match encode_git_check_spill_dir() {
        Ok(d) => d,
        Err(e) => {
            return encode_git::Comparison::NotChecked(format!(
                "cannot prepare a spill directory for the check: {e}"
            ));
        }
    };
    let opts = SourceOpts {
        infer_renames: false,
        reconstruct_refs: false,
        branch_lines: false,
        // RFC 019 D-6's check (review 147 F-1): the Git decoder must see a `--branch-lines` dead
        // line's commits too, under refs/brygge/other/*, to compare them like any other commit —
        // never exposed on the CLI, only this internal re-decode asks for it.
        carry_extra_namespaces: vec![encode_git::OTHER_NAMESPACE.to_string()],
        layout: LayoutPolicy::default(),
        cvs_window: 0,
        cvs_floor: 0,
    };
    let storage = brygge_ir::Storage::Spill(spill_dir.clone());
    let result = decode_source(SourceKind::Git, partial, &opts, &storage);
    let _ = std::fs::remove_dir_all(&spill_dir);
    let result_ir = match result {
        Ok(ir) => ir,
        Err((_, msg)) => {
            return encode_git::Comparison::NotChecked(format!(
                "cannot decode the result for the check: {msg}"
            ));
        }
    };
    encode_git::compare_git_target(ir, digest, &result_ir, &result_refs, default_branch_ref)
}

fn encode_git_check_spill_dir() -> std::io::Result<PathBuf> {
    const MAX_ATTEMPTS: u32 = 64;
    for attempt in 0..MAX_ATTEMPTS {
        let p = unique_temp_path("check-spill", attempt);
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "could not create a temp spill directory",
    ))
}

fn run_encode_git_with_check(
    artifact: &Path,
    out: &Path,
    format: Format,
    check: impl Fn(&Ir, &[u8; 32], &Path, &Path, &GitEnv, &str) -> encode_git::Comparison,
) -> i32 {
    if out.symlink_metadata().is_ok() {
        output::stderr(&format!(
            "{} already exists; encode git refuses to overwrite it",
            out.display()
        ));
        return exit::USAGE;
    }
    let report_path = {
        let mut p = out.as_os_str().to_os_string();
        p.push(".brygge-report");
        PathBuf::from(p)
    };
    // Review 136 F-4: refused up front, exactly as `out` is — today this is silently overwritten.
    if report_path.symlink_metadata().is_ok() {
        output::stderr(&format!(
            "{} already exists; encode git refuses to overwrite it",
            report_path.display()
        ));
        return exit::USAGE;
    }

    let (report, code) = verify_internal_report(artifact, format);
    if code != exit::CLEAN {
        output::stderr(&report);
        return code;
    }

    let decoded = match brygge_ir::from_file(artifact) {
        Ok(d) => d,
        Err(e) => {
            output::stderr(&display::human(&format!(
                "cannot read {}: {e}",
                artifact.display()
            )));
            return exit::FAILURE;
        }
    };
    let ir = &decoded.ir;

    let Some(git) = plugin::find_tool("git") else {
        output::stderr("git is not on PATH: brygge encode git needs it (RFC 019 D-1)");
        return exit::FAILURE;
    };
    let env = match GitEnv::new() {
        Ok(e) => e,
        Err(e) => {
            output::stderr(&format!("cannot prepare git's environment: {e}"));
            return exit::FAILURE;
        }
    };

    let refusals = encode_git_preflight(ir, &git, &env);
    if !refusals.is_empty() {
        output::stderr(&render_encode_git_refusals(format, &refusals));
        return exit::USAGE;
    }

    let default_branch = encode_git::choose_default_branch(ir);

    let parent = out.parent().filter(|p| !p.as_os_str().is_empty());
    let partial = {
        let out_name = out.file_name().map_or_else(
            || "encode-git".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let pid = std::process::id();
        let mut p = None;
        for attempt in 0..64u32 {
            let base_name = format!("{out_name}.partial-{pid}-{attempt}");
            let candidate =
                parent.map_or_else(|| PathBuf::from(&base_name), |d| d.join(&base_name));
            match std::fs::create_dir(&candidate) {
                Ok(()) => {
                    p = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    output::stderr(&format!(
                        "cannot create a working directory beside {}: {e}",
                        out.display()
                    ));
                    return exit::FAILURE;
                }
            }
        }
        match p {
            Some(p) => p,
            None => {
                output::stderr("cannot create a working directory: too many attempts");
                return exit::FAILURE;
            }
        }
    };
    let cleanup = |partial: &Path| {
        let _ = std::fs::remove_dir_all(partial);
    };

    let mut init = Subprocess::new(&git);
    env.apply(&mut init);
    init.current_dir(&partial);
    init.args([
        "init",
        "--quiet",
        "--bare",
        &format!("--initial-branch={}", default_branch.name),
    ]);
    match init.status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            cleanup(&partial);
            output::stderr(&format!("git init failed (exit {:?})", status.code()));
            return exit::FAILURE;
        }
        Err(e) => {
            cleanup(&partial);
            output::stderr(&format!("cannot start git: {e}"));
            return exit::FAILURE;
        }
    }

    let mut import = Subprocess::new(&git);
    env.apply(&mut import);
    import.current_dir(&partial);
    import.args(["fast-import", "--quiet", "--done"]);
    import.stdin(Stdio::piped());
    let mut child = match import.spawn() {
        Ok(c) => c,
        Err(e) => {
            cleanup(&partial);
            output::stderr(&format!("cannot start git fast-import: {e}"));
            return exit::FAILURE;
        }
    };
    let summary = {
        #[allow(clippy::expect_used)]
        let mut stdin = child
            .stdin
            .take()
            .expect("stdin was requested with Stdio::piped()");
        let write_result = encode_git::write_stream(ir, &decoded.digest, &mut stdin);
        drop(stdin);
        write_result
    };
    let status = child.wait();
    let summary = match (summary, status) {
        (Ok(summary), Ok(status)) if status.success() => summary,
        (Ok(_), Ok(status)) => {
            cleanup(&partial);
            output::stderr(&format!(
                "git fast-import failed (exit {:?})",
                status.code()
            ));
            return exit::FAILURE;
        }
        (Err(e), _) => {
            cleanup(&partial);
            output::stderr(&format!("writing the fast-import stream failed: {e}"));
            return exit::FAILURE;
        }
        (Ok(_), Err(e)) => {
            cleanup(&partial);
            output::stderr(&format!("cannot wait for git fast-import: {e}"));
            return exit::FAILURE;
        }
    };

    // RFC 019 D-6 (batch G2): before the partial directory is renamed into place, decode it in-process
    // and check it against the artifact. Any difference refuses the write, same as a pre-flight refusal.
    let default_branch_ref = format!("refs/heads/{}", default_branch.name);
    match check(
        ir,
        &decoded.digest,
        &partial,
        &git,
        &env,
        &default_branch_ref,
    ) {
        encode_git::Comparison::Corresponds => {}
        encode_git::Comparison::DoesNotCorrespond(diffs) => {
            cleanup(&partial);
            output::stderr(&format!(
                "the result does not match the artifact; refusing to write {}:",
                out.display()
            ));
            const SHOWN: usize = 20;
            for d in diffs.iter().take(SHOWN) {
                output::stderr(&format!("  {}", display::human(d)));
            }
            if diffs.len() > SHOWN {
                output::stderr(&format!("  …and {} more", diffs.len() - SHOWN));
            }
            return exit::FAILURE;
        }
        encode_git::Comparison::NotChecked(reason) => {
            cleanup(&partial);
            output::stderr(&format!(
                "the result could not be checked against the artifact: {reason}"
            ));
            return exit::FAILURE;
        }
    }

    // Review 136 F-4: the report is written to a temporary sibling, and checked written, *before* the
    // repository is renamed into place — so a failure here still leaves nothing at `<dir>`, matching
    // every other failure path, instead of a complete repository with no report beside it.
    let fidelity = brygge_ir::honesty::summary(ir);
    let report_text =
        render_encode_git_report(&decoded.digest, &default_branch, &summary, &fidelity);
    let report_dir = report_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let report_tmp = {
        let report_name = report_path.file_name().map_or_else(
            || "brygge-report".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let pid = std::process::id();
        let mut p = None;
        for attempt in 0..64u32 {
            let candidate = report_dir.join(format!("{report_name}.partial-{pid}-{attempt}"));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
                .and_then(|mut f| f.write_all(report_text.as_bytes()).map(|()| f))
            {
                Ok(_) => {
                    p = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    cleanup(&partial);
                    let _ = std::fs::remove_file(&candidate);
                    output::stderr(&format!(
                        "cannot write a temporary report beside {}: {e}",
                        report_path.display()
                    ));
                    return exit::FAILURE;
                }
            }
        }
        match p {
            Some(p) => p,
            None => {
                cleanup(&partial);
                output::stderr("cannot write a temporary report: too many attempts");
                return exit::FAILURE;
            }
        }
    };

    if let Err(e) = std::fs::rename(&partial, out) {
        cleanup(&partial);
        let _ = std::fs::remove_file(&report_tmp);
        output::stderr(&format!("cannot write {}: {e}", out.display()));
        return exit::FAILURE;
    }
    if let Err(e) = std::fs::rename(&report_tmp, &report_path) {
        let _ = std::fs::remove_file(&report_tmp);
        output::stderr(&format!(
            "the repository was written at {}, but its report could not be put at {}: {e}",
            out.display(),
            report_path.display()
        ));
        return exit::FAILURE;
    }

    // RFC 021 D-4 (batch G): the same sentence `honesty.rs`'s `render_human` would print, since this
    // line is a single stderr message rather than the rendered report blob — kept byte-identical to it
    // by hand, so a reader sees the same words whichever of the two this build showed them.
    let submodules_suffix = if fidelity.submodule_paths > 0 {
        format!(
            "; submodules: {} path(s) in the history, {} at the branch and tag tips; their own \
             repositories were not read, imported or verified",
            fidelity.submodule_paths, fidelity.submodule_at_tips
        )
    } else {
        String::new()
    };
    output::stderr(&format!(
        "wrote {} ({} commit(s), {} tag(s), {} ref(s) total; default branch {}{}){submodules_suffix}",
        out.display(),
        summary.commits,
        summary.tags,
        summary.refs,
        default_branch.name,
        if default_branch.existed {
            ""
        } else {
            ", which has no commits"
        }
    ));
    fidelity_exit_class(ir)
}

// ---- transform (RFC 018 D-6) -----------------------------------------------------------------------

// RFC 019 D-4 (batch G3): bumped for `transformer.N.shadowed_by`, added when `authors` became a
// built-in — the same bump `ENCODE_LIST_VERSION` got for `git`.
const TRANSFORM_LIST_VERSION: u32 = 2;
const TRANSFORM_VERSION: u32 = 1;
// RFC 019 D-4 (batch G3): the built-in `transform authors` has no plugin banner to carry
// `transform_version`'s own line, so it gets its own version instead of silently going unversioned.
const TRANSFORM_AUTHORS_VERSION: u32 = 1;

/// `transform --list [--format …]`: every `brygge-transform-*` plugin [`plugin::list`] finds on `PATH`.
pub fn run_transform_list(format: Format) -> i32 {
    let plugins = plugin::list(plugin::Kind::Transform);
    let rendered = match format {
        Format::Human => render_transform_list_human(&plugins),
        Format::Machine => render_transform_list_machine(&plugins),
    };
    if let Some(code) = output::stdout(&rendered) {
        return code;
    }
    exit::CLEAN
}

fn render_transform_list_human(plugins: &[plugin::Listed]) -> String {
    let mut s = String::new();
    if plugins.is_empty() {
        let _ = writeln!(s, "no brygge-transform-* plugins found on PATH");
        return s;
    }
    for p in plugins {
        let _ = write!(s, "{}  {}", p.name, p.path.display());
        if p.name == AUTHORS_TRANSFORM {
            let _ = write!(s, "  (shadowed by the built-in)");
        } else if p.shadowed {
            let _ = write!(s, "  (shadowed)");
        }
        let _ = writeln!(s);
    }
    s
}

fn render_transform_list_machine(plugins: &[plugin::Listed]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "transform_list_version={TRANSFORM_LIST_VERSION}");
    for (i, p) in plugins.iter().enumerate() {
        let _ = writeln!(
            s,
            "transformer.{i}.name={}",
            display::machine_value(&p.name)
        );
        let _ = writeln!(
            s,
            "transformer.{i}.path={}",
            display::machine_value(&p.path.display().to_string())
        );
        // `authors` is always shadowed (the built-in always wins), regardless of plugin::list's own
        // PATH-position-based `shadowed` computation, which knows nothing of the reservation — the same
        // treatment `encode`'s `git` target gets in `render_encode_list_machine`.
        let _ = writeln!(
            s,
            "transformer.{i}.shadowed={}",
            p.shadowed || p.name == AUTHORS_TRANSFORM
        );
        if p.name == AUTHORS_TRANSFORM {
            let _ = writeln!(s, "transformer.{i}.shadowed_by=built_in");
        }
    }
    s
}

/// Whether `a` and `b` name the same file (canonicalized compare, per the handoff's own "--out must not
/// be the same file as `<artifact>`" rule). Falls back to a plain path compare when either side cannot
/// be canonicalized (typically `out`, which usually does not exist yet) — a non-existent path can never
/// be the same *file* as one that does exist, but comparing the given paths directly still catches the
/// common case of passing the same literal path twice.
fn same_file_as(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Reserves a unique, visible temp file for `transform`'s own plugin output, in `dir` (`--out`'s own
/// directory): named `.brygge-transform-<pid>-<nanos>-<attempt>`, `create_new` so a pre-existing file or
/// symlink there is never followed or truncated, mode `0600` on Unix — the same naming/permission
/// convention [`crate::content::spill`]'s own file uses in brygge-ir. Unlike that one, this file is
/// never unlinked here: the plugin needs a real path to write its own output into, and the caller
/// (`run_transform`) removes or renames it once the plugin has run.
fn create_transform_temp(dir: &Path) -> std::io::Result<std::path::PathBuf> {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    const MAX_ATTEMPTS: u32 = 64;
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..MAX_ATTEMPTS {
        let path = dir.join(format!(".brygge-transform-{pid}-{nanos}-{attempt}"));
        let opened = open_transform_temp(&path);
        match opened {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("could not create a temp file")))
}

#[cfg(unix)]
fn open_transform_temp(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
}

#[cfg(not(unix))]
fn open_transform_temp(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
}

/// As [`encode_banner_text`], for `transform` (RFC 018 D-6 §3): the plugin's own output is never
/// trusted on brygge's say-so either — it still has to pass `brygge_ir::transform::check`.
fn transform_banner_text(format: Format, name: &str, path: &Path) -> String {
    match format {
        Format::Human => format!(
            "transforming with the external plugin `brygge-transform-{name}` (`{}`): not part of \
             brygge; brygge verified the input artifact, and will check the plugin's own output before \
             accepting it",
            path.display()
        ),
        Format::Machine => format!(
            "transform_version={TRANSFORM_VERSION}\ntransform.name={}\ntransform.plugin_path={}\n\
             transform.banner=not_part_of_brygge",
            display::machine_value(name),
            display::machine_value(&path.display().to_string()),
        ),
    }
}

fn transform_exit_text(format: Format, name: &str, code: i32) -> String {
    match format {
        Format::Human => format!("`brygge-transform-{name}` exited with {code}"),
        Format::Machine => format!("transform.exit_code={code}"),
    }
}

fn transform_signal_text(format: Format, name: &str, signal: i32) -> String {
    match format {
        Format::Human => match plugin::signal_name(signal) {
            Some(n) => format!("`brygge-transform-{name}` was killed by signal {signal} ({n})"),
            None => format!("`brygge-transform-{name}` was killed by signal {signal}"),
        },
        Format::Machine => format!("transform.signal={signal}"),
    }
}

/// The one result line on a successfully accepted transform: the new step's own index plus the counts
/// [`brygge_ir::transform::check`] itself computed while accepting it.
fn transform_result_text(format: Format, summary: &brygge_ir::transform::StepSummary) -> String {
    match format {
        Format::Human => format!(
            "step {}: {} atom(s) changed, {} op(s) changed, {} ref(s) changed, {} dropped",
            summary.step,
            summary.atoms_changed,
            summary.ops_changed,
            summary.refs_changed,
            summary.dropped
        ),
        Format::Machine => format!(
            "transform.step={}\ntransform.atoms_changed={}\ntransform.ops_changed={}\n\
             transform.refs_changed={}\ntransform.dropped={}",
            summary.step,
            summary.atoms_changed,
            summary.ops_changed,
            summary.refs_changed,
            summary.dropped
        ),
    }
}

/// `transform <name> <artifact> --out <file> [-- <plugin arguments>…]` (RFC 018 D-6 §3). Resolves
/// `brygge-transform-<name>` on `PATH` ([`plugin::find`] — never the bare name), verifies the input
/// artifact first exactly as `encode` does (refusing to run the plugin on anything but a clean `pass`),
/// then refuses an input with two atoms sharing one source identity (nothing downstream could match
/// atoms by it at all). The plugin writes its own output to a private, visible temp file in `--out`'s
/// own directory; only once that output has itself passed every internal check and
/// [`brygge_ir::transform::check`] is it renamed over `--out` — exactly as `decode --out` behaves,
/// silently overwriting whatever was there. Any failure along the way removes the temp file and leaves
/// `--out` untouched.
pub fn run_transform(
    name: &str,
    artifact: &Path,
    out: &Path,
    plugin_args: &[String],
    format: Format,
) -> i32 {
    if same_file_as(out, artifact) {
        output::stderr("--out must not be the same file as <artifact>");
        return exit::USAGE;
    }

    let Some(path) = plugin::find(plugin::Kind::Transform, name) else {
        output::stderr(&format!(
            "no transform for '{name}': brygge-transform-{name} is not on PATH (see `brygge transform \
             --list`)"
        ));
        return exit::USAGE;
    };

    let (report, code) = verify_internal_report(artifact, format);
    if code != exit::CLEAN {
        output::stderr(&report);
        return code;
    }

    let input = match brygge_ir::from_file(artifact) {
        Ok(d) => d,
        Err(e) => {
            output::stderr(&display::human(&format!(
                "cannot read {}: {e}",
                artifact.display()
            )));
            return exit::FAILURE;
        }
    };
    if let Some(dup) = brygge_ir::transform::duplicate_source_identity(&input) {
        output::stderr(&display::human(&format!(
            "refused: the input has two atoms with the same source identity ({dup:?}) — a transform \
             cannot match atoms by it at all"
        )));
        return exit::VERIFY_FAILED;
    }

    output::stderr(&transform_banner_text(format, name, &path));

    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp = match create_transform_temp(dir) {
        Ok(p) => p,
        Err(e) => {
            output::stderr(&format!(
                "cannot create a temp file in {}: {e}",
                dir.display()
            ));
            return exit::FAILURE;
        }
    };

    let mut argv: Vec<std::ffi::OsString> = vec![
        "--artifact".into(),
        artifact.as_os_str().to_os_string(),
        "--out".into(),
        tmp.as_os_str().to_os_string(),
    ];
    argv.extend(plugin_args.iter().map(std::ffi::OsString::from));

    match plugin::run(&path, &argv) {
        plugin::Ran::Exited(0) => {}
        plugin::Ran::Exited(code) => {
            let _ = std::fs::remove_file(&tmp);
            output::stderr(&transform_exit_text(format, name, code));
            return code;
        }
        plugin::Ran::Signaled(signal) => {
            let _ = std::fs::remove_file(&tmp);
            output::stderr(&transform_signal_text(format, name, signal));
            return exit::FAILURE;
        }
        plugin::Ran::CannotStart(e) => {
            let _ = std::fs::remove_file(&tmp);
            output::stderr(&format!(
                "cannot start brygge-transform-{name} ({}): {e}",
                path.display()
            ));
            return exit::FAILURE;
        }
    }

    let parsed_output = brygge_ir::from_file(&tmp);
    let (checks, internal_pass) = compute_internal_checks(&parsed_output);
    if !internal_pass {
        let _ = std::fs::remove_file(&tmp);
        let detail = checks
            .iter()
            .find_map(|(name, outcome)| match outcome {
                CheckOutcome::Fail(d) => Some(format!("{name}: {d}")),
                _ => None,
            })
            .unwrap_or_else(|| "an internal check failed".to_string());
        output::stderr(&display::human(&format!(
            "the transform's output was refused: {detail}"
        )));
        return exit::VERIFY_FAILED;
    }
    let Ok(output_decoded) = parsed_output else {
        // Unreachable: `internal_pass` is only true when `compute_internal_checks` saw `Ok(_)` (its
        // "integrity" check fails immediately on `Err`, which already returned above).
        let _ = std::fs::remove_file(&tmp);
        return exit::FAILURE;
    };

    match brygge_ir::transform::check(&input, &output_decoded, name) {
        Ok(summary) => {
            let final_exit = fidelity_exit_class(&output_decoded.ir);
            // Drop the output's own `Decoded` (and the open handle its content store holds on `tmp`,
            // RFC 015 D-4) before renaming `tmp` out from under it — renaming an open file is not
            // guaranteed to work on every platform.
            drop(output_decoded);
            if let Err(e) = std::fs::rename(&tmp, out) {
                let _ = std::fs::remove_file(&tmp);
                output::stderr(&format!("cannot write {}: {e}", out.display()));
                return exit::FAILURE;
            }
            output::stderr(&transform_result_text(format, &summary));
            final_exit
        }
        Err(violation) => {
            let _ = std::fs::remove_file(&tmp);
            output::stderr(&display::human(&format!(
                "the transform's output was refused: {violation}"
            )));
            exit::VERIFY_FAILED
        }
    }
}

/// One identity `transform authors` could not map, as a unit it lists on stderr.
fn unmapped_line(identity_count: &authors::IdentityCount) -> String {
    let mut counts = Vec::new();
    if identity_count.author > 0 {
        counts.push(format!("{} author", identity_count.author));
    }
    if identity_count.committer > 0 {
        counts.push(format!("{} committer", identity_count.committer));
    }
    if identity_count.tagger > 0 {
        counts.push(format!("{} tagger", identity_count.tagger));
    }
    format!(
        "  {}  ({})",
        display::human(&authors::identity_as_map_source(&identity_count.identity)),
        counts.join(", ")
    )
}

/// As [`transform_result_text`], for the built-in `authors` transform (RFC 019 D-4 §3): the same
/// `transform.*` keys, plus `transform.authors.mapped`/`.unmapped` — the distinct-identity counts no
/// other transform reports.
fn transform_authors_result_text(
    format: Format,
    summary: &brygge_ir::transform::StepSummary,
    mapped: u64,
    unmapped: u64,
) -> String {
    match format {
        Format::Human => format!(
            "step {}: {} atom(s) changed, {} ref(s) changed, {} dropped; {} identit{} mapped, {} left \
             unmapped",
            summary.step,
            summary.atoms_changed,
            summary.refs_changed,
            summary.dropped,
            mapped,
            if mapped == 1 { "y" } else { "ies" },
            unmapped
        ),
        Format::Machine => format!(
            "transform_authors_version={TRANSFORM_AUTHORS_VERSION}\ntransform.step={}\n\
             transform.atoms_changed={}\ntransform.ops_changed={}\ntransform.refs_changed={}\n\
             transform.dropped={}\ntransform.authors.mapped={mapped}\n\
             transform.authors.unmapped={unmapped}",
            summary.step,
            summary.atoms_changed,
            summary.ops_changed,
            summary.refs_changed,
            summary.dropped,
        ),
    }
}

/// `transform authors <artifact> --map <file> --out <file> [--require-all]` (RFC 019 D-4 §3): the one
/// built-in transform, run in-process through [`brygge_ir::transform::Edit`] exactly as a plugin would
/// build its own output — never a subprocess, never a temp file for the *input* side, but still the
/// same output check, atomic write, and `--out` rules `transform`'s plugin path uses.
pub fn run_transform_authors(
    artifact: &Path,
    map_path: &Path,
    out: &Path,
    require_all: bool,
    format: Format,
) -> i32 {
    if same_file_as(out, artifact) {
        output::stderr("--out must not be the same file as <artifact>");
        return exit::USAGE;
    }

    let map_bytes = match std::fs::read(map_path) {
        Ok(b) => b,
        Err(e) => {
            output::stderr(&format!("cannot read {}: {e}", map_path.display()));
            return exit::FAILURE;
        }
    };
    let map = match authors::parse_map(&map_bytes) {
        Ok(m) => m,
        Err(problems) => {
            for p in &problems {
                output::stderr(p);
            }
            return exit::USAGE;
        }
    };

    let input = match brygge_ir::from_file(artifact) {
        Ok(d) => d,
        Err(e) => {
            output::stderr(&display::human(&format!(
                "cannot read {}: {e}",
                artifact.display()
            )));
            return exit::FAILURE;
        }
    };
    if let Some(dup) = brygge_ir::transform::duplicate_source_identity(&input) {
        output::stderr(&display::human(&format!(
            "refused: the input has two atoms with the same source identity ({dup:?}) — a transform \
             cannot match atoms by it at all"
        )));
        return exit::VERIFY_FAILED;
    }

    let identities = authors::collect_identities(&input.ir);
    let mut hits = vec![0u64; map.mappings.len()];
    let mut unmapped = Vec::new();
    for ic in &identities {
        match authors::lookup(&map, &ic.identity).and_then(|idx| hits.get_mut(idx)) {
            Some(hit) => *hit += 1,
            None => unmapped.push(ic),
        }
    }
    if !unmapped.is_empty() {
        output::stderr(&format!(
            "{} identit{} left unmapped:",
            unmapped.len(),
            if unmapped.len() == 1 { "y" } else { "ies" }
        ));
        for ic in &unmapped {
            output::stderr(&unmapped_line(ic));
        }
    }
    if require_all && !unmapped.is_empty() {
        output::stderr(
            "refused: --require-all needs every identity mapped (see the list above); nothing written",
        );
        return exit::USAGE;
    }
    for (mapping, hit) in map.mappings.iter().zip(&hits) {
        if *hit == 0 {
            output::stderr(&format!(
                "warning: map line {} matched nothing",
                mapping.line
            ));
        }
    }
    let mapped_count = (identities.len() - unmapped.len()) as u64;

    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "map_sha256".to_string(),
        brygge_ir::BlobId::of(&map_bytes).to_hex(),
    );
    params.insert("require_all".to_string(), require_all.to_string());

    let mut edit =
        brygge_ir::transform::Edit::begin(&input, "authors", env!("CARGO_PKG_VERSION"), params);
    for (in_atom, atom_edit) in input.ir.atoms.iter().zip(edit.atoms()) {
        if let Some(author) = &in_atom.metadata.author {
            if let Some(target) = authors::remap(&map, author) {
                atom_edit.set_author(Some(target.clone()));
            }
        }
        if let Some(committer) = &in_atom.metadata.committer {
            if let Some(target) = authors::remap(&map, committer) {
                atom_edit.set_committer(Some(target.clone()));
            }
        }
    }
    for r in &input.ir.refs {
        let Some(tagger) = r.annotation.as_ref().and_then(|a| a.tagger.as_ref()) else {
            continue;
        };
        let Some(target) = authors::remap(&map, tagger) else {
            continue;
        };
        let mut new_annotation = r.annotation.clone().unwrap_or_default();
        new_annotation.tagger = Some(target.clone());
        if let Ok(mut ref_edit) = edit.ref_mut(r.kind.clone(), &r.name) {
            ref_edit.set_annotation(Some(new_annotation));
        }
    }

    let output_ir = match edit.finish() {
        Ok(ir) => ir,
        Err(e) => {
            let (code, msg) = map_transform_finish_err(e);
            output::stderr(&display::human(&msg));
            return code;
        }
    };

    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp = match create_transform_temp(dir) {
        Ok(p) => p,
        Err(e) => {
            output::stderr(&format!(
                "cannot create a temp file in {}: {e}",
                dir.display()
            ));
            return exit::FAILURE;
        }
    };
    if let Err(e) = std::fs::write(&tmp, brygge_ir::to_bytes(&output_ir)) {
        let _ = std::fs::remove_file(&tmp);
        output::stderr(&format!(
            "cannot write a temp file in {}: {e}",
            dir.display()
        ));
        return exit::FAILURE;
    }

    let parsed_output = brygge_ir::from_file(&tmp);
    let (checks, internal_pass) = compute_internal_checks(&parsed_output);
    if !internal_pass {
        let _ = std::fs::remove_file(&tmp);
        let detail = checks
            .iter()
            .find_map(|(name, outcome)| match outcome {
                CheckOutcome::Fail(d) => Some(format!("{name}: {d}")),
                _ => None,
            })
            .unwrap_or_else(|| "an internal check failed".to_string());
        output::stderr(&display::human(&format!(
            "the transform's output was refused: {detail}"
        )));
        return exit::VERIFY_FAILED;
    }
    let Ok(output_decoded) = parsed_output else {
        // Unreachable, as in `run_transform`: `internal_pass` is only true on `Ok(_)`.
        let _ = std::fs::remove_file(&tmp);
        return exit::FAILURE;
    };

    match brygge_ir::transform::check(&input, &output_decoded, "authors") {
        Ok(summary) => {
            let final_exit = fidelity_exit_class(&output_decoded.ir);
            drop(output_decoded);
            if let Err(e) = std::fs::rename(&tmp, out) {
                let _ = std::fs::remove_file(&tmp);
                output::stderr(&format!("cannot write {}: {e}", out.display()));
                return exit::FAILURE;
            }
            output::stderr(&transform_authors_result_text(
                format,
                &summary,
                mapped_count,
                unmapped.len() as u64,
            ));
            final_exit
        }
        Err(violation) => {
            let _ = std::fs::remove_file(&tmp);
            output::stderr(&display::human(&format!(
                "the transform's output was refused: {violation}"
            )));
            exit::VERIFY_FAILED
        }
    }
}

#[cfg(test)]
#[allow(clippy::print_stdout, clippy::print_stderr)]
// test diagnostics ("skipping: ... not on PATH")
mod tests;

#[cfg(test)]
mod mutation;

#[cfg(test)]
mod encode_tests;

#[cfg(test)]
mod transform_tests;

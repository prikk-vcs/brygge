//! The command implementations for the three-verb surface (handoff `cli-and-verify-handoff-v2.md`,
//! adapted to IR contract 0.2.0 by the RFC 011 handoff). Each returns a CL-08 exit code and prints its
//! own output; `main` maps the code to `std::process::exit`. Every string that can originate in a source
//! repository is routed through [`crate::display`] before it reaches stdout or stderr (CR-19).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as IoWrite;
use std::path::Path;

use brygge_decode_cvs::{Error as CvsError, Options as CvsOptions, Source as CvsSource};
use brygge_decode_git::Error as GitError;
use brygge_decode_hg::Error as HgError;
use brygge_decode_svn::{
    Error as SvnError, LayoutPolicy, Options as SvnOptions, Source as SvnSource,
};
use brygge_ir::model::{AtomId, FlagKind, PathOp, RefKind, Text};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};
use brygge_ir::{Decoded, Ir, LossClass};

use crate::cli::{Format, SourceKind};
use crate::display;
use crate::exit;

const VERIFY_VERSION: u32 = 4;
const INSPECT_VERSION: u32 = 4;

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
        EpistemicStatus::Stated => None,
    }
}

/// The fixed, lowercase label for a loss class (never `{:?}`).
fn loss_class_label(class: LossClass) -> &'static str {
    match class {
        LossClass::Representation => "representation",
        LossClass::AdvisoryUnreliable => "advisory-unreliable",
        LossClass::Other => "other",
    }
}

/// The fixed, lowercase label for a flag kind (never `{:?}`).
fn flag_kind_label(kind: FlagKind) -> &'static str {
    match kind {
        FlagKind::ConventionViolation => "convention-violation",
        FlagKind::BelowConfidenceFloor => "below-confidence-floor",
    }
}

/// `stated` or `derived:<kind>` — machine-safe (both halves are fixed Rust-enum-derived labels, never
/// source text).
fn status_label_machine(s: &EpistemicStatus) -> String {
    match s {
        EpistemicStatus::Stated => "stated".to_string(),
        EpistemicStatus::Derived(d) => format!("derived:{}", d.kind.label()),
    }
}

/// `stated` or `derived:<kind> {k=v, …}` with confidence where present (human form only — handoff
/// §3.4.2). Parameter values are neutralized; the kind label and parameter keys are fixed decoder-chosen
/// literals, never source text.
fn status_label_human(s: &EpistemicStatus) -> String {
    let EpistemicStatus::Derived(d) = s else {
        return "stated".to_string();
    };
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

fn read_ir(path: &Path) -> Result<Decoded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    brygge_ir::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
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

/// The SVN layout policy recorded in the IR's provenance (default if absent/unparsable).
fn layout_from_provenance(ir: &Ir) -> LayoutPolicy {
    ir.provenance
        .params
        .get("layout")
        .and_then(|l| LayoutPolicy::from_label(l))
        .unwrap_or_default()
}

/// The per-source knobs a decode needs: git uses `infer_renames`; svn uses `reconstruct_refs`/`layout`;
/// cvs uses `reconstruct_refs`/`cvs_window`/`cvs_floor`. Built from CLI flags (decode) or from the
/// artifact's provenance (against-source, so a re-decode reproduces the recorded import).
struct SourceOpts {
    infer_renames: bool,
    reconstruct_refs: bool,
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
        other => (exit::FAILURE, format!("decode failed: {other}")),
    }
}

/// Decode `repo` with the chosen source decoder, mapping decoder errors to a `(exit code, message)`.
/// A refused format/feature is a clean refusal (`FLOOR_REFUSAL`), not a generic failure. The decoder
/// invocation itself is panic-guarded (`guard_decoder`, CR-16) for all four sources, since this is the
/// single path both `decode` and `verify --against-source` use.
fn decode_source(kind: SourceKind, repo: &Path, opts: &SourceOpts) -> Result<Ir, (i32, String)> {
    match kind {
        SourceKind::Cvs => {
            let src = CvsSource::LocalRepo(repo.to_path_buf());
            let cvs = CvsOptions {
                window_secs: opts.cvs_window,
                confidence_floor: opts.cvs_floor,
                reconstruct_refs: opts.reconstruct_refs,
            };
            guard_decoder(|| brygge_decode_cvs::decode(&src, &cvs))
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
            };
            guard_decoder(|| brygge_decode_svn::decode(&src, &svn))
                .map_err(|fault| (exit::FAILURE, fault))?
                .map_err(map_svn_error)
        }
        SourceKind::Git => {
            let git = brygge_decode_git::Options {
                infer_renames: opts.infer_renames,
                rename_threshold: 100,
            };
            guard_decoder(|| brygge_decode_git::decode(repo, &git))
                .map_err(|fault| (exit::FAILURE, fault))?
                .map_err(map_git_error)
        }
        SourceKind::Hg => {
            // Mercurial records its renames, so there is nothing to infer and no inference option.
            let hg = brygge_decode_hg::Options::default();
            guard_decoder(|| brygge_decode_hg::decode(repo, &hg))
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

/// Write `bytes` atomically to `out`: a temp file in the same directory, flushed and `sync_all`'d, then
/// renamed over `out`. The temp file is created with `create_new`, so a pre-existing file or symlink at
/// that path is an error — never followed, never truncated, and never removed (it is not ours). On any
/// later failure the temp file we created is removed and `out` is untouched.
fn atomic_write(out: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
    let result = (|| -> std::io::Result<()> {
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, out)?;
        // The rename is atomic but not yet durable across a power loss until the directory entry
        // itself is synced. Best-effort: some platforms cannot open a directory as a file at all, and
        // the rename has already succeeded either way, so a failure here is not the write's failure.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `decode <git|hg|svn|cvs> <source> --out <artifact>` (CL-01, FL-01/FL-10).
pub fn run_decode(
    kind: SourceKind,
    source: &Path,
    out: &Path,
    infer_renames: bool,
    reconstruct_refs: bool,
    format: Format,
) -> i32 {
    eprintln!("{}", faithfulness_statement(kind));

    let default_cvs = brygge_decode_cvs::Options::default();
    let ir = match decode_source(
        kind,
        source,
        &SourceOpts {
            infer_renames,
            reconstruct_refs,
            layout: LayoutPolicy::default(),
            cvs_window: default_cvs.window_secs,
            cvs_floor: default_cvs.confidence_floor,
        },
    ) {
        Ok(ir) => ir,
        Err((code, msg)) => {
            eprintln!("{}", display::human(&msg));
            return code;
        }
    };

    let bytes = brygge_ir::to_bytes(&ir);
    if let Err(e) = atomic_write(out, &bytes) {
        eprintln!("cannot write {}: {e}", out.display());
        return exit::FAILURE;
    }
    eprintln!("wrote {} ({} bytes)", out.display(), bytes.len());

    // The end-of-run report is `inspect`'s default report (FS-02): the exact same call.
    let report = brygge_ir::honesty::summary(&ir);
    match format {
        Format::Human => print!("{}", report.render_human()),
        Format::Machine => print!("{}", report.render_machine()),
    }
    // Exit class (CL-08): a flagged condition (RFC 011 D-8 — an unresolved SVN layout, a CVS
    // reconstruction under the confidence floor) takes precedence; else recorded loss if any
    // non-representation drop exists; else clean.
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
            eprintln!("{}", display::human(&e));
            return exit::FAILURE;
        }
    };
    if decoded.skipped_non_critical_fields > 0 {
        eprintln!(
            "note: {} field(s) from a newer contract were not understood and were skipped (none of \
             them can change what this artifact claims)",
            decoded.skipped_non_critical_fields
        );
    }
    let ir = &decoded.ir;
    let report = brygge_ir::honesty::summary(ir)
        .with_skipped_non_critical_fields(decoded.skipped_non_critical_fields);
    match format {
        Format::Human => {
            print!("{}", report.render_human());
            if atoms {
                print!("{}", render_atoms_human(ir));
            }
        }
        Format::Machine => {
            print!("{}", report.render_machine());
            if atoms {
                print!("{}", render_atoms_machine(ir));
            }
        }
    }
    exit::CLEAN
}

fn render_atoms_human(ir: &Ir) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "IR (contract {}): {} atom(s), {} ref(s), {} blob(s)",
        brygge_ir::version::CURRENT,
        ir.atoms.len(),
        ir.refs.len(),
        ir.content.len()
    );
    let _ = writeln!(s, "atoms (topological order):");
    for atom in &ir.atoms {
        let subject = message_subject(atom.metadata.message.as_ref());
        let _ = writeln!(
            s,
            "  {}  [{}]  src:{}  {} op(s)  {}",
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

fn render_atoms_machine(ir: &Ir) -> String {
    // `atoms=` is the fidelity report's line; this listing does not repeat it (one key, one line).
    let mut s = String::new();
    let _ = writeln!(s, "inspect_version={INSPECT_VERSION}");
    let _ = writeln!(s, "contract_version={}", brygge_ir::version::CURRENT);
    for (i, atom) in ir.atoms.iter().enumerate() {
        let _ = writeln!(s, "atom.{i}.id={}", hex(&atom.id.0));
        let _ = writeln!(s, "atom.{i}.status={}", status_label_machine(&atom.status));
        if let Some(name) = status_other(&atom.status) {
            let _ = writeln!(s, "atom.{i}.status_other={}", display::machine_value(name));
        }
        let _ = writeln!(s, "atom.{i}.source_id={}", hex(&atom.source.atom_id));
        let _ = writeln!(s, "atom.{i}.ops={}", atom.ops.len());
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
    let mut trees: HashMap<AtomId, BTreeSet<String>> = HashMap::new();
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
            None => BTreeSet::new(),
        };

        // Copies are checked against the tree state *before* this atom's own ops apply, since
        // `from_atom` is always an earlier atom, never this one (RFC 011 §2.5).
        for copy in &atom.copies {
            let source_tree_has_it = if Some(copy.from_atom) == p0 {
                tree.contains(&copy.from)
            } else {
                match trees.get(&copy.from_atom) {
                    Some(t) => t.contains(&copy.from),
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
                PathOp::Add { path, .. } => {
                    if !tree.insert(path.clone()) {
                        fail!("an Add op names an already-present path");
                    }
                }
                PathOp::Modify { path, .. } | PathOp::Replace { path, .. } => {
                    if !tree.contains(path) {
                        fail!("a Modify/Replace op names an absent path");
                    }
                }
                PathOp::Delete { path, .. } => {
                    if !tree.remove(path) {
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
                    &atom.status,
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
                if !rf.status.is_derived() {
                    return CheckOutcome::Fail("a CVS ref is not Derived".to_string());
                }
            }
        }
        brygge_ir::SourceKind::Svn => {
            for atom in &ir.atoms {
                if atom.status.is_derived() {
                    return CheckOutcome::Fail(
                        "an SVN atom is Derived; SVN's revision spine is always Stated".to_string(),
                    );
                }
                if atom.parents.len() > 1 {
                    return CheckOutcome::Fail(
                        "an SVN atom has more than one parent; SVN has no DAG".to_string(),
                    );
                }
            }
            for rf in &ir.refs {
                let ok = matches!(
                    &rf.status,
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
                    if !copy.status.is_derived() {
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
enum AgainstSourceOutcome {
    /// `Some(note)` carries an informational note even on success (e.g. CR-07.6: the two decodes used
    /// different `svnadmin` versions but the history is identical) — never a reason to doubt the result.
    Corresponds(Option<String>),
    DoesNotCorrespond(String),
    Reproduces,
    DoesNotReproduce(String),
    NotChecked(String),
    NotRun,
}

impl AgainstSourceOutcome {
    fn machine_label(&self) -> &'static str {
        match self {
            Self::Corresponds(_) => "corresponds",
            Self::DoesNotCorrespond(_) => "does-not-correspond",
            Self::Reproduces => "reproduces",
            Self::DoesNotReproduce(_) => "does-not-reproduce",
            Self::NotChecked(_) => "not-checked",
            Self::NotRun => "not-run",
        }
    }
    fn human_label(&self) -> &'static str {
        match self {
            Self::Corresponds(_) => "corresponds",
            Self::DoesNotCorrespond(_) => "does not correspond",
            Self::Reproduces => "reproduces",
            Self::DoesNotReproduce(_) => "does not reproduce",
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
    fn note(&self) -> Option<&str> {
        match self {
            Self::Corresponds(Some(n)) => Some(n),
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
fn run_against_source(ir1: &Ir, repo: &Path) -> AgainstSourceOutcome {
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
    let opts = SourceOpts {
        // Only Git infers renames; no other artifact records the parameter, so none is read.
        infer_renames: kind == SourceKind::Git && infer_renames_from_provenance(ir1),
        reconstruct_refs: reconstruct_refs_from_provenance(ir1),
        layout: layout_from_provenance(ir1),
        cvs_window: cvs_window_from_provenance(ir1),
        cvs_floor: cvs_floor_from_provenance(ir1),
    };
    let is_cvs = kind == SourceKind::Cvs;
    let ir2 = match decode_source(kind, repo, &opts) {
        Ok(ir) => ir,
        Err((_, msg)) => {
            return AgainstSourceOutcome::NotChecked(format!("cannot re-decode source: {msg}"));
        }
    };
    // CR-07.6, review 010 R-1: `svnadmin`'s own version is a fact about the tool, not the history, so an
    // upgrade between the two decodes must never by itself make this comparison fail. Compare against a
    // copy of `ir2` with that one param aligned to `ir1`'s; keep the original `ir2` around for the
    // version-mismatch note, which still reports honestly when the versions actually differed.
    let ir2_for_compare = align_svnadmin_version(ir1, ir2.clone());
    // Nothing else in the IR is provenance-only-and-volatile any more (import_time was removed by
    // RFC 011), so the aligned decodes are compared directly.
    let corresponds = ir1 == &ir2_for_compare;
    let versions_differ = svn_version_note(ir1, &ir2);
    let detail = || {
        let d = divergence(ir1, &ir2_for_compare);
        match &versions_differ {
            Some(v) => format!("{d} (svnadmin versions differ: {v})"),
            None => d,
        }
    };
    match (is_cvs, corresponds) {
        (true, true) => AgainstSourceOutcome::Reproduces,
        (true, false) => AgainstSourceOutcome::DoesNotReproduce(detail()),
        (false, true) => AgainstSourceOutcome::Corresponds(
            versions_differ
                .map(|v| format!("svnadmin versions differ ({v}); the history is identical")),
        ),
        (false, false) => AgainstSourceOutcome::DoesNotCorrespond(detail()),
    }
}

/// `Some(reason)` when `repo`'s form (a dumpfile vs a live repository) differs from what `ir1`'s
/// provenance recorded (CR-07.6). `None` when there is nothing recorded to compare (an artifact from
/// before this handoff) or the forms already match.
fn svn_source_form_mismatch(ir1: &Ir, repo: &Path) -> Option<String> {
    let recorded = ir1.provenance.params.get("source_form")?;
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
    fn from(internal_pass: bool, against: &AgainstSourceOutcome) -> Self {
        if !internal_pass || against.is_mismatch() {
            Self::Fail
        } else if against.is_not_checked() {
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

/// `verify <artifact> [--against-source <source>]` (CL-04, CR-02): the internal checks always run; each
/// can fail. With `--against-source`, additionally re-derive and report the two results separately
/// (VF-4). Exit `50` if anything fails; exit `1` if a requested `--against-source` could not be checked
/// at all and nothing that ran failed (the three-valued verdict, review 003 R-5).
pub fn run_verify(artifact: &Path, against_source: Option<&Path>, format: Format) -> i32 {
    let bytes = match std::fs::read(artifact) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {}: {e}", artifact.display());
            return exit::FAILURE;
        }
    };
    let parsed = brygge_ir::from_bytes(&bytes);
    let mut checks: Vec<(&'static str, CheckOutcome)> = Vec::with_capacity(CHECK_NAMES.len());
    let ir_opt = match &parsed {
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

    let against = match (against_source, ir_opt) {
        (Some(src), Some(ir)) => run_against_source(ir, src),
        (Some(_), None) => AgainstSourceOutcome::NotChecked(
            "the artifact failed to decode; there is nothing to re-derive against".to_string(),
        ),
        (None, _) => AgainstSourceOutcome::NotRun,
    };
    let verdict = Verdict::from(internal_pass, &against);

    let skipped = parsed
        .as_ref()
        .map_or(0, |decoded| decoded.skipped_non_critical_fields);
    render_verify(format, &checks, internal_pass, &against, &verdict, skipped);
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
    out.push(format!("verify.result={}", verdict.machine_label()));
    out
}

fn render_verify(
    format: Format,
    checks: &[(&'static str, CheckOutcome)],
    internal_pass: bool,
    against: &AgainstSourceOutcome,
    verdict: &Verdict,
    skipped_non_critical_fields: u64,
) {
    match format {
        Format::Machine => {
            for line in verify_machine_lines(
                checks,
                internal_pass,
                against,
                verdict,
                skipped_non_critical_fields,
            ) {
                println!("{line}");
            }
        }
        Format::Human => {
            println!("verify — the honesty checks any reader can run with no source (always run):");
            for (name, outcome) in checks {
                let mark = match outcome {
                    CheckOutcome::Pass => "ok",
                    CheckOutcome::Fail(_) => "FAIL",
                    CheckOutcome::NotChecked(_) => "not-checked",
                };
                println!("  [{mark}] {name}");
                if let Some(d) = outcome.detail() {
                    println!("      {}", display::human(d));
                }
            }
            println!(
                "  authorship: Unverifiable by construction (no IR field can express target \
                 verification)"
            );
            println!(
                "  internal: {}",
                if internal_pass { "pass" } else { "fail" }
            );
            if let AgainstSourceOutcome::NotChecked(reason) = against {
                println!(
                    "  against-source: could not be checked ({})",
                    display::human(reason)
                );
            } else if !matches!(against, AgainstSourceOutcome::NotRun) {
                println!("  against-source: {}", against.human_label());
                if let Some(d) = against.detail().or_else(|| against.note()) {
                    println!("      {}", display::human(d));
                }
            }
            println!("{}", verdict.human_line());
        }
    }
}

#[cfg(test)]
mod tests;

//! The command implementations for the three-verb surface (handoff `cli-and-verify-handoff-v2.md`). Each
//! returns a CL-08 exit code and prints its own output; `main` maps the code to `std::process::exit`.
//! Every string that can originate in a source repository is routed through [`crate::display`] before it
//! reaches stdout or stderr (CR-19).

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
use brygge_ir::model::{AtomId, PathOp, RefKind};
use brygge_ir::status::{Derivation, DerivationKind, EpistemicStatus};
use brygge_ir::{Ir, LossClass};

use crate::cli::{Format, SourceKind};
use crate::display;
use crate::exit;

const VERIFY_VERSION: u32 = 2;
const INSPECT_VERSION: u32 = 2;

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

/// The fixed, lowercase label for a loss class (never `{:?}`).
fn loss_class_label(class: LossClass) -> &'static str {
    match class {
        LossClass::Representation => "representation",
        LossClass::AdvisoryUnreliable => "advisory-unreliable",
        LossClass::Other => "other",
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
        | PathOp::Delete { status, .. } => status,
    }
}

fn read_ir(path: &Path) -> Result<Ir, String> {
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

/// The per-source knobs a decode needs: git/hg use `infer_renames`; svn uses `reconstruct_refs`/`layout`;
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
                .map_err(|e| match e {
                    CvsError::FloorRefusal { feature, reason } => (
                        exit::FLOOR_REFUSAL,
                        format!("refused CVS feature '{feature}' below the floor: {reason}"),
                    ),
                    other => (exit::FAILURE, format!("decode failed: {other}")),
                })
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
                .map_err(|e| match e {
                    SvnError::FloorRefusal { feature, reason } => (
                        exit::FLOOR_REFUSAL,
                        format!("refused Subversion feature '{feature}' below the floor: {reason}"),
                    ),
                    SvnError::UnsupportedFormat { what, reason } => (
                        exit::FLOOR_REFUSAL,
                        format!("unsupported Subversion dump form '{what}': {reason}"),
                    ),
                    other => (exit::FAILURE, format!("decode failed: {other}")),
                })
        }
        SourceKind::Git => {
            let git = brygge_decode_git::Options {
                infer_renames: opts.infer_renames,
                rename_threshold: 100,
            };
            guard_decoder(|| brygge_decode_git::decode(repo, &git))
                .map_err(|fault| (exit::FAILURE, fault))?
                .map_err(|e| match e {
                    GitError::FloorRefusal { feature, reason } => (
                        exit::FLOOR_REFUSAL,
                        format!("refused Git feature '{feature}' below the floor: {reason}"),
                    ),
                    other => (exit::FAILURE, format!("decode failed: {other}")),
                })
        }
        SourceKind::Hg => {
            let hg = brygge_decode_hg::Options {
                infer_renames: opts.infer_renames,
                rename_threshold: 100,
            };
            guard_decoder(|| brygge_decode_hg::decode(repo, &hg))
                .map_err(|fault| (exit::FAILURE, fault))?
                .map_err(|e| match e {
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
                    other => (exit::FAILURE, format!("decode failed: {other}")),
                })
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
             source recorded. Authorship is Unverifiable."
        }
        SourceKind::Svn => {
            "Subversion: revisions are carried as recorded. Branches and tags are only a directory \
             convention; with --reconstruct-refs they are reconstructed and marked derived. Authorship is \
             Unverifiable."
        }
        SourceKind::Cvs => {
            "CVS has no atomic commits: every changeset is brygge's reconstruction (derived), and a \
             changeset cannot be checked against the source. File contents and per-file history are \
             carried as recorded. Authorship is Unverifiable."
        }
    }
}

/// Write `bytes` atomically to `out` (CR-13): a temp file in the same directory, flushed and
/// `sync_all`'d, then renamed over `out`. On any failure the temp file is removed and `out` is untouched.
fn atomic_write(out: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "artifact".to_string());
    let tmp = dir.join(format!(".{file_name}.brygge-tmp-{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, out)?;
        // The rename is atomic but not yet durable across a power loss until the directory entry
        // itself is synced (review 003 A-2). Best-effort: some platforms cannot open a directory as a
        // file at all, and the rename has already succeeded either way, so a failure here is not the
        // write's failure.
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
    // Exit class (CL-08): a convention violation (svn ref reconstruction found no layout, FA-2) takes
    // precedence; else recorded loss if any non-representation drop exists; else clean.
    if ir.loss.dropped.iter().any(|d| {
        d.what == brygge_decode_svn::LAYOUT_UNMATCHED || d.what == brygge_decode_cvs::UNDER_FLOOR
    }) {
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
    let ir = match read_ir(artifact) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("{}", display::human(&e));
            return exit::FAILURE;
        }
    };
    let report = brygge_ir::honesty::summary(&ir);
    match format {
        Format::Human => {
            print!("{}", report.render_human());
            if atoms {
                print!("{}", render_atoms_human(&ir));
            }
        }
        Format::Machine => {
            print!("{}", report.render_machine());
            if atoms {
                print!("{}", render_atoms_machine(&ir));
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
        ir.contract_version,
        ir.atoms.len(),
        ir.refs.len(),
        ir.content.len()
    );
    let _ = writeln!(s, "atoms (topological order):");
    for atom in &ir.atoms {
        let subject = atom
            .metadata
            .message
            .as_deref()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("");
        let _ = writeln!(
            s,
            "  {}  [{}]  src:{}  {} op(s)  {}",
            short_hex(&atom.id.0),
            status_label_human(&atom.status),
            short_hex(&atom.source.atom_id),
            atom.ops.len(),
            display::human(subject),
        );
        for hint in &atom.rename_hints {
            let _ = writeln!(
                s,
                "      rename {} -> {} [{}]",
                display::human(&hint.from),
                display::human(&hint.to),
                status_label_human(&hint.status)
            );
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
    s
}

fn render_atoms_machine(ir: &Ir) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "inspect_version={INSPECT_VERSION}");
    let _ = writeln!(s, "contract_version={}", ir.contract_version);
    let _ = writeln!(s, "atoms={}", ir.atoms.len());
    for (i, atom) in ir.atoms.iter().enumerate() {
        let _ = writeln!(s, "atom.{i}.id={}", hex(&atom.id.0));
        let _ = writeln!(s, "atom.{i}.status={}", status_label_machine(&atom.status));
        let _ = writeln!(s, "atom.{i}.source_id={}", hex(&atom.source.atom_id));
        let _ = writeln!(s, "atom.{i}.ops={}", atom.ops.len());
        let _ = writeln!(s, "atom.{i}.parents={}", atom.parents.len());
        let _ = writeln!(
            s,
            "atom.{i}.message={}",
            display::machine_value(atom.metadata.message.as_deref().unwrap_or(""))
        );
        for (j, hint) in atom.rename_hints.iter().enumerate() {
            let _ = writeln!(
                s,
                "atom.{i}.rename.{j}.from={}",
                display::machine_value(&hint.from)
            );
            let _ = writeln!(
                s,
                "atom.{i}.rename.{j}.to={}",
                display::machine_value(&hint.to)
            );
            let _ = writeln!(
                s,
                "atom.{i}.rename.{j}.status={}",
                status_label_machine(&hint.status)
            );
        }
    }
    for (i, rf) in ir.refs.iter().enumerate() {
        let _ = writeln!(s, "ref.{i}.name={}", display::machine_value(&rf.name));
        let _ = writeln!(s, "ref.{i}.kind={}", ref_kind_label(&rf.kind));
        let _ = writeln!(s, "ref.{i}.target={}", hex(&rf.target.0));
        let _ = writeln!(s, "ref.{i}.status={}", status_label_machine(&rf.status));
    }
    for (i, drop) in ir.loss.dropped.iter().enumerate() {
        let _ = writeln!(s, "loss.{i}.class={}", loss_class_label(drop.class));
        let _ = writeln!(s, "loss.{i}.what={}", display::machine_value(&drop.what));
    }
    s
}

// ---- verify (CL-04, CR-02) -------------------------------------------------------------------------

/// The outcome of one internal check: it can always fail, and `n/a` is reserved for a check that
/// genuinely cannot apply (and says why) — never a way to hide a failure.
enum CheckOutcome {
    Pass,
    Fail(String),
    NotApplicable(String),
}

impl CheckOutcome {
    fn label(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail(_) => "fail",
            Self::NotApplicable(_) => "n/a",
        }
    }
    fn detail(&self) -> Option<&str> {
        match self {
            Self::Pass => None,
            Self::Fail(d) | Self::NotApplicable(d) => Some(d),
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
/// tree). `Add` needs the path absent; `Modify`/`Delete` need it present. Every rename hint's `to` exists
/// in the resulting tree. Memory- and time-bounded (review 003 R-3): a parent's first-parent refcount is
/// decremented *before* this atom borrows its tree, so the atom holding the **last** reference `take`s
/// the tree out of the map and mutates it in place — no clone — while an earlier sibling at a branch
/// point clones, since the tree must still exist for the reference(s) after it. A linear history (every
/// refcount is 1) therefore clones nothing at all.
fn replay_ir(ir: &Ir) -> (CheckOutcome, ReplayStats) {
    let mut refcount: HashMap<AtomId, usize> = HashMap::new();
    for atom in &ir.atoms {
        if let Some(p0) = atom.parents.first() {
            *refcount.entry(*p0).or_insert(0) += 1;
        }
    }
    let mut trees: HashMap<AtomId, BTreeSet<String>> = HashMap::new();
    let mut stats = ReplayStats::default();
    for atom in &ir.atoms {
        let mut tree = match atom.parents.first() {
            Some(p0) => {
                let remaining = refcount.get_mut(p0).map_or(0, |c| {
                    *c -= 1;
                    *c
                });
                if remaining == 0 {
                    // The last first-parent reference to this tree: take it, never clone.
                    match trees.remove(p0) {
                        Some(t) => t,
                        None => {
                            return (
                                CheckOutcome::Fail(
                                    "an atom's first-parent tree is unavailable (a structural \
                                     inconsistency)"
                                        .to_string(),
                                ),
                                stats,
                            );
                        }
                    }
                } else {
                    // A branch point: another reference still needs the original, so this one clones.
                    match trees.get(p0) {
                        Some(t) => {
                            stats.clones += 1;
                            t.clone()
                        }
                        None => {
                            return (
                                CheckOutcome::Fail(
                                    "an atom's first-parent tree is unavailable (a structural \
                                     inconsistency)"
                                        .to_string(),
                                ),
                                stats,
                            );
                        }
                    }
                }
            }
            None => BTreeSet::new(),
        };
        for op in &atom.ops {
            match op {
                PathOp::Add { path, .. } => {
                    if !tree.insert(path.clone()) {
                        return (
                            CheckOutcome::Fail(
                                "an Add op names an already-present path".to_string(),
                            ),
                            stats,
                        );
                    }
                }
                PathOp::Modify { path, .. } => {
                    if !tree.contains(path) {
                        return (
                            CheckOutcome::Fail("a Modify op names an absent path".to_string()),
                            stats,
                        );
                    }
                }
                PathOp::Delete { path, .. } => {
                    if !tree.remove(path) {
                        return (
                            CheckOutcome::Fail("a Delete op names an absent path".to_string()),
                            stats,
                        );
                    }
                }
            }
        }
        for hint in &atom.rename_hints {
            if !tree.contains(&hint.to) {
                return (
                    CheckOutcome::Fail(
                        "a rename hint's 'to' path is not present in the resulting tree"
                            .to_string(),
                    ),
                    stats,
                );
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
/// present, and the parameters its kind requires (RFC 011 D-10's registry, already met by every shipped
/// decoder).
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
                for key in ["window_secs", "cluster_keys"] {
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
        for hint in &atom.rename_hints {
            if let Err(e) = check_status(&hint.status) {
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
            return CheckOutcome::NotApplicable(
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
                let ok = matches!(
                    &atom.status,
                    EpistemicStatus::Derived(d) if d.kind == DerivationKind::ReconstructedChangeset
                );
                if !ok {
                    return CheckOutcome::Fail(
                        "a CVS atom is not Derived(ReconstructedChangeset)".to_string(),
                    );
                }
                if !atom.rename_hints.is_empty() {
                    return CheckOutcome::Fail(
                        "a CVS atom carries a rename hint; CVS records no renames".to_string(),
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
                for hint in &atom.rename_hints {
                    if !hint.status.is_derived() {
                        return CheckOutcome::Fail(
                            "a Git rename hint is Stated; Git never states a rename".to_string(),
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
    Corresponds,
    DoesNotCorrespond(String),
    Reproduces,
    DoesNotReproduce(String),
    NotChecked(String),
    NotRun,
}

impl AgainstSourceOutcome {
    fn machine_label(&self) -> &'static str {
        match self {
            Self::Corresponds => "corresponds",
            Self::DoesNotCorrespond(_) => "does-not-correspond",
            Self::Reproduces => "reproduces",
            Self::DoesNotReproduce(_) => "does-not-reproduce",
            Self::NotChecked(_) => "not-checked",
            Self::NotRun => "not-run",
        }
    }
    fn human_label(&self) -> &'static str {
        match self {
            Self::Corresponds => "corresponds",
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
    let opts = SourceOpts {
        infer_renames: infer_renames_from_provenance(ir1),
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
    // Compare identity-bearing content (import time is provenance-only — ID-4).
    let mut a = ir1.clone();
    let mut b = ir2;
    a.provenance.import_time = None;
    b.provenance.import_time = None;
    let corresponds = a == b;
    match (is_cvs, corresponds) {
        (true, true) => AgainstSourceOutcome::Reproduces,
        (true, false) => AgainstSourceOutcome::DoesNotReproduce(divergence(&a, &b)),
        (false, true) => AgainstSourceOutcome::Corresponds,
        (false, false) => AgainstSourceOutcome::DoesNotCorrespond(divergence(&a, &b)),
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

/// `verify <artifact> [--against-source <source>]` (CL-04, CR-02): the internal checks always run; each
/// can fail. With `--against-source`, additionally re-derive and report the two results separately
/// (VF-4). Exit `50` if anything fails.
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
        Ok(ir) => {
            checks.push(("integrity", CheckOutcome::Pass));
            Some(ir)
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
                CheckOutcome::NotApplicable(
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
    // A genuine check failure (internal, or a real against-source mismatch) is exit 50. Otherwise, a
    // requested against-source that could not be checked at all is a runtime failure, exit 1 — distinct
    // from both "clean" and "failed a check" (review 003 R-1, CL-08). `verify.result`/the human verdict
    // line report whether anything that *did* run failed, which "not checked" did not.
    let overall_pass = internal_pass && !against.is_mismatch();

    render_verify(format, &checks, internal_pass, &against, overall_pass);

    if !internal_pass || against.is_mismatch() {
        exit::VERIFY_FAILED
    } else if against.is_not_checked() {
        exit::FAILURE
    } else {
        exit::CLEAN
    }
}

fn render_verify(
    format: Format,
    checks: &[(&'static str, CheckOutcome)],
    internal_pass: bool,
    against: &AgainstSourceOutcome,
    overall_pass: bool,
) {
    match format {
        Format::Machine => {
            println!("verify_version={VERIFY_VERSION}");
            for (name, outcome) in checks {
                println!("verify.check.{name}={}", outcome.label());
            }
            println!("verify.authorship=unverifiable");
            println!(
                "verify.internal={}",
                if internal_pass { "pass" } else { "fail" }
            );
            println!("verify.against_source={}", against.machine_label());
            let mut i = 0usize;
            for (_, outcome) in checks {
                if let Some(d) = outcome.detail() {
                    println!("verify.detail.{i}={}", display::machine_value(d));
                    i += 1;
                }
            }
            if let Some(d) = against.detail() {
                println!("verify.detail.{i}={}", display::machine_value(d));
            }
            println!(
                "verify.result={}",
                if overall_pass { "pass" } else { "fail" }
            );
        }
        Format::Human => {
            println!("verify — the honesty checks any reader can run with no source (always run):");
            for (name, outcome) in checks {
                let mark = match outcome {
                    CheckOutcome::Pass => "ok",
                    CheckOutcome::Fail(_) => "FAIL",
                    CheckOutcome::NotApplicable(_) => "n/a",
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
                if let Some(d) = against.detail() {
                    println!("      {}", display::human(d));
                }
            }
            println!("  => {}", if overall_pass { "PASS" } else { "FAIL" });
        }
    }
}

#[cfg(test)]
mod tests;

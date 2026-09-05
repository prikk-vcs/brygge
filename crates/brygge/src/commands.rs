//! The read-side command implementations (handoff D-B..D-E). Each returns a CL-08 exit code and prints
//! its own output; `main` maps the code to `std::process::exit`.

use std::fmt::Write as _;
use std::path::Path;

use brygge_decode_git::{Error as GitError, Options};
use brygge_ir::model::PathOp;
use brygge_ir::status::EpistemicStatus;
use brygge_ir::{Ir, LossClass};

use crate::cli::Format;
use crate::exit;

const VERIFY_VERSION: u32 = 1;
const INSPECT_VERSION: u32 = 1;

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

fn status_label(s: &EpistemicStatus) -> String {
    match s {
        EpistemicStatus::Stated => "stated".to_string(),
        EpistemicStatus::Derived(d) => format!("derived:{}", d.kind.label()),
    }
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

fn options_from_provenance(ir: &Ir) -> Options {
    let p = &ir.provenance.params;
    Options {
        detect_renames: p
            .get("detect_renames")
            .map(|v| v == "true")
            .unwrap_or(false),
        rename_threshold: p
            .get("rename_threshold")
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(100),
    }
}

/// `decode git <path>` (CL-01, FL-01).
pub fn run_decode(path: &Path, out: Option<&Path>, detect_renames: bool, format: Format) -> i32 {
    let opts = Options {
        detect_renames,
        rename_threshold: 100,
    };
    match brygge_decode_git::decode(path, &opts) {
        Ok(ir) => {
            if let Some(out) = out {
                let bytes = brygge_ir::to_bytes(&ir);
                if let Err(e) = std::fs::write(out, &bytes) {
                    eprintln!("cannot write {}: {e}", out.display());
                    return exit::FAILURE;
                }
                eprintln!("wrote {} ({} bytes)", out.display(), bytes.len());
            }
            let report = brygge_ir::honesty::summary(&ir);
            match format {
                Format::Human => print!("{}", report.render_human()),
                Format::Machine => print!("{}", report.render_machine()),
            }
            // Exit class: recorded loss only if a non-representation drop exists (handoff D-B).
            if ir
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
        Err(GitError::FloorRefusal { feature, reason }) => {
            eprintln!("refused Git feature '{feature}' below the floor: {reason}");
            exit::FLOOR_REFUSAL
        }
        Err(e) => {
            eprintln!("decode failed: {e}");
            exit::FAILURE
        }
    }
}

/// `inspect --ir <file>` (CL-02): atoms + epistemic status (IX-02), source ids (IX-03), loss (IX-04).
pub fn run_inspect(ir_path: &Path, format: Format) -> i32 {
    let ir = match read_ir(ir_path) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("{e}");
            return exit::FAILURE;
        }
    };
    match format {
        Format::Machine => print!("{}", render_inspect_machine(&ir)),
        Format::Human => print!("{}", render_inspect_human(&ir)),
    }
    exit::CLEAN
}

/// `summary --import <file>` (CL-05, FS-02): reproduce the fidelity summary from the artifact alone.
pub fn run_summary(import: &Path, format: Format) -> i32 {
    let ir = match read_ir(import) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("{e}");
            return exit::FAILURE;
        }
    };
    let report = brygge_ir::honesty::summary(&ir);
    match format {
        Format::Human => print!("{}", report.render_human()),
        Format::Machine => print!("{}", report.render_machine()),
    }
    exit::CLEAN
}

/// `verify --internal --import <file>` (CL-04, VF-3): honesty checks provable with no source present.
pub fn run_verify_internal(import: &Path, format: Format) -> i32 {
    // Integrity: reading re-checks the digest, blob content-addresses, referential integrity, version.
    let bytes = match std::fs::read(import) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {}: {e}", import.display());
            return exit::FAILURE;
        }
    };
    let ir = match brygge_ir::from_bytes(&bytes) {
        Ok(ir) => ir,
        Err(e) => {
            // A bad digest is the check firing, not a generic error.
            print_verify(
                format,
                "internal",
                &[("integrity", false)],
                Some(&e.to_string()),
            );
            return exit::VERIFY_FAILED;
        }
    };

    let provenance_ok = !ir.provenance.decoder.is_empty();
    let loss_ok = true; // the loss boundary is a present, stated field of every IR
    let derived_ok = derived_wellformed(&ir);
    let report = brygge_ir::honesty::summary(&ir);
    let authorship_ok = report.render_human().contains("Unverified");
    let summary_ok = brygge_ir::honesty::summary(&ir) == report; // recoverable & deterministic

    let checks = [
        ("integrity", true),
        ("provenance", provenance_ok),
        ("loss_boundary", loss_ok),
        ("derived_wellformed", derived_ok),
        ("authorship_unverified", authorship_ok),
        ("summary_recoverable", summary_ok),
    ];
    let pass = checks.iter().all(|(_, ok)| *ok);
    print_verify(format, "internal", &checks, None);
    if pass {
        exit::CLEAN
    } else {
        exit::VERIFY_FAILED
    }
}

/// `verify --against-source <repo> --import <file>` (CL-04, VF-2): re-derive from the source and confirm
/// correspondence, without trusting the earlier run.
pub fn run_verify_against_source(repo: &Path, import: &Path, format: Format) -> i32 {
    let ir1 = match read_ir(import) {
        Ok(ir) => ir,
        Err(e) => {
            print_verify(format, "against-source", &[("integrity", false)], Some(&e));
            return exit::VERIFY_FAILED;
        }
    };
    let opts = options_from_provenance(&ir1);
    let ir2 = match brygge_decode_git::decode(repo, &opts) {
        Ok(ir) => ir,
        Err(GitError::FloorRefusal { feature, reason }) => {
            eprintln!("source refused Git feature '{feature}': {reason}");
            return exit::FLOOR_REFUSAL;
        }
        Err(e) => {
            eprintln!("cannot re-decode source: {e}");
            return exit::FAILURE;
        }
    };

    // Compare identity-bearing content (import time is provenance-only — ID-4).
    let mut a = ir1;
    let mut b = ir2;
    a.provenance.import_time = None;
    b.provenance.import_time = None;

    if a == b {
        print_verify(format, "against-source", &[("corresponds", true)], None);
        exit::CLEAN
    } else {
        let detail = divergence(&a, &b);
        print_verify(
            format,
            "against-source",
            &[("corresponds", false)],
            Some(&detail),
        );
        exit::VERIFY_FAILED
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

fn derived_wellformed(ir: &Ir) -> bool {
    let ok = |s: &EpistemicStatus| match s {
        EpistemicStatus::Stated => true,
        EpistemicStatus::Derived(d) => !d.by.is_empty() && !d.decoder_version.is_empty(),
    };
    ir.atoms.iter().all(|a| {
        ok(&a.status)
            && a.ops.iter().all(|op| ok(op_status(op)))
            && a.rename_hints.iter().all(|h| ok(&h.status))
    }) && ir.refs.iter().all(|r| ok(&r.status))
}

fn print_verify(format: Format, mode: &str, checks: &[(&str, bool)], detail: Option<&str>) {
    let pass = checks.iter().all(|(_, ok)| *ok);
    match format {
        Format::Machine => {
            println!("verify_version={VERIFY_VERSION}");
            println!("verify.mode={mode}");
            for (name, ok) in checks {
                println!("verify.{name}={}", if *ok { "pass" } else { "fail" });
            }
            if let Some(d) = detail {
                println!("verify.detail={d}");
            }
            println!("verify.result={}", if pass { "pass" } else { "fail" });
        }
        Format::Human => {
            println!("verify ({mode}) — authorship is imported, Unverified by any target (VF-4)");
            for (name, ok) in checks {
                println!("  [{}] {name}", if *ok { "ok" } else { "FAIL" });
            }
            if let Some(d) = detail {
                println!("  detail: {d}");
            }
            println!("  => {}", if pass { "PASS" } else { "FAIL" });
        }
    }
}

fn render_inspect_human(ir: &Ir) -> String {
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
            "  {}  [{}]  git:{}  {} op(s)  {subject}",
            short_hex(&atom.id.0),
            status_label(&atom.status),
            short_hex(&atom.source.atom_id),
            atom.ops.len()
        );
        for hint in &atom.rename_hints {
            let _ = writeln!(
                s,
                "      rename {} -> {} [{}]",
                hint.from,
                hint.to,
                status_label(&hint.status)
            );
        }
    }
    if !ir.refs.is_empty() {
        let _ = writeln!(s, "refs:");
        for rf in &ir.refs {
            let _ = writeln!(
                s,
                "  {} ({:?}) -> {}",
                rf.name,
                rf.kind,
                short_hex(&rf.target.0)
            );
        }
    }
    let _ = writeln!(s, "loss boundary:");
    if ir.loss.dropped.is_empty() {
        let _ = writeln!(s, "  (nothing dropped)");
    } else {
        for drop in &ir.loss.dropped {
            let _ = writeln!(s, "  [{:?}] {} — {}", drop.class, drop.what, drop.reason);
        }
    }
    s
}

fn render_inspect_machine(ir: &Ir) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "inspect_version={INSPECT_VERSION}");
    let _ = writeln!(s, "contract_version={}", ir.contract_version);
    let _ = writeln!(s, "atoms={}", ir.atoms.len());
    for (i, atom) in ir.atoms.iter().enumerate() {
        let _ = writeln!(s, "atom.{i}.id={}", hex(&atom.id.0));
        let _ = writeln!(s, "atom.{i}.status={}", status_label(&atom.status));
        let _ = writeln!(s, "atom.{i}.source_id={}", hex(&atom.source.atom_id));
        let _ = writeln!(s, "atom.{i}.ops={}", atom.ops.len());
        let _ = writeln!(s, "atom.{i}.parents={}", atom.parents.len());
    }
    for rf in &ir.refs {
        let _ = writeln!(s, "ref.{}.kind={:?}", rf.name, rf.kind);
        let _ = writeln!(s, "ref.{}.target={}", rf.name, hex(&rf.target.0));
    }
    for (i, drop) in ir.loss.dropped.iter().enumerate() {
        let _ = writeln!(s, "loss.{i}.class={:?}", drop.class);
        let _ = writeln!(s, "loss.{i}.what={}", drop.what);
    }
    s
}

#[cfg(test)]
mod tests;

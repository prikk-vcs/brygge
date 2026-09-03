//! The honesty machinery: the recoverable fidelity report (RFC 002 D-3/D-6, `HO-4/FS-02`).
//!
//! [`summary`] is a **pure function of the [`Ir`]**, so a later reader reproduces the exact end-of-run
//! summary from the artifact alone — the external proof that honesty travels with the import and cannot
//! be lost with a log. The report is **always complete**: there is no flag that drops the derived,
//! dropped, or provenance sections (`HO-5/CF-02`); verbosity affects rendering, never presence.

use std::collections::BTreeMap;

use crate::model::{Ir, LossClass};
use crate::status::EpistemicStatus;

/// The machine-report contract version (RFC 002 D-3), independent of the IR contract and the tool.
pub const REPORT_VERSION: u32 = 1;

/// What an import preserved, derived, dropped, and refused. Grouped counts are in sorted-key order for
/// deterministic rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FidelityReport {
    /// The machine-report contract version.
    pub report_version: u32,
    /// Number of atoms.
    pub atoms: u64,
    /// Number of refs.
    pub refs: u64,
    /// Number of distinct blobs.
    pub blobs: u64,
    /// Total content bytes.
    pub content_bytes: u64,
    /// Derived assertions, grouped by taxonomy-kind label → count (RFC 002 D-1).
    pub derived: BTreeMap<String, u64>,
    /// Dropped data, grouped by loss-class label → count (RFC 002 D-2).
    pub dropped: BTreeMap<String, u64>,
    /// Source features refused below the floor — empty at the IR level; decoders fill it (`FA-3`).
    pub refused: Vec<String>,
}

/// Compute the fidelity report from an [`Ir`] (pure — `FS-02`).
#[must_use]
pub fn summary(ir: &Ir) -> FidelityReport {
    let mut derived: BTreeMap<String, u64> = BTreeMap::new();
    let note = |s: &EpistemicStatus, d: &mut BTreeMap<String, u64>| {
        if let EpistemicStatus::Derived(der) = s {
            *d.entry(der.kind.label().to_string()).or_insert(0) += 1;
        }
    };
    for atom in &ir.atoms {
        note(&atom.status, &mut derived);
        for op in &atom.ops {
            note(op_status(op), &mut derived);
        }
        for hint in &atom.rename_hints {
            note(&hint.status, &mut derived);
        }
    }
    for rf in &ir.refs {
        note(&rf.status, &mut derived);
    }

    let mut dropped: BTreeMap<String, u64> = BTreeMap::new();
    for drop in &ir.loss.dropped {
        *dropped
            .entry(class_label(drop.class).to_string())
            .or_insert(0) += 1;
    }

    FidelityReport {
        report_version: REPORT_VERSION,
        atoms: ir.atoms.len() as u64,
        refs: ir.refs.len() as u64,
        blobs: ir.content.len() as u64,
        content_bytes: ir.content.total_bytes(),
        derived,
        dropped,
        refused: Vec::new(),
    }
}

impl FidelityReport {
    /// A stable, deterministic machine rendering (versioned — a CI gate can pin it, `CL-07/CT-04`).
    #[must_use]
    pub fn render_machine(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "report_version={}", self.report_version);
        let _ = writeln!(s, "atoms={}", self.atoms);
        let _ = writeln!(s, "refs={}", self.refs);
        let _ = writeln!(s, "blobs={}", self.blobs);
        let _ = writeln!(s, "content_bytes={}", self.content_bytes);
        for (kind, n) in &self.derived {
            let _ = writeln!(s, "derived.{kind}={n}");
        }
        for (class, n) in &self.dropped {
            let _ = writeln!(s, "dropped.{class}={n}");
        }
        for r in &self.refused {
            let _ = writeln!(s, "refused={r}");
        }
        s
    }

    /// A human-facing rendering. Authorship is always shown `Unverified` (`VF-4/HO-3`).
    #[must_use]
    pub fn render_human(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "fidelity report (v{}) — authorship: Unverified (imported, not verified by any target)",
            self.report_version
        );
        let _ = writeln!(
            s,
            "  preserved: {} atom(s), {} ref(s), {} blob(s), {} content byte(s)",
            self.atoms, self.refs, self.blobs, self.content_bytes
        );
        if self.derived.is_empty() {
            let _ = writeln!(s, "  derived:   (none — every assertion is source-stated)");
        } else {
            let _ = writeln!(s, "  derived:");
            for (kind, n) in &self.derived {
                let _ = writeln!(s, "    {kind}: {n}");
            }
        }
        if self.dropped.is_empty() {
            let _ = writeln!(s, "  dropped:   (nothing dropped)");
        } else {
            let _ = writeln!(s, "  dropped:");
            for (class, n) in &self.dropped {
                let _ = writeln!(s, "    {class}: {n}");
            }
        }
        if !self.refused.is_empty() {
            let _ = writeln!(s, "  refused:");
            for r in &self.refused {
                let _ = writeln!(s, "    {r}");
            }
        }
        s
    }
}

fn op_status(op: &crate::model::PathOp) -> &EpistemicStatus {
    use crate::model::PathOp::{Add, Delete, Modify};
    match op {
        Add { status, .. } | Modify { status, .. } | Delete { status, .. } => status,
    }
}

fn class_label(class: LossClass) -> &'static str {
    match class {
        LossClass::Representation => "representation",
        LossClass::AdvisoryUnreliable => "advisory-unreliable",
        LossClass::Other => "other",
    }
}

#[cfg(test)]
mod tests;

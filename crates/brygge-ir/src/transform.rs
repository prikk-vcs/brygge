//! `brygge transform`'s own honesty rule, as a function (RFC 018 D-6): [`check`] accepts an `output`
//! artifact as an honest transform of an `input` one only if every change is marked and every removal
//! is recorded, and refuses it with a [`Violation`] naming the first thing wrong otherwise. [`Edit`] is
//! the sanctioned way to build an `output` that passes it: it computes every mark from the difference
//! with `input` itself, never from what the caller says, so a transform built with it passes [`check`]
//! by construction.
//!
//! Usable by anyone holding both artifacts — not only the CLI (`prikk`, for one, wants the same rule).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::artifact::Decoded;
use crate::content::BlobId;
use crate::model::{
    AtomId, ChangeAtom, ChangedPart, CopyRecord, LossClass, PathOp, RefKind, RefRecord,
    SourceIdentity,
};
use crate::status::EpistemicStatus;

/// What stopped [`check`] from accepting `output` as an honest transform of `input`: the first rule it
/// found broken, in words a user can act on (e.g. "atom 2a994c92ff58 (source Git:39728fbd945b): author
/// changed but not marked").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation(String);

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Violation {}

impl Violation {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// What one transform step changed (RFC 018 D-6) — the counts `brygge transform`'s own last line
/// prints, and `inspect`'s per-step line already shows (`honesty::summary`'s own count, recomputed
/// independently here, over the two real artifacts rather than one decoded one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StepSummary {
    /// The step's own index (`input.ir.transforms.len()`).
    pub step: u64,
    /// How many atoms this step changed (marked `Transformed`, or newly absent with an `atom` drop).
    pub atoms_changed: u64,
    /// How many ops this step changed (marked, moved, or newly absent with an `op` drop).
    pub ops_changed: u64,
    /// How many refs this step changed (marked, renamed, or newly absent with a `ref` drop).
    pub refs_changed: u64,
    /// How many new drops this step made.
    pub dropped: u64,
}

fn changed_set(changed: &[ChangedPart]) -> BTreeSet<ChangedPart> {
    changed.iter().copied().collect()
}

fn first_duplicate_source_identity(atoms: &[ChangeAtom]) -> Option<&SourceIdentity> {
    let mut seen: HashSet<&SourceIdentity> = HashSet::new();
    for atom in atoms {
        if !seen.insert(&atom.source) {
            return Some(&atom.source);
        }
    }
    None
}

/// The input `AtomId` an `atom`/`op`/`copy`-class `Transform` drop names, parsed back from its own
/// grammar (`ir-artifact-format.md`, RFC 018 D-6 R-b) — already validated at decode time, so a
/// malformed one here would be a bug in that validation, not untrusted input reaching this function
/// unvalidated. Returns `None` for a `ref` drop, which names no `AtomId`.
fn drop_subject_atom_id(class_word: &str, what: &str) -> Option<AtomId> {
    let hex = match class_word {
        "atom" => what.strip_prefix("atom ")?,
        "op" | "copy" => what
            .strip_prefix(class_word)?
            .strip_prefix(' ')?
            .split(' ')
            .next()?,
        _ => return None,
    };
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(AtomId(bytes))
}

/// The word naming a `Transform` drop's own kind — `"atom"`, `"op"`, `"copy"`, or `"ref"` — the first
/// space-delimited token of `what`.
fn drop_kind_word(what: &str) -> &str {
    what.split(' ').next().unwrap_or("")
}

/// Resolves `p` (one of `atom`'s own input parents) to the output parent(s) it maps to, through
/// [`Edit`]'s own documented substitution: a surviving parent maps to its own output id; a removed
/// parent is replaced, in place, by *its* own parents, resolved the same way (recursively — bounded by
/// the DAG's own depth, which `check_structure` already proved acyclic before this ever runs).
/// Appends to `out`, preserving order and never duplicating an id already present.
fn resolve_parent(
    p: AtomId,
    id_map: &BTreeMap<AtomId, AtomId>,
    input_by_atom_id: &BTreeMap<AtomId, &ChangeAtom>,
    out: &mut Vec<AtomId>,
) {
    if let Some(&mapped) = id_map.get(&p) {
        if !out.contains(&mapped) {
            out.push(mapped);
        }
    } else if let Some(atom) = input_by_atom_id.get(&p) {
        for &gp in &atom.parents {
            resolve_parent(gp, id_map, input_by_atom_id, out);
        }
    }
}

fn mapped_parents(
    original: &[AtomId],
    id_map: &BTreeMap<AtomId, AtomId>,
    input_by_atom_id: &BTreeMap<AtomId, &ChangeAtom>,
) -> Vec<AtomId> {
    let mut out = Vec::new();
    for &p in original {
        resolve_parent(p, id_map, input_by_atom_id, &mut out);
    }
    out
}

/// An op's own payload, variant-aware: `(variant tag, blob, mode)` — `Delete` carries neither.
fn op_payload(op: &PathOp) -> (u8, Option<BlobId>, Option<u32>) {
    match op {
        PathOp::Add { blob, mode, .. } => (0, Some(*blob), Some(*mode)),
        PathOp::Modify { blob, mode, .. } => (1, Some(*blob), Some(*mode)),
        PathOp::Delete { .. } => (2, None, None),
        PathOp::Replace { blob, mode, .. } => (3, Some(*blob), Some(*mode)),
    }
}

/// Whether `input` has two atoms sharing one [`SourceIdentity`] — the same condition [`check`] itself
/// refuses as its very first rule, exposed separately so a caller (`brygge transform`'s own CLI) can
/// refuse before ever running a transform plugin on it, rather than only after the plugin has already
/// done its work and returned (RFC 018 D-6 §3).
#[must_use]
pub fn duplicate_source_identity(input: &Decoded) -> Option<&SourceIdentity> {
    first_duplicate_source_identity(&input.ir.atoms)
}

/// `brygge transform`'s own honesty rule (RFC 018 D-6): `output` is accepted as an honest transform of
/// `input`, performed by the plugin named `name`, only if every rule below holds; `k =
/// input.ir.transforms.len()` is the new step's own index throughout.
///
/// # Errors
/// The first [`Violation`] found, in the order the rules are checked below (the chain, then
/// provenance/flags/earlier drops, then atoms, then — inside each matched atom — its ops, copies and
/// parents, then refs).
#[allow(clippy::too_many_lines)]
pub fn check(
    input: &Decoded,
    output: &Decoded,
    name: &str,
) -> std::result::Result<StepSummary, Violation> {
    if let Some(dup) = first_duplicate_source_identity(&input.ir.atoms) {
        return Err(Violation::new(format!(
            "the input has two atoms with the same source identity ({dup:?}) — cannot match atoms by \
             it at all"
        )));
    }

    let k = input.ir.transforms.len() as u64;

    // ---- rule 1: the chain ------------------------------------------------------------------------
    if output.ir.transforms.len() != input.ir.transforms.len() + 1 {
        return Err(Violation::new(format!(
            "output has {} transform record(s), expected the input's {} plus exactly one",
            output.ir.transforms.len(),
            input.ir.transforms.len()
        )));
    }
    let same_prefix = output
        .ir
        .transforms
        .get(..input.ir.transforms.len())
        .is_some_and(|prefix| prefix == input.ir.transforms.as_slice());
    if !same_prefix {
        return Err(Violation::new(
            "output's transform chain does not start with the input's own chain, unchanged",
        ));
    }
    let Some(record) = output.ir.transforms.get(input.ir.transforms.len()) else {
        return Err(Violation::new(
            "output's transform chain is missing the new record entirely",
        ));
    };
    if record.name != name {
        return Err(Violation::new(format!(
            "the new transform record's name is {:?}, expected {name:?}",
            record.name
        )));
    }
    if record.consumed != input.digest {
        return Err(Violation::new(
            "the new transform record's consumed digest does not match the input artifact's own digest",
        ));
    }

    // ---- rule 2: unchanged wholesale ---------------------------------------------------------------
    if output.ir.provenance != input.ir.provenance {
        return Err(Violation::new(
            "provenance changed, which no transform may do",
        ));
    }
    if output.ir.flags != input.ir.flags {
        return Err(Violation::new("flags changed, which no transform may do"));
    }
    let n_in_drops = input.ir.loss.dropped.len();
    let same_drop_prefix = output
        .ir
        .loss
        .dropped
        .get(..n_in_drops)
        .is_some_and(|prefix| prefix == input.ir.loss.dropped.as_slice());
    if !same_drop_prefix {
        return Err(Violation::new(
            "an earlier drop was changed or removed, which no transform may do",
        ));
    }
    let Some(new_drops) = output.ir.loss.dropped.get(n_in_drops..) else {
        return Err(Violation::new("internal: drop slice out of range"));
    };
    for d in new_drops {
        if d.class != LossClass::Transform || d.step != Some(k) {
            return Err(Violation::new(format!(
                "a new drop is not a Transform-class drop at step {k}: {d:?}"
            )));
        }
    }

    // ---- rule 3 (part 1): match atoms by source identity -------------------------------------------
    let input_by_source: HashMap<&SourceIdentity, &ChangeAtom> =
        input.ir.atoms.iter().map(|a| (&a.source, a)).collect();
    let input_by_atom_id: BTreeMap<AtomId, &ChangeAtom> =
        input.ir.atoms.iter().map(|a| (a.id, a)).collect();
    let output_by_atom_id: BTreeMap<AtomId, &ChangeAtom> =
        output.ir.atoms.iter().map(|a| (a.id, a)).collect();

    let mut id_map: BTreeMap<AtomId, AtomId> = BTreeMap::new();
    let mut output_by_source: HashMap<&SourceIdentity, AtomId> = HashMap::new();
    for out_atom in &output.ir.atoms {
        if output_by_source
            .insert(&out_atom.source, out_atom.id)
            .is_some()
        {
            return Err(Violation::new(format!(
                "two output atoms share one source identity ({:?})",
                out_atom.source
            )));
        }
        let Some(&in_atom) = input_by_source.get(&out_atom.source) else {
            return Err(Violation::new(format!(
                "an output atom (source {:?}) matches no input atom — invented",
                out_atom.source
            )));
        };
        id_map.insert(in_atom.id, out_atom.id);
    }

    // Every new `atom`-class drop names an input atom with no match; every input atom with no match
    // has exactly one such drop.
    let mut atom_drop_subjects: HashSet<AtomId> = HashSet::new();
    let mut op_drop_subjects: HashSet<(AtomId, String)> = HashSet::new();
    let mut copy_drop_subjects: HashSet<(AtomId, String)> = HashSet::new();
    let mut ref_drop_subjects: HashSet<(String, String)> = HashSet::new();
    for d in new_drops {
        match drop_kind_word(&d.what) {
            "atom" => {
                let Some(id) = drop_subject_atom_id("atom", &d.what) else {
                    return Err(Violation::new(format!("malformed atom drop: {:?}", d.what)));
                };
                if !atom_drop_subjects.insert(id) {
                    return Err(Violation::new(format!(
                        "two `atom` drops name the same input atom {id}"
                    )));
                }
            }
            "op" => {
                let Some(id) = drop_subject_atom_id("op", &d.what) else {
                    return Err(Violation::new(format!("malformed op drop: {:?}", d.what)));
                };
                let path = d.what.splitn(3, ' ').nth(2).unwrap_or("").to_string();
                if !op_drop_subjects.insert((id, path)) {
                    return Err(Violation::new(format!(
                        "two `op` drops name the same op: {:?}",
                        d.what
                    )));
                }
            }
            "copy" => {
                let Some(id) = drop_subject_atom_id("copy", &d.what) else {
                    return Err(Violation::new(format!("malformed copy drop: {:?}", d.what)));
                };
                let to = d.what.splitn(3, ' ').nth(2).unwrap_or("").to_string();
                if !copy_drop_subjects.insert((id, to)) {
                    return Err(Violation::new(format!(
                        "two `copy` drops name the same copy: {:?}",
                        d.what
                    )));
                }
            }
            "ref" => {
                let mut parts = d.what.splitn(3, ' ');
                let _ref_word = parts.next();
                let kind = parts.next().unwrap_or("").to_string();
                let name = parts.next().unwrap_or("").to_string();
                if !ref_drop_subjects.insert((kind, name)) {
                    return Err(Violation::new(format!(
                        "two `ref` drops name the same ref: {:?}",
                        d.what
                    )));
                }
            }
            _ => {
                return Err(Violation::new(format!(
                    "a new drop's `what` is malformed: {:?}",
                    d.what
                )));
            }
        }
    }
    for in_atom in &input.ir.atoms {
        let matched = id_map.contains_key(&in_atom.id);
        let dropped = atom_drop_subjects.contains(&in_atom.id);
        if !matched && !dropped {
            return Err(Violation::new(format!(
                "atom {} (source {:?}) was dropped with no `atom` drop recording it",
                in_atom.id, in_atom.source
            )));
        }
        if matched && dropped {
            return Err(Violation::new(format!(
                "atom {} has an `atom` drop naming it, but it still has a matching output atom",
                in_atom.id
            )));
        }
    }
    for &id in &atom_drop_subjects {
        if !input_by_atom_id.contains_key(&id) {
            return Err(Violation::new(format!(
                "an `atom` drop names {id}, which is not an input atom at all"
            )));
        }
    }

    let mut summary = StepSummary {
        step: k,
        dropped: new_drops.len() as u64,
        ..StepSummary::default()
    };

    // ---- rule 3 (part 2), 4, 5, R-a: each matched atom -----------------------------------------------
    for in_atom in &input.ir.atoms {
        let Some(&out_id) = id_map.get(&in_atom.id) else {
            continue; // removed; already accounted for above.
        };
        let Some(out_atom) = output_by_atom_id.get(&out_id) else {
            return Err(Violation::new(
                "internal: a matched output atom id does not name a real output atom",
            ));
        };

        // Ops, matched by path.
        let in_ops: BTreeMap<&str, &PathOp> = in_atom.ops.iter().map(|o| (o.path(), o)).collect();
        let out_ops: BTreeMap<&str, &PathOp> = out_atom.ops.iter().map(|o| (o.path(), o)).collect();
        let mut ops_differ = false;
        let mut gone_without_drop = 0u64;
        let mut new_ops = 0u64;

        for (&path, in_op) in &in_ops {
            let Some(out_op) = out_ops.get(path) else {
                if !op_drop_subjects.contains(&(in_atom.id, path.to_string())) {
                    gone_without_drop += 1;
                }
                continue;
            };
            let (in_variant, in_blob, in_mode) = op_payload(in_op);
            let (out_variant, out_blob, out_mode) = op_payload(out_op);
            if in_variant != out_variant {
                return Err(Violation::new(format!(
                    "atom {} (source {:?}): op {path:?} changed kind, which no transform may do",
                    in_atom.id, in_atom.source
                )));
            }
            let mut expected = BTreeSet::new();
            if in_blob != out_blob {
                expected.insert(ChangedPart::Blob);
            }
            if in_mode != out_mode {
                expected.insert(ChangedPart::Mode);
            }
            match out_op.status() {
                EpistemicStatus::Transformed {
                    step,
                    changed,
                    prior,
                } => {
                    if *step != k {
                        return Err(Violation::new(format!(
                            "atom {} op {path:?}: marked at step {step}, expected {k}",
                            in_atom.id
                        )));
                    }
                    if prior.as_ref() != in_op.status() {
                        return Err(Violation::new(format!(
                            "atom {} op {path:?}: prior does not equal the input op's own status",
                            in_atom.id
                        )));
                    }
                    if changed_set(changed) != expected {
                        return Err(Violation::new(format!(
                            "atom {} op {path:?}: changed does not equal exactly what differs",
                            in_atom.id
                        )));
                    }
                    ops_differ = true;
                    summary.ops_changed += 1;
                }
                unmarked => {
                    if !expected.is_empty() || unmarked != in_op.status() {
                        return Err(Violation::new(format!(
                            "atom {} (source {:?}): op {path:?} changed but not marked",
                            in_atom.id, in_atom.source
                        )));
                    }
                }
            }
        }
        for (&path, out_op) in &out_ops {
            if in_ops.contains_key(path) {
                continue;
            }
            new_ops += 1;
            match out_op.status() {
                EpistemicStatus::Transformed { step, changed, .. }
                    if *step == k && changed.contains(&ChangedPart::Path) => {}
                _ => {
                    return Err(Violation::new(format!(
                        "atom {} (source {:?}): new op {path:?} is not marked Transformed with path \
                         in changed",
                        in_atom.id, in_atom.source
                    )));
                }
            }
            ops_differ = true;
            summary.ops_changed += 1;
        }
        if new_ops != gone_without_drop {
            return Err(Violation::new(format!(
                "atom {} (source {:?}): {new_ops} moved op(s) but {gone_without_drop} gone op(s) with \
                 no drop — ops may neither be invented nor disappear unrecorded",
                in_atom.id, in_atom.source
            )));
        }

        // Copies, matched by `to` (never individually marked — RFC 018 D-5 forbids `Transformed` on a
        // copy's own status — so a surviving copy must be byte-identical, mapped through `id_map`).
        let in_copies: BTreeMap<&str, &CopyRecord> =
            in_atom.copies.iter().map(|c| (c.to.as_str(), c)).collect();
        let out_copies: BTreeMap<&str, &CopyRecord> =
            out_atom.copies.iter().map(|c| (c.to.as_str(), c)).collect();
        let mut copies_differ = false;
        for (&to, out_copy) in &out_copies {
            let Some(in_copy) = in_copies.get(to) else {
                return Err(Violation::new(format!(
                    "atom {} (source {:?}): an invented copy to {to:?}",
                    in_atom.id, in_atom.source
                )));
            };
            let mapped_from_atom = id_map.get(&in_copy.from_atom).copied();
            if mapped_from_atom != Some(out_copy.from_atom)
                || in_copy.from != out_copy.from
                || in_copy.status != out_copy.status
            {
                return Err(Violation::new(format!(
                    "atom {} (source {:?}): copy to {to:?} changed, which no transform may do \
                     (only removal is supported)",
                    in_atom.id, in_atom.source
                )));
            }
        }
        for &to in in_copies.keys() {
            if !out_copies.contains_key(to) {
                if !copy_drop_subjects.contains(&(in_atom.id, to.to_string())) {
                    return Err(Violation::new(format!(
                        "atom {} (source {:?}): a gone copy to {to:?} has no `copy` drop recording it",
                        in_atom.id, in_atom.source
                    )));
                }
                copies_differ = true;
            }
        }

        // Parents (R-a): the output's own parents must always equal the input's, mapped through the
        // matching (substituting a removed parent with its own) — an unconditional honesty check,
        // regardless of marking, since every surviving ancestor keeps its *identity* even though its
        // id value changes the moment anything about it (or any of *its* ancestors) is edited.
        let expected_parents = mapped_parents(&in_atom.parents, &id_map, &input_by_atom_id);
        if expected_parents != out_atom.parents {
            return Err(Violation::new(format!(
                "atom {} (source {:?}): parents do not equal the input's own, mapped through the \
                 matching",
                in_atom.id, in_atom.source
            )));
        }
        // Whether `Parents` must be in `changed` is a different question: not "do the id *values*
        // differ" (they always do, trivially, once anything upstream changes), but "did the actual
        // *set of ancestor identities* change" — true only when an original parent was removed,
        // forcing substitution by its own parents.
        let parents_differ = in_atom.parents.iter().any(|p| !id_map.contains_key(p));

        let mut actual_changed: BTreeSet<ChangedPart> = BTreeSet::new();
        if in_atom.metadata.author != out_atom.metadata.author {
            actual_changed.insert(ChangedPart::Author);
        }
        if in_atom.metadata.author_time != out_atom.metadata.author_time {
            actual_changed.insert(ChangedPart::AuthorTime);
        }
        if in_atom.metadata.committer != out_atom.metadata.committer {
            actual_changed.insert(ChangedPart::Committer);
        }
        if in_atom.metadata.commit_time != out_atom.metadata.commit_time {
            actual_changed.insert(ChangedPart::CommitTime);
        }
        if in_atom.metadata.message != out_atom.metadata.message {
            actual_changed.insert(ChangedPart::Message);
        }
        if ops_differ {
            actual_changed.insert(ChangedPart::Ops);
        }
        if copies_differ {
            actual_changed.insert(ChangedPart::Copies);
        }
        if parents_differ {
            actual_changed.insert(ChangedPart::Parents);
        }

        match &out_atom.status {
            EpistemicStatus::Transformed {
                step,
                changed,
                prior,
            } => {
                if *step != k {
                    return Err(Violation::new(format!(
                        "atom {} (source {:?}): marked at step {step}, expected {k}",
                        in_atom.id, in_atom.source
                    )));
                }
                if prior.as_ref() != &in_atom.status {
                    return Err(Violation::new(format!(
                        "atom {} (source {:?}): prior does not equal the input atom's own status",
                        in_atom.id, in_atom.source
                    )));
                }
                if changed_set(changed) != actual_changed {
                    return Err(Violation::new(format!(
                        "atom {} (source {:?}): changed does not equal exactly the set of parts that \
                         differ",
                        in_atom.id, in_atom.source
                    )));
                }
                summary.atoms_changed += 1;
            }
            unmarked => {
                if !actual_changed.is_empty() || *unmarked != in_atom.status {
                    return Err(Violation::new(format!(
                        "atom {} (source {:?}): changed but not marked",
                        in_atom.id, in_atom.source
                    )));
                }
            }
        }
    }

    // ---- rule 6: refs, matched by (kind, name) ------------------------------------------------------
    let ref_key = |rf: &RefRecord| (rf.kind.clone(), rf.name.clone());
    let in_refs: BTreeMap<(RefKind, String), &RefRecord> =
        input.ir.refs.iter().map(|r| (ref_key(r), r)).collect();
    let out_refs: BTreeMap<(RefKind, String), &RefRecord> =
        output.ir.refs.iter().map(|r| (ref_key(r), r)).collect();

    // A gone ref is accounted for either by its own `ref` drop, or by being one of the "renamed" ones
    // (counted against a new ref below) — unlike atoms/ops, a gone ref needs no drop if it was renamed.
    let mut gone_refs_unrecorded = 0u64;
    for (key, in_ref) in &in_refs {
        if out_refs.contains_key(key) {
            continue;
        }
        if !ref_drop_subjects.contains(&(ref_kind_word(&in_ref.kind), in_ref.name.clone())) {
            gone_refs_unrecorded += 1;
        }
    }

    let mut new_refs = 0u64;
    for (key, out_ref) in &out_refs {
        if in_refs.contains_key(key) {
            continue;
        }
        new_refs += 1;
        match &out_ref.status {
            EpistemicStatus::Transformed { step, changed, .. }
                if *step == k
                    && (changed.contains(&ChangedPart::Name)
                        || changed.contains(&ChangedPart::Kind)) => {}
            _ => {
                return Err(Violation::new(format!(
                    "ref {:?} ({:?}) is new but not marked Transformed with name or kind in changed",
                    out_ref.name, out_ref.kind
                )));
            }
        }
        summary.refs_changed += 1;
    }
    if new_refs != gone_refs_unrecorded {
        return Err(Violation::new(format!(
            "{new_refs} renamed ref(s) but {gone_refs_unrecorded} gone ref(s) with no drop — refs may \
             neither be invented nor disappear unrecorded"
        )));
    }

    for (key, in_ref) in &in_refs {
        let Some(out_ref) = out_refs.get(key) else {
            continue; // gone; already checked above.
        };
        // As with parents: comparing id *values* (`id_map[in_ref.target]` vs `out_ref.target`) would
        // always coincide for an honest output, whether or not the target changed, since both sides
        // are computed through the same matching. The real question is whether the ref now names a
        // *different atom*, by identity — there is no substitution rule for a removed target (unlike
        // parents), so a target naming a now-gone atom's identity is exactly that: a real change.
        let in_target_identity = input_by_atom_id.get(&in_ref.target).map(|a| &a.source);
        let out_target_identity = output_by_atom_id.get(&out_ref.target).map(|a| &a.source);
        let target_differs = in_target_identity != out_target_identity;
        let annotation_differs = in_ref.annotation != out_ref.annotation;
        let mut expected = BTreeSet::new();
        if target_differs {
            expected.insert(ChangedPart::Target);
        }
        if annotation_differs {
            expected.insert(ChangedPart::Annotation);
        }
        match &out_ref.status {
            EpistemicStatus::Transformed {
                step,
                changed,
                prior,
            } => {
                if *step != k {
                    return Err(Violation::new(format!(
                        "ref {:?}: marked at step {step}, expected {k}",
                        in_ref.name
                    )));
                }
                if prior.as_ref() != &in_ref.status {
                    return Err(Violation::new(format!(
                        "ref {:?}: prior does not equal the input ref's own status",
                        in_ref.name
                    )));
                }
                if changed_set(changed) != expected {
                    return Err(Violation::new(format!(
                        "ref {:?}: changed does not equal exactly what differs",
                        in_ref.name
                    )));
                }
                summary.refs_changed += 1;
            }
            unmarked => {
                if !expected.is_empty()
                    || in_ref.source != out_ref.source
                    || *unmarked != in_ref.status
                {
                    return Err(Violation::new(format!(
                        "ref {:?}: changed but not marked",
                        in_ref.name
                    )));
                }
            }
        }
    }

    Ok(summary)
}

/// The fixed, lowercase label a `ref`-class drop's `<kind>` token uses (RFC 018 D-6 R-b) — distinct
/// from [`crate::model::RefKind`]'s own `variant()` (a number, not this grammar's word).
fn ref_kind_word(kind: &RefKind) -> String {
    match kind {
        RefKind::Branch => "branch".to_string(),
        RefKind::Tag => "tag".to_string(),
        RefKind::Bookmark => "bookmark".to_string(),
        RefKind::NamedBranch => "named-branch".to_string(),
        RefKind::Other(label) => format!("other:{label}"),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mutation;

mod edit;
pub use edit::{AtomEdit, Edit};

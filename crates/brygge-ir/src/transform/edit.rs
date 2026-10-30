//! [`Edit`]: the honest way to write a transform (RFC 018 D-6) — builds an output `Ir` by editing a
//! working copy of the input's own atoms/refs, computing every mark from the *difference* with the
//! input itself (never from what the caller says), and runs [`super::check`] on its own result before
//! handing it back, so a transform built with this never fails the CLI's own check.

use std::collections::BTreeMap;

use super::{Violation, check};
use crate::artifact::Decoded;
use crate::builder::{AtomDraft, IrBuilder};
use crate::content::BlobId;
use crate::model::{
    Annotation, AtomId, ChangeAtom, ChangedPart, CopyRecord, DropRecord, Identity, Ir, LossClass,
    MetadataClaims, PathOp, RefKind, RefRecord, Text, Time, TransformRecord,
};
use crate::status::EpistemicStatus;
use crate::{Error, Result};

/// A working copy of one input atom's own editable claims and ops (RFC 018 D-6). There is no way to
/// add an atom, or an op the input atom did not already have — every setter here edits something that
/// already exists.
#[derive(Debug, Clone)]
pub struct AtomEdit {
    metadata: MetadataClaims,
    ops: Vec<PathOp>,
    copies: Vec<CopyRecord>,
    removed_ops: BTreeMap<String, String>,
    removed_copies: BTreeMap<String, String>,
}

impl AtomEdit {
    fn from_atom(atom: &ChangeAtom) -> Self {
        Self {
            metadata: atom.metadata.clone(),
            ops: atom.ops.clone(),
            copies: atom.copies.clone(),
            removed_ops: BTreeMap::new(),
            removed_copies: BTreeMap::new(),
        }
    }

    /// Set the claimed author (`None` to state that the source gave none).
    pub fn set_author(&mut self, author: Option<Identity>) {
        self.metadata.author = author;
    }
    /// Set the claimed author time.
    pub fn set_author_time(&mut self, time: Option<Time>) {
        self.metadata.author_time = time;
    }
    /// Set the claimed committer.
    pub fn set_committer(&mut self, committer: Option<Identity>) {
        self.metadata.committer = committer;
    }
    /// Set the claimed commit time.
    pub fn set_commit_time(&mut self, time: Option<Time>) {
        self.metadata.commit_time = time;
    }
    /// Set the claimed message.
    pub fn set_message(&mut self, message: Option<Text>) {
        self.metadata.message = message;
    }

    fn find_op_mut(&mut self, path: &str) -> Result<&mut PathOp> {
        self.ops
            .iter_mut()
            .find(|o| o.path() == path)
            .ok_or_else(|| Error::Invariant(format!("no op at path {path:?} to edit")))
    }

    /// Set an existing op's content address. Refuses a `Delete` op, which carries none.
    ///
    /// # Errors
    /// [`Error::Invariant`] if there is no op at `path`, or it is a `Delete`.
    pub fn set_op_blob(&mut self, path: &str, blob: BlobId) -> Result<()> {
        match self.find_op_mut(path)? {
            PathOp::Add { blob: b, .. }
            | PathOp::Modify { blob: b, .. }
            | PathOp::Replace { blob: b, .. } => {
                *b = blob;
                Ok(())
            }
            PathOp::Delete { .. } => Err(Error::Invariant(format!(
                "op at path {path:?} is a Delete; it has no blob to set"
            ))),
        }
    }

    /// Set an existing op's file mode. Refuses a `Delete` op, which carries none.
    ///
    /// # Errors
    /// [`Error::Invariant`] if there is no op at `path`, or it is a `Delete`.
    pub fn set_op_mode(&mut self, path: &str, mode: u32) -> Result<()> {
        match self.find_op_mut(path)? {
            PathOp::Add { mode: m, .. }
            | PathOp::Modify { mode: m, .. }
            | PathOp::Replace { mode: m, .. } => {
                *m = mode;
                Ok(())
            }
            PathOp::Delete { .. } => Err(Error::Invariant(format!(
                "op at path {path:?} is a Delete; it has no mode to set"
            ))),
        }
    }

    /// Move an existing op to a new path (the only way an op's own path may change).
    ///
    /// # Errors
    /// [`Error::Invariant`] if there is no op at `from_path`, or `to_path` is already in use.
    pub fn move_op(&mut self, from_path: &str, to_path: &str) -> Result<()> {
        if self.ops.iter().any(|o| o.path() == to_path) {
            return Err(Error::Invariant(format!(
                "cannot move an op to {to_path:?}: already in use"
            )));
        }
        let op = self.find_op_mut(from_path)?;
        match op {
            PathOp::Add { path, .. }
            | PathOp::Modify { path, .. }
            | PathOp::Delete { path, .. }
            | PathOp::Replace { path, .. } => *path = to_path.to_string(),
        }
        Ok(())
    }

    /// Remove an existing op, recording why (an `op`-class `Transform` drop at `finish`).
    ///
    /// # Errors
    /// [`Error::Invariant`] if there is no op at `path`.
    pub fn remove_op(&mut self, path: &str, reason: impl Into<String>) -> Result<()> {
        let idx = self
            .ops
            .iter()
            .position(|o| o.path() == path)
            .ok_or_else(|| Error::Invariant(format!("no op at path {path:?} to remove")))?;
        self.ops.remove(idx);
        self.removed_ops.insert(path.to_string(), reason.into());
        Ok(())
    }

    /// Remove an existing copy record (by its own `to`), recording why.
    ///
    /// # Errors
    /// [`Error::Invariant`] if there is no copy to `to`.
    pub fn remove_copy(&mut self, to: &str, reason: impl Into<String>) -> Result<()> {
        let idx = self
            .copies
            .iter()
            .position(|c| c.to == to)
            .ok_or_else(|| Error::Invariant(format!("no copy to {to:?} to remove")))?;
        self.copies.remove(idx);
        self.removed_copies.insert(to.to_string(), reason.into());
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct RefEditState {
    annotation: Option<Annotation>,
    new_name: Option<String>,
}

/// An editable ref (RFC 018 D-6): only its annotation may be changed in place; a name change goes
/// through [`Edit::rename_ref`] instead, since it is also the key [`Edit::ref_mut`]/[`Edit::remove_ref`]
/// address it by.
pub struct RefEdit<'a> {
    state: &'a mut RefEditState,
}

impl RefEdit<'_> {
    /// Set the ref's own annotation.
    pub fn set_annotation(&mut self, annotation: Option<Annotation>) {
        self.state.annotation = annotation;
    }
}

/// Builds a transformed `Ir` by editing a working copy of `input`'s own atoms and refs (RFC 018 D-6).
/// There is no way to add an atom, or an op with no input op — every edit changes, moves, or removes
/// something that already exists. [`Edit::finish`] computes every mark from the difference with
/// `input` itself (never from what the caller said), rebuilds through [`IrBuilder`] so every id and
/// canonical order is recomputed, and then runs [`check`] on its own result — so a transform built with
/// `Edit` passes the CLI's check by construction.
pub struct Edit<'a> {
    input: &'a Decoded,
    transform_name: String,
    transform_version: String,
    transform_params: BTreeMap<String, String>,
    atoms: Vec<AtomEdit>,
    removed_atoms: BTreeMap<usize, String>,
    refs: BTreeMap<(RefKind, String), RefEditState>,
    removed_refs: BTreeMap<(RefKind, String), String>,
    new_blobs: Vec<Vec<u8>>,
}

impl<'a> Edit<'a> {
    /// Start an edit of `input`, naming the transform that will make it (its own record's `name`,
    /// `version`, and `params`).
    #[must_use]
    pub fn begin(
        input: &'a Decoded,
        name: impl Into<String>,
        version: impl Into<String>,
        params: BTreeMap<String, String>,
    ) -> Self {
        let atoms = input.ir.atoms.iter().map(AtomEdit::from_atom).collect();
        let refs = input
            .ir
            .refs
            .iter()
            .map(|r| {
                (
                    (r.kind.clone(), r.name.clone()),
                    RefEditState {
                        annotation: r.annotation.clone(),
                        new_name: None,
                    },
                )
            })
            .collect();
        Self {
            input,
            transform_name: name.into(),
            transform_version: version.into(),
            transform_params: params,
            atoms,
            removed_atoms: BTreeMap::new(),
            refs,
            removed_refs: BTreeMap::new(),
            new_blobs: Vec::new(),
        }
    }

    /// Every atom's own working copy, in the input's own order.
    pub fn atoms(&mut self) -> impl Iterator<Item = &mut AtomEdit> {
        self.atoms.iter_mut()
    }

    /// One atom's own working copy, by its index in the input's own atom list.
    ///
    /// # Errors
    /// [`Error::Invariant`] if `index` is out of range, or that atom was already removed.
    pub fn atom_mut(&mut self, index: usize) -> Result<&mut AtomEdit> {
        if self.removed_atoms.contains_key(&index) {
            return Err(Error::Invariant(format!(
                "atom {index} was already removed"
            )));
        }
        self.atoms
            .get_mut(index)
            .ok_or_else(|| Error::Invariant(format!("no atom at index {index}")))
    }

    /// Remove an atom, recording why (an `atom`-class `Transform` drop at `finish`). Its children
    /// inherit its own parents, in order and without duplicates — the only choice `Edit` makes for the
    /// author.
    ///
    /// # Errors
    /// [`Error::Invariant`] if `index` is out of range, or already removed.
    pub fn remove_atom(&mut self, index: usize, reason: impl Into<String>) -> Result<()> {
        if index >= self.atoms.len() {
            return Err(Error::Invariant(format!("no atom at index {index}")));
        }
        if self.removed_atoms.insert(index, reason.into()).is_some() {
            return Err(Error::Invariant(format!(
                "atom {index} was already removed"
            )));
        }
        Ok(())
    }

    /// Remove an op from one atom, recording why — equivalent to `self.atom_mut(atom)?.remove_op(path,
    /// reason)`, for callers that would rather not hold the `AtomEdit` borrow themselves.
    ///
    /// # Errors
    /// As [`Edit::atom_mut`] and [`AtomEdit::remove_op`].
    pub fn remove_op(&mut self, atom: usize, path: &str, reason: impl Into<String>) -> Result<()> {
        self.atom_mut(atom)?.remove_op(path, reason)
    }

    /// Remove a copy from one atom, recording why.
    ///
    /// # Errors
    /// As [`Edit::atom_mut`] and [`AtomEdit::remove_copy`].
    pub fn remove_copy(&mut self, atom: usize, to: &str, reason: impl Into<String>) -> Result<()> {
        self.atom_mut(atom)?.remove_copy(to, reason)
    }

    /// One ref's own working copy, by its current `(kind, name)`.
    ///
    /// # Errors
    /// [`Error::Invariant`] if no ref has that `(kind, name)`, or it was already removed.
    pub fn ref_mut(&mut self, kind: RefKind, name: &str) -> Result<RefEdit<'_>> {
        let key = (kind, name.to_string());
        if self.removed_refs.contains_key(&key) {
            return Err(Error::Invariant(format!(
                "ref {name:?} was already removed"
            )));
        }
        let state = self
            .refs
            .get_mut(&key)
            .ok_or_else(|| Error::Invariant(format!("no ref {name:?} of that kind")))?;
        Ok(RefEdit { state })
    }

    /// Remove a ref, recording why (a `ref`-class `Transform` drop at `finish`).
    ///
    /// # Errors
    /// [`Error::Invariant`] if no ref has that `(kind, name)`, or it was already removed.
    pub fn remove_ref(
        &mut self,
        kind: RefKind,
        name: &str,
        reason: impl Into<String>,
    ) -> Result<()> {
        let key = (kind, name.to_string());
        if !self.refs.contains_key(&key) {
            return Err(Error::Invariant(format!("no ref {name:?} of that kind")));
        }
        if self.removed_refs.insert(key, reason.into()).is_some() {
            return Err(Error::Invariant(format!(
                "ref {name:?} was already removed"
            )));
        }
        Ok(())
    }

    /// Rename a ref (its kind stays the same).
    ///
    /// # Errors
    /// [`Error::Invariant`] if no ref has that `(kind, name)`, it was already removed, or `new_name` is
    /// already in use by another ref of the same kind.
    pub fn rename_ref(&mut self, kind: RefKind, name: &str, new_name: &str) -> Result<()> {
        let key = (kind.clone(), name.to_string());
        if self.removed_refs.contains_key(&key) {
            return Err(Error::Invariant(format!(
                "ref {name:?} was already removed"
            )));
        }
        if self
            .refs
            .contains_key(&(kind.clone(), new_name.to_string()))
        {
            return Err(Error::Invariant(format!(
                "cannot rename to {new_name:?}: already in use"
            )));
        }
        let state = self
            .refs
            .get_mut(&key)
            .ok_or_else(|| Error::Invariant(format!("no ref {name:?} of that kind")))?;
        state.new_name = Some(new_name.to_string());
        Ok(())
    }

    /// Add a new blob's bytes, for a later [`AtomEdit::set_op_blob`] to reference — content-addressed,
    /// like every other blob.
    pub fn add_blob(&mut self, bytes: Vec<u8>) -> BlobId {
        let id = BlobId::of(&bytes);
        self.new_blobs.push(bytes);
        id
    }

    /// Finish the edit: computes every mark from the difference with the input, writes the new
    /// transform record (`consumed` = the input's own digest) and the drops, rebuilds through
    /// [`IrBuilder`], and runs [`check`] on the result before returning it.
    ///
    /// # Errors
    /// [`Error::Invariant`] if the result does not pass [`check`] (a bug in this function, never in the
    /// caller's own edits — every edit above only ever produces a difference `check` already accepts);
    /// whatever [`IrBuilder::finish`] itself can return.
    pub fn finish(self) -> Result<Ir> {
        let input_ir = &self.input.ir;
        let step = input_ir.transforms.len() as u64;

        let mut builder = IrBuilder::new(input_ir.provenance.clone());
        for id in input_ir.content.ids() {
            let bytes = input_ir.content.get(id)?.ok_or_else(|| {
                Error::Invariant(format!("blob {id} indexed in the input but not readable"))
            })?;
            builder.add_blob(bytes)?;
        }
        for bytes in &self.new_blobs {
            builder.add_blob(bytes.clone())?;
        }

        let mut id_map: BTreeMap<AtomId, AtomId> = BTreeMap::new();
        let input_by_atom_id: BTreeMap<AtomId, &ChangeAtom> =
            input_ir.atoms.iter().map(|a| (a.id, a)).collect();
        let mut dropped: Vec<DropRecord> = Vec::new();

        for (index, in_atom) in input_ir.atoms.iter().enumerate() {
            if let Some(reason) = self.removed_atoms.get(&index) {
                dropped.push(DropRecord {
                    class: LossClass::Transform,
                    what: format!("atom {}", in_atom.id.to_hex()),
                    reason: reason.clone(),
                    step: Some(step),
                });
                continue;
            }
            let Some(edit) = self.atoms.get(index) else {
                return Err(Error::Invariant(format!(
                    "internal: no working copy for atom {index}"
                )));
            };

            let parents = super::mapped_parents(&in_atom.parents, &id_map, &input_by_atom_id);
            // Not `parents != in_atom.parents`: every surviving ancestor gets a *new* id the moment
            // anything about it changes, so comparing the freshly mapped (new-id-space) list against
            // the original (old-id-space) one would differ on id value alone, for every descendant of
            // any edit anywhere upstream — never a real reparenting. The only real change is losing an
            // ancestor and inheriting its own parents in its place.
            let parents_differ = in_atom.parents.iter().any(|p| !id_map.contains_key(p));

            // Ops: diff `edit.ops` against `in_atom.ops`, by path — the same shape `check` itself
            // verifies, computed here instead of asserted, since this is the side that knows which
            // ops were actually touched.
            let in_ops: BTreeMap<&str, &PathOp> =
                in_atom.ops.iter().map(|o| (o.path(), o)).collect();
            let mut ops_differ = false;
            let mut final_ops = Vec::with_capacity(edit.ops.len());
            for op in &edit.ops {
                let path = op.path().to_string();
                match in_ops.get(path.as_str()) {
                    Some(in_op) => {
                        let (in_v, in_b, in_m) = super::op_payload(in_op);
                        let (out_v, out_b, out_m) = super::op_payload(op);
                        debug_assert_eq!(in_v, out_v, "AtomEdit never changes an op's own kind");
                        let mut changed = Vec::new();
                        if in_b != out_b {
                            changed.push(ChangedPart::Blob);
                        }
                        if in_m != out_m {
                            changed.push(ChangedPart::Mode);
                        }
                        if changed.is_empty() {
                            final_ops.push(op.clone());
                        } else {
                            ops_differ = true;
                            final_ops.push(with_op_status(
                                op,
                                EpistemicStatus::Transformed {
                                    step,
                                    changed,
                                    prior: Box::new(in_op.status().clone()),
                                },
                            ));
                        }
                    }
                    None => {
                        // Moved here from some removed/renamed path — a "new" op at this atom.
                        ops_differ = true;
                        final_ops.push(with_op_status(
                            op,
                            EpistemicStatus::Transformed {
                                step,
                                changed: vec![ChangedPart::Path],
                                prior: Box::new(op.status().clone()),
                            },
                        ));
                    }
                }
            }
            for (path, reason) in &edit.removed_ops {
                dropped.push(DropRecord {
                    class: LossClass::Transform,
                    what: format!("op {} {path}", in_atom.id.to_hex()),
                    reason: reason.clone(),
                    step: Some(step),
                });
            }
            for (to, reason) in &edit.removed_copies {
                dropped.push(DropRecord {
                    class: LossClass::Transform,
                    what: format!("copy {} {to}", in_atom.id.to_hex()),
                    reason: reason.clone(),
                    step: Some(step),
                });
            }
            let copies_differ = edit.copies != in_atom.copies || !edit.removed_copies.is_empty();

            let mut actual_changed: Vec<ChangedPart> = Vec::new();
            if edit.metadata.author != in_atom.metadata.author {
                actual_changed.push(ChangedPart::Author);
            }
            if edit.metadata.author_time != in_atom.metadata.author_time {
                actual_changed.push(ChangedPart::AuthorTime);
            }
            if edit.metadata.committer != in_atom.metadata.committer {
                actual_changed.push(ChangedPart::Committer);
            }
            if edit.metadata.commit_time != in_atom.metadata.commit_time {
                actual_changed.push(ChangedPart::CommitTime);
            }
            if edit.metadata.message != in_atom.metadata.message {
                actual_changed.push(ChangedPart::Message);
            }
            if ops_differ {
                actual_changed.push(ChangedPart::Ops);
            }
            if copies_differ {
                actual_changed.push(ChangedPart::Copies);
            }
            if parents_differ {
                actual_changed.push(ChangedPart::Parents);
            }
            actual_changed.sort_by_key(ChangedPart::as_str);

            let status = if actual_changed.is_empty() {
                in_atom.status.clone()
            } else {
                EpistemicStatus::Transformed {
                    step,
                    changed: actual_changed,
                    prior: Box::new(in_atom.status.clone()),
                }
            };

            let new_id = builder.add_atom(AtomDraft {
                parents,
                ops: final_ops,
                copies: edit.copies.clone(),
                metadata: edit.metadata.clone(),
                source: in_atom.source.clone(),
                status,
            })?;
            id_map.insert(in_atom.id, new_id);
        }

        for ((kind, name), state) in &self.refs {
            if let Some(reason) = self.removed_refs.get(&(kind.clone(), name.clone())) {
                dropped.push(DropRecord {
                    class: LossClass::Transform,
                    what: format!("ref {} {name}", super::ref_kind_word(kind)),
                    reason: reason.clone(),
                    step: Some(step),
                });
                continue;
            }
            let Some(in_ref) = input_ir
                .refs
                .iter()
                .find(|r| &r.kind == kind && &r.name == name)
            else {
                return Err(Error::Invariant(format!(
                    "internal: no input ref {name:?} of that kind"
                )));
            };
            let final_name = state.new_name.clone().unwrap_or_else(|| name.clone());
            let target = *id_map.get(&in_ref.target).ok_or_else(|| {
                Error::Invariant(format!(
                    "ref {name:?} targets atom {}, which was removed with no retarget — remove_ref it \
                     explicitly instead",
                    in_ref.target
                ))
            })?;
            let mut changed = Vec::new();
            if state.new_name.is_some() {
                changed.push(ChangedPart::Name);
            }
            // `target` is only ever the *same* atom's own, freshly mapped id — `Edit` has no way to
            // retarget a ref to a genuinely different atom, so `Target` is never marked here (unlike
            // `check`'s own general rule, which must still accept a tool that does retarget one).
            if state.annotation != in_ref.annotation {
                changed.push(ChangedPart::Annotation);
            }
            let status = if changed.is_empty() {
                in_ref.status.clone()
            } else {
                changed.sort_by_key(ChangedPart::as_str);
                EpistemicStatus::Transformed {
                    step,
                    changed,
                    prior: Box::new(in_ref.status.clone()),
                }
            };
            builder.add_ref(RefRecord {
                name: final_name,
                kind: kind.clone(),
                target,
                status,
                source: in_ref.source.clone(),
                annotation: state.annotation.clone(),
            })?;
        }

        let mut loss = input_ir.loss.clone();
        loss.dropped.extend(dropped);
        builder.set_loss(loss);
        for flag in &input_ir.flags {
            builder.add_flag(flag.clone());
        }
        builder.add_transform(TransformRecord {
            name: self.transform_name.clone(),
            version: self.transform_version.clone(),
            params: self.transform_params.clone(),
            consumed: self.input.digest,
        });

        let output_ir = builder.finish()?;

        let output_bytes = crate::artifact::to_bytes(&output_ir);
        let output_decoded = crate::artifact::from_bytes(&output_bytes).map_err(|e| {
            Error::Invariant(format!("the edited Ir does not even round-trip: {e}"))
        })?;
        check(self.input, &output_decoded, &self.transform_name).map_err(|v: Violation| {
            Error::Invariant(format!("Edit produced an output check refuses: {v}"))
        })?;

        Ok(output_ir)
    }
}

fn with_op_status(op: &PathOp, status: EpistemicStatus) -> PathOp {
    match op.clone() {
        PathOp::Add {
            path, blob, mode, ..
        } => PathOp::Add {
            path,
            blob,
            mode,
            status,
        },
        PathOp::Modify {
            path, blob, mode, ..
        } => PathOp::Modify {
            path,
            blob,
            mode,
            status,
        },
        PathOp::Delete { path, .. } => PathOp::Delete { path, status },
        PathOp::Replace {
            path, blob, mode, ..
        } => PathOp::Replace {
            path,
            blob,
            mode,
            status,
        },
    }
}

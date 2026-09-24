//! Deterministic IR construction (RFC 003 D-5, RFC 011 §2.5).
//!
//! [`IrBuilder`] is the sanctioned way to build an [`Ir`]: it canonicalizes each atom's operations and
//! copies, computes the content-addressed [`AtomId`], validates that refs target known atoms and that a
//! copy's `from_atom` was already added, and on [`IrBuilder::finish`] emits atoms in a deterministic
//! **topological order with a total tiebreak by source atom id**, refs/drops/flags in their own
//! canonical orders, and prunes any blob no op ended up referencing — so re-decoding the same source
//! yields byte-identical output (`VF-1`).

use std::collections::{BTreeMap, BTreeSet};

use crate::content::{BlobId, ContentStore};
use crate::model::{
    AtomId, ChangeAtom, CopyRecord, Flag, ImportProvenance, Ir, LossBoundary, MetadataClaims,
    PathOp, RefRecord, SourceIdentity,
};
use crate::status::EpistemicStatus;
use crate::{Error, Result};

/// An atom to add, without its (computed) id.
#[derive(Debug, Clone)]
pub struct AtomDraft {
    /// Parent atom ids, order significant.
    pub parents: Vec<AtomId>,
    /// Path operations (any order; the builder canonicalizes).
    pub ops: Vec<PathOp>,
    /// Copy records (any order; the builder canonicalizes).
    pub copies: Vec<CopyRecord>,
    /// Message/authorship claims.
    pub metadata: MetadataClaims,
    /// The source's opaque identity for this atom.
    pub source: SourceIdentity,
    /// Atom-level epistemic status.
    pub status: EpistemicStatus,
}

/// Builds an [`Ir`] deterministically.
#[derive(Debug)]
pub struct IrBuilder {
    content: ContentStore,
    atoms: Vec<ChangeAtom>,
    known: BTreeSet<AtomId>,
    refs: Vec<RefRecord>,
    provenance: ImportProvenance,
    loss: LossBoundary,
    flags: Vec<Flag>,
}

impl IrBuilder {
    /// Start a build with the given import provenance.
    #[must_use]
    pub fn new(provenance: ImportProvenance) -> Self {
        Self {
            content: ContentStore::new(),
            atoms: Vec::new(),
            known: BTreeSet::new(),
            refs: Vec::new(),
            provenance,
            loss: LossBoundary::default(),
            flags: Vec::new(),
        }
    }

    /// Insert file bytes into the content store, returning the content address.
    pub fn add_blob(&mut self, bytes: Vec<u8>) -> BlobId {
        self.content.insert(bytes)
    }

    /// Add an atom, canonicalizing its operations and copies and computing its id.
    ///
    /// # Errors
    /// [`Error::Invariant`] if two ops in `draft.ops` share a path, or a copy's `from_atom` is not an
    /// atom already added.
    pub fn add_atom(&mut self, draft: AtomDraft) -> Result<AtomId> {
        let mut ops = draft.ops;
        ops.sort_by(|a, b| a.path().cmp(b.path()));
        for (a, b) in ops.iter().zip(ops.iter().skip(1)) {
            if a.path() == b.path() {
                return Err(Error::Invariant(format!(
                    "two ops on one path in one atom: {:?}",
                    a.path()
                )));
            }
        }
        let mut copies = draft.copies;
        copies.sort_by(|a, b| (&a.to, &a.from, a.from_atom).cmp(&(&b.to, &b.from, b.from_atom)));
        for copy in &copies {
            if !self.known.contains(&copy.from_atom) {
                return Err(Error::Invariant(format!(
                    "copy of {:?} names from_atom {} which was not already added",
                    copy.from, copy.from_atom
                )));
            }
        }

        let mut atom = ChangeAtom {
            id: AtomId([0u8; 32]),
            parents: draft.parents,
            ops,
            copies,
            metadata: draft.metadata,
            source: draft.source,
            status: draft.status,
        };
        atom.id = atom.compute_id();
        let id = atom.id;
        self.known.insert(id);
        self.atoms.push(atom);
        Ok(id)
    }

    /// Add a ref. The target must be an atom already added.
    ///
    /// # Errors
    /// [`Error::Invariant`] if `record.target` is not a known atom.
    pub fn add_ref(&mut self, record: RefRecord) -> Result<()> {
        if !self.known.contains(&record.target) {
            return Err(Error::Invariant(format!(
                "ref {:?} targets unknown atom {}",
                record.name, record.target
            )));
        }
        self.refs.push(record);
        Ok(())
    }

    /// Set the loss boundary for this import.
    pub fn set_loss(&mut self, loss: LossBoundary) {
        self.loss = loss;
    }

    /// Add a flagged condition (RFC 011 D-8) — drives CLI exit 30.
    pub fn add_flag(&mut self, flag: Flag) {
        self.flags.push(flag);
    }

    /// Finish the build: order the atoms canonically, sort refs/drops/flags, prune unreferenced blobs,
    /// and assemble the [`Ir`].
    ///
    /// # Errors
    /// [`Error::Invariant`] if the atom graph contains a cycle (source histories are acyclic; a cycle
    /// signals a decoder bug), or if two refs share `(name, kind variant)`, two drops share
    /// `(class, what)`, or two flags share `(kind, what)` (the reader would refuse such an artifact).
    pub fn finish(self) -> Result<Ir> {
        let ordered = topo_order(&self.atoms)?;

        // The canonical orders are on the variant numbers of the format (`ir-artifact-format.md` §6), the
        // same numbers the reader checks. A key that repeats cannot be written: the reader would refuse it.
        let mut refs = self.refs;
        refs.sort_by(|a, b| (&a.name, a.kind.variant()).cmp(&(&b.name, b.kind.variant())));
        if let Some((a, _)) = refs
            .iter()
            .zip(refs.iter().skip(1))
            .find(|(a, b)| (&a.name, a.kind.variant()) == (&b.name, b.kind.variant()))
        {
            return Err(Error::Invariant(format!(
                "two refs named {:?} of one kind",
                a.name
            )));
        }

        let mut loss = self.loss;
        loss.dropped
            .sort_by(|a, b| (a.class.variant(), &a.what).cmp(&(b.class.variant(), &b.what)));
        if let Some((a, _)) = loss
            .dropped
            .iter()
            .zip(loss.dropped.iter().skip(1))
            .find(|(a, b)| (a.class.variant(), &a.what) == (b.class.variant(), &b.what))
        {
            return Err(Error::Invariant(format!(
                "two drops of one class with the same `what`: {:?}",
                a.what
            )));
        }

        let mut flags = self.flags;
        flags.sort_by(|a, b| (a.kind.variant(), &a.what).cmp(&(b.kind.variant(), &b.what)));
        if let Some((a, _)) = flags
            .iter()
            .zip(flags.iter().skip(1))
            .find(|(a, b)| (a.kind.variant(), &a.what) == (b.kind.variant(), &b.what))
        {
            return Err(Error::Invariant(format!(
                "two flags of one kind with the same `what`: {:?}",
                a.what
            )));
        }

        let referenced: BTreeSet<BlobId> = ordered
            .iter()
            .flat_map(|a| a.ops.iter())
            .filter_map(|op| match op {
                PathOp::Add { blob, .. }
                | PathOp::Modify { blob, .. }
                | PathOp::Replace { blob, .. } => Some(*blob),
                PathOp::Delete { .. } => None,
            })
            .collect();
        let mut content = ContentStore::new();
        for (id, bytes) in self.content.iter_sorted() {
            if referenced.contains(id) {
                content.insert(bytes.to_vec());
            }
        }

        Ok(Ir {
            atoms: ordered,
            refs,
            provenance: self.provenance,
            loss,
            flags,
            content,
        })
    }
}

/// Deterministic topological order: Kahn's algorithm, with the ready set ordered by
/// (source atom id, `AtomId`) so ties break the same way every run (RFC 003 D-5, RFC 011 §2.5). Returns
/// only the id sequence — shared between [`topo_order`] (construction) and
/// [`crate::artifact::from_bytes`] (decode-time verification that stored atoms are already in this
/// order). `None` means the atom graph contains a cycle.
pub(crate) fn canonical_order_ids(atoms: &[ChangeAtom]) -> Option<Vec<AtomId>> {
    let by_id: BTreeMap<AtomId, &ChangeAtom> = atoms.iter().map(|a| (a.id, a)).collect();
    // in-degree counts only parents that are part of this set (external parents are treated as roots).
    let mut indegree: BTreeMap<AtomId, usize> = atoms.iter().map(|a| (a.id, 0usize)).collect();
    let mut children: BTreeMap<AtomId, Vec<AtomId>> = BTreeMap::new();
    for a in atoms {
        for p in &a.parents {
            if by_id.contains_key(p) {
                children.entry(*p).or_default().push(a.id);
                if let Some(d) = indegree.get_mut(&a.id) {
                    *d += 1;
                }
            }
        }
    }
    // ready set keyed by (source atom id bytes, atom id) for a stable total order.
    let mut ready: BTreeSet<(Vec<u8>, AtomId)> = BTreeSet::new();
    for a in atoms {
        if indegree.get(&a.id).copied().unwrap_or(0) == 0 {
            ready.insert((a.source.atom_id.clone(), a.id));
        }
    }
    let mut out: Vec<AtomId> = Vec::with_capacity(atoms.len());
    while let Some(key) = ready.iter().next().cloned() {
        ready.remove(&key);
        let id = key.1;
        out.push(id);
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                if let Some(d) = indegree.get_mut(kid) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        if let Some(k) = by_id.get(kid) {
                            ready.insert((k.source.atom_id.clone(), *kid));
                        }
                    }
                }
            }
        }
    }
    if out.len() != atoms.len() {
        return None;
    }
    Some(out)
}

/// Order `atoms` canonically (construction time — see [`canonical_order_ids`]).
fn topo_order(atoms: &[ChangeAtom]) -> Result<Vec<ChangeAtom>> {
    let order = canonical_order_ids(atoms).ok_or_else(|| {
        Error::Invariant("atom graph contains a cycle (source history must be acyclic)".to_string())
    })?;
    let mut by_id: BTreeMap<AtomId, ChangeAtom> =
        atoms.iter().cloned().map(|a| (a.id, a)).collect();
    Ok(order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect())
}

#[cfg(test)]
mod tests;

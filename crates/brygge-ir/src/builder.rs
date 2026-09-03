//! Deterministic IR construction (RFC 003 D-5).
//!
//! [`IrBuilder`] is the sanctioned way to build an [`Ir`]: it canonicalizes each atom's operations,
//! computes the content-addressed [`AtomId`], validates that refs target known atoms, and on
//! [`IrBuilder::finish`] emits atoms in a deterministic **topological order with a total tiebreak by
//! source atom id** — so re-decoding the same source yields byte-identical output (`VF-1`).

use std::collections::{BTreeMap, BTreeSet};

use crate::Error;
use crate::content::{BlobId, ContentStore};
use crate::model::{
    AtomId, ChangeAtom, ImportProvenance, Ir, LossBoundary, MetadataClaims, PathOp, RefRecord,
    RenameHint, SourceIdentity,
};
use crate::status::EpistemicStatus;
use crate::version::{self, ContractVersion};

/// An atom to add, without its (computed) id.
#[derive(Debug, Clone)]
pub struct AtomDraft {
    /// Parent atom ids, order significant.
    pub parents: Vec<AtomId>,
    /// Path operations (any order; the builder canonicalizes).
    pub ops: Vec<PathOp>,
    /// Rename hints (any order; the builder canonicalizes).
    pub rename_hints: Vec<RenameHint>,
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
    contract_version: ContractVersion,
    content: ContentStore,
    atoms: Vec<ChangeAtom>,
    known: BTreeSet<AtomId>,
    refs: Vec<RefRecord>,
    provenance: ImportProvenance,
    loss: LossBoundary,
}

impl IrBuilder {
    /// Start a build with the given import provenance; the contract version is the current one.
    #[must_use]
    pub fn new(provenance: ImportProvenance) -> Self {
        Self {
            contract_version: version::CURRENT,
            content: ContentStore::new(),
            atoms: Vec::new(),
            known: BTreeSet::new(),
            refs: Vec::new(),
            provenance,
            loss: LossBoundary::default(),
        }
    }

    /// Insert file bytes into the content store, returning the content address.
    pub fn add_blob(&mut self, bytes: Vec<u8>) -> BlobId {
        self.content.insert(bytes)
    }

    /// Add an atom, canonicalizing its operations and computing its id.
    pub fn add_atom(&mut self, draft: AtomDraft) -> AtomId {
        let mut ops = draft.ops;
        // Canonical op order: by path (stable). Two ops on one path in one atom is a decoder concern.
        ops.sort_by(|a, b| a.path().cmp(b.path()));
        let mut rename_hints = draft.rename_hints;
        rename_hints.sort_by(|a, b| {
            (a.from.as_str(), a.to.as_str()).cmp(&(b.from.as_str(), b.to.as_str()))
        });

        let mut atom = ChangeAtom {
            id: AtomId([0u8; 32]),
            parents: draft.parents,
            ops,
            rename_hints,
            metadata: draft.metadata,
            source: draft.source,
            status: draft.status,
        };
        atom.id = atom.compute_id();
        let id = atom.id;
        self.known.insert(id);
        self.atoms.push(atom);
        id
    }

    /// Add a ref. The target must be an atom already added.
    ///
    /// # Errors
    /// [`Error::Invariant`] if `record.target` is not a known atom.
    pub fn add_ref(&mut self, record: RefRecord) -> Result<(), Error> {
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

    /// Finish the build: order the atoms canonically and assemble the [`Ir`].
    ///
    /// # Errors
    /// [`Error::Invariant`] if the atom graph contains a cycle (source histories are acyclic; a cycle
    /// signals a decoder bug).
    pub fn finish(self) -> Result<Ir, Error> {
        let ordered = topo_order(&self.atoms)?;
        // Refs are stored in canonical name order for determinism.
        let mut refs = self.refs;
        refs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Ir {
            contract_version: self.contract_version,
            atoms: ordered,
            refs,
            provenance: self.provenance,
            loss: self.loss,
            content: self.content,
        })
    }
}

/// Deterministic topological order: Kahn's algorithm, with the ready set ordered by
/// (source atom id, `AtomId`) so ties break the same way every run (RFC 003 D-5).
fn topo_order(atoms: &[ChangeAtom]) -> Result<Vec<ChangeAtom>, Error> {
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
    let mut out: Vec<ChangeAtom> = Vec::with_capacity(atoms.len());
    while let Some(key) = ready.iter().next().cloned() {
        ready.remove(&key);
        let id = key.1;
        if let Some(atom) = by_id.get(&id) {
            out.push((*atom).clone());
        }
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
        return Err(Error::Invariant(
            "atom graph contains a cycle (source history must be acyclic)".to_string(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

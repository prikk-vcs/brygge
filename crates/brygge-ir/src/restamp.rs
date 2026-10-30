//! [`Ir::restamped`]: relabel an `Ir` to the versions a *different* decode of it recorded (review 106
//! V-1), so `--against-source` can compare two decodes made by different brygge versions without a
//! version difference alone reading as tampering.
//!
//! This is not a transform (RFC 018 D-6): it changes no history, invents nothing, and records no
//! `TransformRecord`. It only edits the one thing a decoder itself chooses — which brygge and decoder
//! version it stamps — and rebuilds through [`IrBuilder`] because [`crate::model::AtomId`] is a hash
//! over an atom's own status, which a `Derivation`'s `decoder_version` is part of.

use std::collections::BTreeMap;

use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{AtomId, ChangeAtom, CopyRecord, Ir, PathOp, RefRecord};
use crate::status::EpistemicStatus;
use crate::{Error, Result};

impl Ir {
    /// Relabel this `Ir` as if `brygge_version` and `decoder_version` had decoded it: sets
    /// `provenance.brygge_version`/`decoder_version`, and `decoder_version` on every
    /// [`crate::status::Derivation`] whose `by` equals `provenance.decoder` (in every atom's, op's,
    /// copy's and ref's status, and through [`EpistemicStatus::Transformed`] priors), then rebuilds
    /// through [`IrBuilder`] so every id, parent list, copy source and ref target stays consistent
    /// with the relabelled atoms.
    ///
    /// Never a way to relabel an artifact as something else's work: it changes only what a decoder
    /// itself would have stamped, nothing a decoder reads from the source. It is a no-op — byte for
    /// byte, once written — when `brygge_version` and `decoder_version` already equal this `Ir`'s own.
    ///
    /// # Errors
    /// [`Error::Invariant`] if the `Ir` is malformed in a way [`IrBuilder`] itself would refuse (a
    /// parent, copy source or ref target with no match among this `Ir`'s own atoms) — never on a
    /// well-formed `Ir`, since restamping neither adds nor removes an atom, a copy or a ref.
    pub fn restamped(self, brygge_version: &str, decoder_version: &str) -> Result<Ir> {
        let Ir {
            atoms,
            refs,
            provenance,
            loss,
            flags,
            content,
            transforms,
        } = self;

        let decoder = provenance.decoder.clone();
        let mut new_provenance = provenance;
        new_provenance.brygge_version = brygge_version.to_string();
        new_provenance.decoder_version = decoder_version.to_string();

        let mut builder = IrBuilder::new(new_provenance);
        for id in content.ids() {
            let bytes = content.get(id)?.ok_or_else(|| {
                Error::Invariant(format!("blob {id} indexed in this Ir but not readable"))
            })?;
            builder.add_blob(bytes)?;
        }

        let mut id_map: BTreeMap<AtomId, AtomId> = BTreeMap::new();
        for atom in atoms {
            let ChangeAtom {
                id: old_id,
                parents,
                ops,
                copies,
                metadata,
                source,
                status,
            } = atom;

            let new_parents = parents
                .iter()
                .map(|p| {
                    id_map.get(p).copied().ok_or_else(|| {
                        Error::Invariant(format!(
                            "atom {old_id} names parent {p}, not among this Ir's own atoms before it"
                        ))
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let new_ops = ops
                .into_iter()
                .map(|op| restamp_op(op, &decoder, decoder_version))
                .collect();
            let new_copies = copies
                .into_iter()
                .map(|copy| restamp_copy(copy, &id_map, &decoder, decoder_version))
                .collect::<Result<Vec<_>>>()?;
            let new_status = restamp_status(status, &decoder, decoder_version);

            let new_id = builder.add_atom(AtomDraft {
                parents: new_parents,
                ops: new_ops,
                copies: new_copies,
                metadata,
                source,
                status: new_status,
            })?;
            id_map.insert(old_id, new_id);
        }

        for r in refs {
            let RefRecord {
                name,
                kind,
                target,
                status,
                source,
                annotation,
            } = r;
            let new_target = *id_map.get(&target).ok_or_else(|| {
                Error::Invariant(format!(
                    "ref {name:?} targets atom {target}, not among this Ir's own atoms"
                ))
            })?;
            builder.add_ref(RefRecord {
                name,
                kind,
                target: new_target,
                status: restamp_status(status, &decoder, decoder_version),
                source,
                annotation,
            })?;
        }

        builder.set_loss(loss);
        for flag in flags {
            builder.add_flag(flag);
        }
        for t in transforms {
            builder.add_transform(t);
        }

        builder.finish()
    }
}

/// Restamp one status: a [`Derivation`] whose `by` names this decoder gets `decoder_version`; a
/// [`EpistemicStatus::Transformed`] status is restamped through its `prior`, since a transform's own
/// `step`/`changed` never depend on which brygge decoded the input.
fn restamp_status(
    status: EpistemicStatus,
    decoder: &str,
    decoder_version: &str,
) -> EpistemicStatus {
    match status {
        EpistemicStatus::Stated => EpistemicStatus::Stated,
        EpistemicStatus::Derived(mut d) => {
            if d.by == decoder {
                d.decoder_version = decoder_version.to_string();
            }
            EpistemicStatus::Derived(d)
        }
        EpistemicStatus::Transformed {
            step,
            changed,
            prior,
        } => EpistemicStatus::Transformed {
            step,
            changed,
            prior: Box::new(restamp_status(*prior, decoder, decoder_version)),
        },
    }
}

fn restamp_op(op: PathOp, decoder: &str, decoder_version: &str) -> PathOp {
    match op {
        PathOp::Add {
            path,
            blob,
            mode,
            status,
        } => PathOp::Add {
            path,
            blob,
            mode,
            status: restamp_status(status, decoder, decoder_version),
        },
        PathOp::Modify {
            path,
            blob,
            mode,
            status,
        } => PathOp::Modify {
            path,
            blob,
            mode,
            status: restamp_status(status, decoder, decoder_version),
        },
        PathOp::Delete { path, status } => PathOp::Delete {
            path,
            status: restamp_status(status, decoder, decoder_version),
        },
        PathOp::Replace {
            path,
            blob,
            mode,
            status,
        } => PathOp::Replace {
            path,
            blob,
            mode,
            status: restamp_status(status, decoder, decoder_version),
        },
    }
}

fn restamp_copy(
    copy: CopyRecord,
    id_map: &BTreeMap<AtomId, AtomId>,
    decoder: &str,
    decoder_version: &str,
) -> Result<CopyRecord> {
    let CopyRecord {
        from,
        from_atom,
        to,
        status,
    } = copy;
    let new_from_atom = *id_map.get(&from_atom).ok_or_else(|| {
        Error::Invariant(format!(
            "copy to {to:?} names from_atom {from_atom}, not among this Ir's own atoms before it"
        ))
    })?;
    Ok(CopyRecord {
        from,
        from_atom: new_from_atom,
        to,
        status: restamp_status(status, decoder, decoder_version),
    })
}

#[cfg(test)]
mod tests;

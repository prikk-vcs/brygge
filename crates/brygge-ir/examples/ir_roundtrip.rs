//! Build a tiny IR by hand, serialize it, read it back, and show the honesty surfaces — with **no
//! source repository and no decoder** (RFC 001 handoff §9). Demonstrates the whole light core standing
//! alone: the model, the canonical content-addressed + digested artifact, and the recoverable fidelity
//! report.
//!
//! Run from the workspace root:
//!
//! ```sh
//! cargo run -p brygge-ir --example ir_roundtrip
//! ```

use std::collections::BTreeMap;

use brygge_ir::{
    AtomDraft, Derivation, DerivationKind, DropRecord, EpistemicStatus, ImportProvenance,
    IrBuilder, LossBoundary, LossClass, MetadataClaims, PathOp, RefKind, RefRecord, RenameHint,
    SourceIdentity, SourceKind, from_bytes, summary, to_bytes,
};

fn git_source(atom: &str) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"example-repo".to_vec(),
        atom_id: atom.as_bytes().to_vec(),
        signatures: Vec::new(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut b = IrBuilder::new(ImportProvenance {
        source: git_source("HEAD"),
        brygge_version: env!("CARGO_PKG_VERSION").to_string(),
        decoder: "example (hand-built)".to_string(),
        decoder_version: "0".to_string(),
        params: BTreeMap::new(),
        import_time: Some(1_725_000_000),
    });

    // Atom 1: create readme.txt — a source-STATED add.
    let readme = b.add_blob(b"hello, brygge\n".to_vec());
    let a1 = b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "readme.txt".into(),
            blob: readme,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        rename_hints: vec![],
        metadata: MetadataClaims {
            message: Some("initial commit".into()),
            author_time: Some(1_700_000_000),
            ..MetadataClaims::default()
        },
        source: git_source("c1"),
        status: EpistemicStatus::Stated,
    });

    // Atom 2: move readme.txt -> docs/readme.txt. Git records delete+create; the rename is INFERRED,
    // so the literal ops are Stated and the rename hint is Derived (RFC 001 D-3).
    let moved = b.add_blob(b"hello, brygge\n".to_vec()); // same content -> same BlobId (dedup)
    let a2 = b.add_atom(AtomDraft {
        parents: vec![a1],
        ops: vec![
            PathOp::Delete {
                path: "readme.txt".into(),
                status: EpistemicStatus::Stated,
            },
            PathOp::Add {
                path: "docs/readme.txt".into(),
                blob: moved,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            },
        ],
        rename_hints: vec![RenameHint {
            from: "readme.txt".into(),
            to: "docs/readme.txt".into(),
            status: EpistemicStatus::Derived(Derivation {
                kind: DerivationKind::InferredRename,
                by: "example".into(),
                decoder_version: "0".into(),
                params: {
                    let mut p = BTreeMap::new();
                    p.insert("algorithm".into(), "identical-content".into());
                    p.insert("threshold".into(), "100".into());
                    p
                },
                confidence: Some(100),
            }),
        }],
        metadata: MetadataClaims {
            message: Some("move readme under docs/".into()),
            author_time: Some(1_700_000_100),
            ..MetadataClaims::default()
        },
        source: git_source("c2"),
        status: EpistemicStatus::Stated,
    });

    b.add_ref(RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: a2,
        status: EpistemicStatus::Stated,
        source: Some(git_source("refs/heads/main")),
    })?;

    // A recorded drop: representation-only data brygge does not carry (HO-2).
    b.set_loss(LossBoundary {
        dropped: vec![DropRecord {
            class: LossClass::Representation,
            what: "packfile layout, reflogs".into(),
            reason: "representation not assertion; reconstructible/local".into(),
        }],
    });

    let ir = b.finish()?;

    // Serialize → read back → prove they match.
    let bytes = to_bytes(&ir);
    let back = from_bytes(&bytes)?; // verifies digest, blob content-addresses, referential integrity
    assert_eq!(back, ir, "round-trip must be exact");
    println!("artifact: {} bytes; round-trip verified\n", bytes.len());

    // Inspect: each atom's epistemic status and its rename hints.
    println!("atoms (topological order):");
    for atom in &back.atoms {
        let kind = match &atom.status {
            EpistemicStatus::Stated => "stated".to_string(),
            EpistemicStatus::Derived(d) => format!("derived:{}", d.kind.label()),
        };
        let msg = atom.metadata.message.as_deref().unwrap_or("");
        let hex = atom.id.to_hex();
        let short = hex.get(..12).unwrap_or(hex.as_str());
        println!("  {short} [{kind}] {msg}");
        for h in &atom.rename_hints {
            let hs = if h.status.is_derived() {
                "derived"
            } else {
                "stated"
            };
            println!("      rename {} -> {} ({hs})", h.from, h.to);
        }
    }

    println!("\nloss boundary:");
    for d in &back.loss.dropped {
        println!("  dropped [{:?}] {} — {}", d.class, d.what, d.reason);
    }

    // The fidelity report, reproduced from the artifact alone (FS-02).
    println!("\n{}", summary(&back).render_human());
    Ok(())
}

//! Build a tiny IR by hand, serialize it, read it back, and show the honesty surfaces — with **no
//! source repository and no decoder** (RFC 001 handoff §9). Demonstrates the whole light core standing
//! alone: the model, the canonical tagged-record content-addressed + digested artifact (contract 0.2.0,
//! RFC 011), and the recoverable fidelity report.
//!
//! Run from the workspace root:
//!
//! ```sh
//! cargo run -p brygge-ir --example ir_roundtrip
//! ```

use std::collections::BTreeMap;

use brygge_ir::{
    AtomDraft, CopyRecord, Derivation, DerivationKind, DropRecord, EpistemicStatus,
    ImportProvenance, IrBuilder, LossBoundary, LossClass, MetadataClaims, PathOp, RefKind,
    RefRecord, SourceIdentity, SourceKind, Text, from_bytes, summary, to_bytes,
};

fn git_source(atom: &str) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"example-repo".to_vec(),
        atom_id: atom.as_bytes().to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut b = IrBuilder::new(ImportProvenance {
        source: git_source("HEAD"),
        brygge_version: env!("CARGO_PKG_VERSION").to_string(),
        decoder: "example (hand-built)".to_string(),
        decoder_version: "0".to_string(),
        params: BTreeMap::new(),
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
        copies: vec![],
        metadata: MetadataClaims {
            message: Some(Text::utf8("initial commit")),
            author_time: Some(brygge_ir::Time {
                seconds: 1_700_000_000,
                offset_minutes: None,
            }),
            ..MetadataClaims::default()
        },
        source: git_source("c1"),
        status: EpistemicStatus::Stated,
    })?;

    // Atom 2: move readme.txt -> docs/readme.txt. Git records delete+create; the rename is INFERRED,
    // so the literal ops are Stated and the copy record is Derived (RFC 001 D-3, RFC 011 D-5).
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
        copies: vec![CopyRecord {
            from: "readme.txt".into(),
            from_atom: a1,
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
            message: Some(Text::utf8("move readme under docs/")),
            author_time: Some(brygge_ir::Time {
                seconds: 1_700_000_100,
                offset_minutes: None,
            }),
            ..MetadataClaims::default()
        },
        source: git_source("c2"),
        status: EpistemicStatus::Stated,
    })?;

    b.add_ref(RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: a2,
        status: EpistemicStatus::Stated,
        source: Some(git_source("refs/heads/main")),
        annotation: None,
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
    let decoded = from_bytes(&bytes)?; // verifies digest, blob content-addresses, referential integrity
    assert_eq!(decoded.ir, ir, "round-trip must be exact");
    println!("artifact: {} bytes; round-trip verified\n", bytes.len());

    // Inspect: each atom's epistemic status and its copy records.
    println!("atoms (topological order):");
    for atom in &decoded.ir.atoms {
        let kind = match &atom.status {
            EpistemicStatus::Stated => "stated".to_string(),
            EpistemicStatus::Derived(d) => format!("derived:{}", d.kind.label()),
        };
        let msg = atom
            .metadata
            .message
            .as_ref()
            .and_then(Text::as_utf8)
            .unwrap_or("");
        let hex = atom.id.to_hex();
        let short = hex.get(..12).unwrap_or(hex.as_str());
        println!("  {short} [{kind}] {msg}");
        for c in &atom.copies {
            let label = if atom.is_move(c) { "move" } else { "copy" };
            let cs = if c.status.is_derived() {
                "derived"
            } else {
                "stated"
            };
            println!("      {label} {} -> {} ({cs})", c.from, c.to);
        }
    }

    println!("\nloss boundary:");
    for d in &decoded.ir.loss.dropped {
        println!("  dropped [{:?}] {} — {}", d.class, d.what, d.reason);
    }

    // The fidelity report, reproduced from the artifact alone (FS-02).
    println!("\n{}", summary(&decoded.ir).render_human());
    Ok(())
}

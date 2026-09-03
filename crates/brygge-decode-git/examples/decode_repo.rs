//! Decode a Git repository into a brygge IR and print its fidelity report.
//!
//! Usage: `cargo run -p brygge-decode-git --example decode_repo -- [path-to-repo]`
//! (defaults to the current directory). Add `--detect-renames` to turn on the opt-in, always-marked
//! rename inference (RFC 004 D-3).

fn main() {
    let mut path = ".".to_string();
    let mut detect_renames = false;
    for arg in std::env::args().skip(1) {
        if arg == "--detect-renames" {
            detect_renames = true;
        } else {
            path = arg;
        }
    }

    let opts = brygge_decode_git::Options {
        detect_renames,
        rename_threshold: 100,
    };

    match brygge_decode_git::decode(std::path::Path::new(&path), &opts) {
        Ok(ir) => {
            let bytes = brygge_ir::to_bytes(&ir);
            println!(
                "decoded {}: {} atom(s), {} ref(s), {}-byte artifact\n",
                path,
                ir.atoms.len(),
                ir.refs.len(),
                bytes.len()
            );
            print!("{}", brygge_ir::honesty::summary(&ir).render_human());
            println!("\nfirst atoms (topological order):");
            for atom in ir.atoms.iter().take(8) {
                let hex = atom.id.to_hex();
                let short = hex.get(..12).unwrap_or(hex.as_str());
                let subject = atom
                    .metadata
                    .message
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("");
                println!("  {short}  {subject}");
            }
        }
        Err(e) => {
            eprintln!("decode failed: {e}");
            std::process::exit(1);
        }
    }
}

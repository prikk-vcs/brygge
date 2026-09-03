//! The brygge command-line launcher.
//!
//! This increment wires the workspace together and links [`brygge_ir`] (the light core). The command
//! surface — `decode` / `inspect` / `verify` / `encode` — lands with the source decoders (RFC 002+,
//! ROADMAP Phase A1). For now the binary reports its version and the IR contract version it speaks.

fn main() {
    println!(
        "brygge {} — carry version-control history (Git/Mercurial/SVN/CVS) into an intermediate \
         representation, and encode a target from it.",
        env!("CARGO_PKG_VERSION"),
    );
    println!("IR contract version: {}", brygge_ir::version::CURRENT);
    println!(
        "commands (decode/inspect/verify/encode) arrive with the source decoders — see ROADMAP.md."
    );
}

# brygge-decode-git

brygge's **Git source decoder** (RFC 004): reads a local Git object database with
[`gix`](https://crates.io/crates/gix) and produces a [`brygge-ir`](../brygge-ir) `Ir` — content,
ancestry, and messages carried faithfully **as claims**, entirely *Stated* except for explicitly-marked
*Derived* rename hints (which are opt-in and never replace the literal delete+create Git recorded).

This is the **one crate that links `gix`** (RFC 009 D-1): `brygge-ir`, the encoders, and
`verify --internal` link none of it, so a target checks a brygge import on its own surface. gix is used
with **no network feature** — brygge reads a local object database and performs no network I/O
(INV-3) — and executes **no source-provided code**: no hooks, filters/smudge, or submodule fetch
(RFC 009 D-4). `#![forbid(unsafe_code)]`.

## Usage

```rust,no_run
let ir = brygge_decode_git::decode(
    std::path::Path::new("/path/to/repo/.git"),
    &brygge_decode_git::Options::default(),
)?;
println!("{}", brygge_ir::honesty::summary(&ir).render_human());
# Ok::<(), brygge_decode_git::Error>(())
```

Or run the example against any repository:

```
cargo run -p brygge-decode-git --example decode_repo -- /path/to/repo [--infer-renames]
```

Status: **Built (ROADMAP M1); not yet released** — commits→atoms, tree-snapshot diff→literal ops,
opaque SHA/signature, branches+tags, the owner-ratified floor (submodules, replace/grafts, shallow all
refused), the representation loss boundary, byte-deterministic (pack-independent) output,
`verify --against-source`, and the full CLI surface. Rename inference is off by default and, when on,
marked *Derived* beside the literal ops. A **symbolic ref** (e.g. `refs/remotes/origin/HEAD`, which an
ordinary `git clone` always creates) is not carried as a ref in any namespace — the IR has no alias
concept — and is recorded as a `symbolic refs (N)` drop; its target, if any, is a distinct ref this same
decode carries or drops on its own merits (RFC 004 OQ-B, CR-16). An unparseable author/committer time
becomes an absent claim, never fabricated as `0` or salvaged from a malformed token (NG-5, CR-16). Built
against the RFC 004 handoff, the gix security review, and the CR-16 correction handoff, all in
`rfcs/handoffs/004-git-decoder/`.

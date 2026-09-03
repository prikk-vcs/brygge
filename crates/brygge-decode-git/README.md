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

Status: crate skeleton in place; the decoder is implemented against the RFC 004 program-design handoff
(`rfcs/handoffs/004-git-decoder/`). See also the gix adoption security review in that folder.

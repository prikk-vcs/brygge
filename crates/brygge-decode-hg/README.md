# brygge-decode-hg

brygge's **Mercurial source decoder** (RFC 005): reads a local Mercurial repository's revlog store
**directly** — pure Rust, no `hg` binary, no subprocess (RFC 005 D-1, Tier 2) — and produces a
[`brygge-ir`](../brygge-ir) `Ir`, mostly *Stated*, **including source-recorded renames** carried as
`Stated` (the point of M2: hg records renames, so an hg import shows fewer derived marks than Git).

This is the one crate that reads hg (RFC 009 D-1); `brygge-ir` and `verify --internal` link none of it.
It parses an untrusted store, so every parser is bounds-checked and panic-free, and a repository whose
`.hg/requires` names a format this build does not implement is **refused, never guessed**.

Status: **foundation increment** — the crate, the error/option types, and the format-safety gate
(`requires`) are in place and tested. The revlog reader (index + delta chains + zlib), the
changelog/manifest/filelog mapping, the fncache path encoding, and `decode()` are built against real
Mercurial fixtures (needs Mercurial installed), per the handoff at
`rfcs/handoffs/005-mercurial-decoder/hg-decoder-implementation-handoff-v1.md`.

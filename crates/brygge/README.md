# brygge

The command-line launcher for [brygge](https://github.com/prikk-vcs/brygge) — a tool that carries
version-control history **out of** Git, Mercurial, Subversion, and CVS into an **intermediate
representation** (IR), and **encodes** a target (prikk first) from it, with every derivation and loss
made legible.

This crate is the binary; the reusable core is [`brygge-ir`](https://crates.io/crates/brygge-ir). The
`decode` / `inspect` / `verify` / `encode` commands land with the source decoders (see the repository's
`ROADMAP.md`). Its stance, inherited from prikk's import contract (RFC 113): *facts derive, judgment is
authored, and the join is checked* — a migration tool that silently guesses produces history nobody can
trust.

License: Apache-2.0.

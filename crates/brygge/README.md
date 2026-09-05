# brygge

The command-line launcher for [brygge](https://github.com/prikk-vcs/brygge) — a tool that carries
version-control history **out of** Git, Mercurial, Subversion, and CVS into an **intermediate
representation** (IR), and **encodes** a target (prikk first) from it, with every derivation and loss
made legible.

This crate is the binary; the reusable core is [`brygge-ir`](https://crates.io/crates/brygge-ir). Its
stance, inherited from prikk's import contract (RFC 113): *facts derive, judgment is authored, and the
join is checked* — a migration tool that silently guesses produces history nobody can trust.

## Commands (read side — RFC 004 increment 2)

```
brygge decode git <path> [--ir <out>] [--detect-renames] [--format human|machine]
brygge inspect --ir <file> [--format human|machine]
brygge verify --internal --import <file>              # honesty checks, no source needed (VF-3)
brygge verify --against-source <repo> --import <file> # re-derive & confirm correspondence (VF-2)
brygge summary --import <file> [--format human|machine]
brygge --version | --help
```

`decode` and `verify --against-source` link the Git decoder; `inspect` / `summary` / `verify --internal`
link only `brygge-ir` (RFC 009 D-1). Output is human by default, `--format machine` for CI. **Exit codes**
carry the outcome class (CL-08): `0` clean · `10` recorded loss · `20` floor refusal · `30` convention
violation · `40` partial · `50` verify failed · `1` failure. `encode` is **gated** pending the prikk
import surface (RFC 008) and prints an honest "not available in this build" rather than guessing.

License: Apache-2.0.

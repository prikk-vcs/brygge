# brygge

The command-line launcher for [brygge](https://github.com/prikk-vcs/brygge) — a tool that carries
version-control history **out of** Git, Mercurial, Subversion, and CVS into an **intermediate
representation** (IR), and **encodes** a target (prikk first) from it, with every derivation and loss
made legible.

This crate is the binary; the reusable core is [`brygge-ir`](https://crates.io/crates/brygge-ir). Its
stance, inherited from prikk's import contract (RFC 113): *facts derive, judgment is authored, and the
join is checked* — a migration tool that silently guesses produces history nobody can trust.

## Commands (the three-verb surface, external design v0.3 CL-01…CL-08)

```
brygge decode <git|hg|svn|cvs> <source> --out <artifact> [--infer-renames | --reconstruct-refs] [--format human|machine]
brygge inspect <artifact> [--atoms] [--format human|machine]
brygge verify <artifact> [--against-source <source>] [--format human|machine]
brygge --version | --help | <command> --help
```

`decode` and `verify --against-source` link the matching decoder crate; `inspect` and the internal half of
`verify` link only `brygge-ir` (RFC 009 D-1). Output is human by default, `--format machine` for CI. Every
string that can originate in a source repository is neutralized before it reaches stdout or stderr
(`display`, CR-19), on both forms.

`--out` is required on `decode`: it never runs without producing its artifact, and writes it atomically
(a temp file in the same directory, synced, then renamed over `--out`; on any failure the temp file is
removed and an existing artifact is left untouched — CR-13). `verify` always runs the internal honesty
checks (`integrity`, `structure`, `replay`, `derivations`, `source-invariants`, `provenance`,
`loss-boundary` — each can fail); `--against-source` additionally re-derives and reports a second,
separate result (VF-4).

**Exit codes** carry the outcome class (CL-08): `0` clean · `10` recorded loss · `20` floor refusal or a
resource ceiling hit · `30` convention/confidence violation · `40` partial (reserved) · `50` verify failed
· `1` runtime failure (unreadable input, I/O, an internal decoder fault) · `2` usage error (bad arguments,
an option given to a source kind it does not apply to). `encode` is **not part of the surface yet**
(Track B1, pending prikk's import foundations) and is a usage error, not a hidden command.

License: Apache-2.0.

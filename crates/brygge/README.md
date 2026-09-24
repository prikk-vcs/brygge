# brygge

**brygge** carries version-control history **out of** Git, Mercurial, Subversion and CVS into an
**intermediate representation (IR)** that belongs to no particular system, so a target can import it. It
records what the source actually stated, marks in the artifact itself anything it had to infer, and records
everything it did not carry. It reads untrusted repositories, uses no network, and writes only the file you
name.

This crate is the command-line tool. The IR library is [`brygge-ir`](https://crates.io/crates/brygge-ir).

## Install

```sh
cargo install --locked brygge
```

It needs Rust 1.85 or later. `--locked` builds the exact dependency set brygge was tested with. Prebuilt
binaries for Linux (x86_64, arm64), macOS (Apple Silicon) and Windows (x86_64) are attached to every
[GitHub release](https://github.com/prikk-vcs/brygge/releases) from 0.1.1 on, with checksums and build-provenance
attestations (`gh attestation verify <file> --repo prikk-vcs/brygge`). Decoding a
live Subversion repository also needs `svnadmin`; decoding a dumpfile does not.

## Commands

```
brygge decode <git|hg|svn|cvs> <source> --out <artifact> [--infer-renames | --reconstruct-refs] [--format human|machine]
brygge inspect <artifact> [--atoms] [--format human|machine]
brygge verify <artifact> [--against-source <source>] [--format human|machine]
```

- **`decode`** reads a source into an IR artifact. Before it starts, it states what that source can and
  cannot promise. It writes the artifact atomically, so a failed run leaves an existing artifact untouched.
  - `--infer-renames` (Git) adds rename hints, marked as brygge's judgment.
  - `--reconstruct-refs` (Subversion, CVS) reconstructs branches and tags from convention, also marked.
- **`inspect`** prints the fidelity report: what was carried, what brygge derived, what was dropped, and
  what needs attention. `--atoms` lists every atom with its status and source id.
- **`verify`** checks an artifact's integrity and honesty with nothing but the artifact.
  `--against-source` also decodes the source again and reports, separately, whether the two correspond.

Authorship is always *Unverifiable*: brygge carries who the source claims wrote something, and verifies
no one. Every piece of text from a repository is escaped before it reaches your terminal.

`--format machine` prints stable, versioned `key=value` lines for scripts and CI.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | clean |
| `10` | completed; something was not carried, and it is recorded |
| `20` | refused: a repository shape brygge will not approximate, an unsupported format, or a size ceiling |
| `30` | completed, but flagged as needing attention (e.g. an SVN layout brygge could not follow) |
| `50` | `verify` found a problem |
| `1` | runtime failure, or a `verify` whose requested source comparison could not run |
| `2` | usage error |

## Documentation

- The documentation site: <https://prikk-vcs.github.io/brygge/>
- User guides, one per source: <https://github.com/prikk-vcs/brygge/tree/main/docs/src/guide>
- Machine-readable output: <https://github.com/prikk-vcs/brygge/blob/main/docs/src/reference/machine-output.md>
- The IR artifact format: <https://github.com/prikk-vcs/brygge/blob/main/docs/src/reference/ir-artifact-format.md>
- Security model: <https://github.com/prikk-vcs/brygge/blob/main/docs/src/brygge-03-threat-model-v0.1.md>
- Changes: <https://github.com/prikk-vcs/brygge/blob/main/CHANGELOG.md>

brygge decodes, inspects and verifies. It has no encoder yet: the first, for prikk, waits on prikk's
import foundations.

License: Apache-2.0.

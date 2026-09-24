# brygge

**brygge** (Norwegian: *wharf*, where cargo is landed) carries version-control history **out of** Git,
Mercurial, Subversion and CVS into an **intermediate representation (IR)** that belongs to no particular
system, so a target can import it. prikk is the first target.

The idea everything turns on is **faithfulness with provenance, not neutrality**:
- brygge records what the source actually stated;
- it marks, in the artifact itself, anything it had to infer;
- it records everything it did not carry.

It reads untrusted repositories, uses no network, and writes only the file you name.

## Install

```sh
cargo install --locked brygge
```

brygge needs Rust 1.85 or later to build. Prebuilt binaries for Linux (x86_64, arm64), macOS (Apple
Silicon) and Windows (x86_64) are attached to every
[GitHub release](https://github.com/prikk-vcs/brygge/releases) from 0.1.1 on.

## Three commands

```sh
brygge decode git /path/to/repo --out repo.ir     # or hg | svn | cvs
brygge inspect repo.ir                            # the fidelity report: carried, derived, dropped, flagged
brygge verify repo.ir --against-source /path/to/repo
```

## Where to go next

- **Importing a repository:** the user guide for your source. It says what is carried, what is not,
  what is refused, and what you can do about it.
- **Scripting and CI:** [Machine-readable output](reference/machine-output.md): every key, its versions,
  and the exit codes.
- **Reading or writing artifacts yourself:** [the IR artifact format](reference/ir-artifact-format.md),
  contract 0.2.0.
- **What brygge defends, and how:** [the threat model](brygge-03-threat-model-v0.1.md).
- **The source code, the design decisions (RFCs) and the change log:** the
  [repository on GitHub](https://github.com/prikk-vcs/brygge).

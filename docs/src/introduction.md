# brygge

**brygge carries version-control history from one system to another.** It **decodes** a Git, Mercurial,
Subversion or CVS repository into one portable, verifiable file, an **IR artifact**, that belongs to no system, and
**encodes** that artifact into a target system.

- **Why it exists:** moving history between systems tends to blur fact and guesswork. A migration that hides what it
  guessed cannot be trusted later.
- **Its role:** the bridge between two systems, with the IR artifact as the one handover point, so any source can
  reach any target.
  - **Decoding** is complete for all four sources.
  - **Encoding:** prikk is the first target, through a built-in encoder that follows prikk's own import design
    (planned). Until then, and for other targets, external encoder plugins do it, labelled as not part of brygge.
- **What it does:** it records what the source **stated**, marks what it had to **derive**, and records what it
  could not carry, all inside the artifact. `inspect` shows them, and `verify` checks them, alone or against the
  source. Transform plugins can change an artifact on the way, but only by recording every change. brygge reads
  untrusted repositories, uses no network, and writes only where you tell it.
- **A typical day:** you are leaving CVS. `brygge decode cvs` turns the repository into an artifact.
  `brygge inspect` shows which changesets it had to reconstruct. `brygge verify --against-source` confirms that the
  artifact still matches the repository. Then `brygge encode <target>` hands it to the new system.

**The name:** *brygge* is Norwegian for *wharf*, where cargo is unloaded from one ship, counted, labelled, and loaded
onto the next. brygge does the same for history: every crate that arrived damaged is written down, and every one it
had to repack is marked as repacked. No ship leaves with a crate it cannot account for.

## Install

```sh
cargo install --locked brygge
```

On Arch Linux, from the AUR: `brygge` (built from source) or `brygge-bin` (the release's binary).

brygge needs Rust 1.87 or later to build. Prebuilt binaries for Linux (x86_64, arm64), macOS (Apple
Silicon) and Windows (x86_64) are attached to every
[GitHub release](https://github.com/prikk-vcs/brygge/releases) from 0.1.1 on.

## Four commands

```sh
brygge decode git /path/to/repo --out repo.ir     # or hg | svn | cvs
brygge inspect repo.ir                            # the fidelity report: carried, derived, dropped, flagged
brygge verify repo.ir --against-source /path/to/repo
brygge encode <target> repo.ir                     # through an external encoder plugin
```

## Where to go next

- **Importing a repository:** the user guide for your source. It says what is carried, what is not,
  what is refused, and what you can do about it.
- **Scripting and CI:** [Machine-readable output](reference/machine-output.md): every key, its versions,
  and the exit codes.
- **Writing an encoder or a transform:** [Writing an encoder](reference/writing-an-encoder.md) and
  [Writing a transform](reference/writing-a-transform.md): the rules a plugin must keep, and how brygge runs it.
- **Reading or writing artifacts yourself:** [the IR artifact format](reference/ir-artifact-format.md),
  contract 0.3.0 (still reads 0.2.0).
- **What brygge defends, and how:** [the threat model](brygge-03-threat-model-v0.1.md).
- **The source code, the design decisions (RFCs) and the change log:** the
  [repository on GitHub](https://github.com/prikk-vcs/brygge).

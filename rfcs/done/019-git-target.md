# RFC 019 — Git as a target (0.8.0): `brygge encode git`, checked against the artifact

**Status.** **Accepted (2026-11-21)** by the owner: "All of your recommendations are accepted." OQ-1 to OQ-3 are
ruled as recommended: the encode report goes beside the repository only, the authors map is in 0.8.0, and the result
is a bare repository. Handoffs: `rfcs/handoffs/019-git-target/`. Proposed (2026-11-21) by the architect.
**One refinement at acceptance (the architect, 2026-11-21):** D-1's encoder is a **module of the `brygge` crate**
(`crates/brygge/src/encode_git/`), not a new crate. crates.io's trusted publishing cannot make a crate's first
release, so a new crate would need a one-time manual publish with a token. The module keeps D-1's isolation: it
uses `brygge-ir`'s public API and `std` only, and a check enforces this (handoff G1 §2). Everything else in D-1
stands.
**Refinements at batch G1 (the architect, reviews 136–137):**
- **D-7 gains a refusal:** every atom must be reachable from a ref, so a result never holds history that Git would
  garbage-collect.
- **`Brygge-Source-Id`'s form depends on the source kind:**
  - Git and hg: 40 hexadecimal digits;
  - Subversion: `r<N>`;
  - CVS: one `path@rev` per trailer line;
  - otherwise `hex:`.
- **The CVS decoder's `--reconstruct-refs` adds the main line as a derived branch, `trunk`** (handoff
  `cvs-main-line-ref-handoff-v1.md`). It implements ROADMAP Phase B2
(the owner, 2026-11-14: "Migration support of both subversion to git and cvs to git … officially support them.
However, less prioritized. Our primary target is prikk.").

**Done (2026-11-24): implemented in 0.8.0.**

## Summary

- **What it adds:** brygge gains a supported encoder, `brygge encode git <artifact> --out <dir>`. It writes a new
  Git repository from an IR artifact, so Subversion → Git, CVS → Git and Mercurial → Git migrations go through
  brygge with every derivation and every loss still recorded.
- **brygge then checks its own result:** it decodes the new repository with its own Git decoder and compares it
  with the artifact. A result that does not match is refused and removed.
- **Where it starts:** `examples/encode-git` (0.6.0, unsupported) is the starting point. This RFC turns it into
  brygge's own encoder, and settles the four questions ROADMAP B2 left open: built-in or plugin, where the marks
  go, determinism, and the check.

## Priority

- **prikk comes first** (ROADMAP Track B). B2 runs only while B1 waits on prikk's import foundations; prikk's
  theme 17 is unscheduled (prikk reply 006, 2026-11-14).
- **If prikk schedules theme 17 during this cycle,** the B2 batches not yet started pause, and B1's RFC 008 takes
  their place. Batches already finished still ship.

## Decisions

### D-1 — Built in, as its own crate; `git` runs as a trusted tool

- **A new crate, `brygge-encode-git`,** depends on `brygge-ir` alone, as the example does (PU-3). It writes a
  `git fast-import` stream. It links no Git library.
- **The CLI drives the rest:**
  - `brygge encode git <artifact> --out <dir>`: `<dir>` must not exist;
  - brygge verifies the artifact first (RFC 017 D-3);
  - it creates a bare repository in a temporary sibling of `<dir>`, and runs `git fast-import` into it;
  - it runs D-6's check, then renames the temporary directory to `<dir>`. A refused result leaves nothing at
    `<dir>` (C-7).
- **`git` is a trusted external tool,** under the same rules as `svnadmin` (C-2b): found on `PATH` among absolute
  entries only, spawned directly with no shell, and with a fixed environment. That environment sets
  `GIT_CONFIG_NOSYSTEM=1`, an empty `HOME` and a fixed `LC_ALL`, so no user configuration or hook can change the
  result. `git` is the only new runtime requirement, and only for `encode git`.
- **`git` becomes a reserved target name, like `prikk`.** A plugin named `brygge-encode-git` on `PATH` is not run;
  `encode --list` shows it as shadowed by the built-in.
- **`examples/encode-git` is retired.** Its code moves into the crate, and the encoder guide points to the crate
  instead.
- **Why built-in, not a supported plugin:**
  - official support means brygge's own claims, tests and releases;
  - the D-6 check needs brygge's Git decoder, which only the CLI links;
  - and a plugin's output is, by RFC 017's own labelling, "not brygge's".

### D-2 — The marks live in each commit's trailers (INV-1)

**Every commit and annotated tag ends with one trailer block,** after one blank line, in this order:

| Trailer | When | Says |
|---|---|---|
| `Brygge-Encoded: brygge <version> from <source kind>` | always | this commit was written by brygge from another history, so it is not that history's own record |
| `Brygge-Source-Id: <the atom's source id>` | always | the source's own id (Git commit id, hg node, SVN revision, CVS reconstruction id), so every commit can be traced back |
| `Brygge-Derived: <scope> <kind> <params>` | per derived assertion | as in the example |
| `Brygge-Transformed: <scope> <step> <name> <version> (<changed>)` | per transform layer | as in the example |
| `Brygge-Supplied: <fields>` | when Git's format needed a value the IR does not carry | as the example's `Encoded-supplied` |
| `Brygge-Altered: <field> (<why>)` | when a carried value had to change to be written | as the example's `Encoded-altered` |
| `Brygge-Artifact: sha256:<digest>` | always | the artifact the commit was built from, which holds the full loss record |

- **Why trailers, not `git notes`:** notes live under `refs/notes/`, which `git clone` and an ordinary push do not
  carry. A migrated repository that loses its notes would read as native history, which is exactly what INV-1
  forbids. Trailers are part of the commit, and `git interpret-trailers` parses them.
- **One prefix, `Brygge-`, for every trailer.** The example's `Encoded-*` names are not kept: one name per concept
  (the owner's design philosophy).
- **The repository-level record:**
  - the artifact's drops and flags, and any refusal D-7 overrides, go to an **encode report** written beside the
    repository: `<dir>.brygge-report`, in the machine format of `machine-output.md`;
  - the guide tells users to keep the artifact and the report with the migrated repository. OQ-1 asks whether
    the report should also live inside it.

### D-3 — Refs

| IR ref kind | Git ref |
|---|---|
| `Branch` | `refs/heads/<name>` |
| `Tag` with an annotation | an annotated tag, `refs/tags/<name>` |
| `Tag` without one | a lightweight tag, `refs/tags/<name>` |
| `Bookmark`, `NamedBranch` (hg) | `refs/heads/<name>` |
| `Other` | `refs/brygge/other/<name>`, outside the branch and tag namespaces, so it never reads as either |

- **Checked before writing:**
  - two IR refs that would map to one Git ref are refused, naming both;
  - every name passes Git's own rules (`git check-ref-format`, run once over all names).
- **The default branch (`HEAD`)** is the source's own default when the IR names one. Otherwise brygge uses the ref
  named `trunk`, `main`, `master` or `default`, in that order, and says which it chose.

### D-4 — Identities, and a supported authors map

- **Git needs `name <email>`.** Subversion and CVS record a user name with no email.
  - An absent email is written as `<>` and marked `Brygge-Supplied`, as the example does;
  - names that are not UTF-8, or that hold control characters, are altered and marked, as the example does.
- **A supported transform, `brygge transform authors --map <file>`,** maps source identities to Git identities
  before encoding:
  - it promotes `examples/transform-authors`;
  - it runs inside brygge, recorded as a `Transformed` step (RFC 018 D-5), so every commit it changed carries
    `Brygge-Transformed`;
  - the map file has one line per identity: `<source name> = Name <email>`. An unmapped identity is listed, and
    refused when `--require-all` is given;
  - `brygge inspect --authors <artifact>` lists every identity and its count, so a user can write the map.
- **This is OQ-2**, because it widens the release.

### D-5 — Deterministic output

- **The same artifact and the same brygge version give the same commit ids.** Times and identities come only from
  the IR, the trailers are a fixed function of the IR, and `git fast-import` adds nothing of its own.
- **Tested by encoding twice:** `git for-each-ref` must be identical across the two runs.
- **Across brygge versions, ids can change,** because `Brygge-Encoded` names the version. The release notes say
  so whenever the mapping changes.

### D-6 — brygge checks the result against the artifact

- **After `fast-import`,** brygge decodes the new repository with `brygge-decode-git` and compares it with the
  artifact under D-2 to D-4's mapping, atom by commit:
  - the parents;
  - the tree: every path, its mode and its content address;
  - the author and committer claims, except fields that a `Brygge-Supplied` or `Brygge-Altered` trailer names;
  - the message, which must be the source's message followed by exactly the trailer block;
  - the refs: every IR ref, and nothing else.
- **On any difference:** exit 1, the report names the first differences (bounded), and nothing is left at `<dir>`.
- **Also available on its own,** for a repository someone kept:
  `brygge verify <artifact> --against-target git <repository>`.
- **Why this check matters:** it turns "the encoder follows the mapping" from a claim into something brygge
  checked, the same way `verify --against-source` checks a decoder.

### D-7 — Refusals, all reported before anything is written

- **The pre-flight** lists every refusal in one report, then exits 2. It writes nothing.
- **What it refuses:**
  - a mode Git cannot write (only `100644`, `100755` and `120000` are accepted; `160000` cannot occur, since the
    IR has no submodule);
  - ref-name collisions and invalid names (D-3);
  - a non-UTF-8 annotated-tag message, because `fast-import` has no `encoding` for tags. A commit message keeps
    its declared encoding, as the example does.
- **No override flags in 0.8.0.** A refusal is fixed with a transform (rename a ref, rewrite a message), which
  then records itself.

### D-8 — Scale and evidence

- **Bounded memory:** the stream is written atom by atom, with blobs read from the artifact's store in store
  order. Peak memory follows the history's metadata, as for decode (RFC 015).
- **The release's level-3 evidence:**
  - **Subversion → Git:** `wordpress-core`;
  - **CVS → Git:** `openbsd-games` and `openbsd-cpio`, with and without `--reconstruct-refs`;
  - **Mercurial → Git:** `tortoisehg-thg`;
  - **Git → Git:** `eza` and `fish-shell`. The commit ids are new, because of the trailers, but every tree must
    equal the source commit's tree. This is the strongest correctness check available.
  - Each passes D-6. Time and peak are measured with `release-corpus.py`'s method and recorded in
    `tools/bench/README.md`.

## Amendment A — Subversion branch lines (D-9; accepted 2026-11-22)

**Accepted by the owner (2026-11-22):** "Yes. Accepted." The handoff is `branch-lines-handoff-v1.md` (batch G5). **Refinement at handoff (the architect):** no new `DerivationKind` variant (an unknown variant fails an older reader's read). A line atom is `ReconstructedBranch` with `{layout, root, line: "true"}`, so the contract is unchanged.

**Why.** Encoding wordpress-core showed that RFC 019 as accepted gives Subversion an unusable Git result.
- **What happens:** every commit holds the whole Subversion tree (`branches/`, `tags/`, `trunk/` at the root), and
  the `1.5` branch is a pointer into that one linear history.
- **Why:** the Subversion decoder imports SVN's stated layer, one linear chain of whole-repository revisions, and
  derives refs on top of it (RFC 006 D-4). `encode git` writes that faithfully, and G2's check confirms it.
- **What a migration means instead:** each Git branch holds only its own directory's files, with its own line of
  commits.
- CVS, Mercurial and Git are not affected: their branches already have their own trees. The architect missed this
  in the RFC, and records it here.

### D-9 — `decode svn --branch-lines`: one line of history per branch, as a marked derivation

- **Where:** a new option of the Subversion decoder. It is the derived layer of RFC 006 D-4, widened from "which
  directory is a branch" to "which revisions form that branch's line". It is opt-in and always marked, as
  `--reconstruct-refs` is, which it implies. The default decode (the stated spine) is unchanged.
- **The layout:** the existing layout policy (standard `trunk`/`branches/*`/`tags/*` by default). Each directory the
  policy names is a **root**.
- **Atoms:**
  - each revision gives **one atom per root it touches**, in canonical root order;
  - each atom's ops are that root's ops, with the root's prefix removed, so a branch's files sit at the top of its
    tree;
  - a revision that touches no root gives no atom, and is counted: a representation-class drop, "revisions with no
    branch change (N)". A revision that touches paths outside every root records them as the existing flag
    already does, plus a drop of those ops.
- **Parents:**
  - the previous atom on the same root's line;
  - **a root created by a copy** (`svn cp <src>@<rev> <root>`, the way SVN branches and tags) gets as its first
    parent the newest atom on `<src>`'s line at or before `<rev>`, and its tree starts as that atom's tree. That is
    the branch point;
  - a root created without a copy starts a new line, with no parent.
  - Merges are not inferred (`svn:mergeinfo` stays a recorded property drop, as today).
- **Refs:**
  - each live root gets a ref, as `--reconstruct-refs` gives today (`trunk` and branches as branches, tags as
    tags), targeting its line's last atom;
  - **a root that was deleted** (and each earlier life of a root that was deleted and later re-created) gets a ref
    of kind `Other`, named `<root>@r<last revision it existed>`. `encode git` writes it under `refs/brygge/other/`
    (D-3). Its history stays reachable without reading as a live branch. This settles review 137's deleted-branch
    risk: nothing is unreachable.
- **Marks:**
  - every atom is `Derived`, with a new kind `ReconstructedBranchLine` and params `{layout, root}`;
  - the source id is the revision plus its root, written by `Brygge-Source-Id` as `r<N> <root>` (for example
    `r1234 branches/1.5`). This extends F-7's Subversion form.
- **Verification:** `--against-source` re-derives with the options recorded in the artifact, including
  `branch_lines`, as for every other derived layer. `encode git`'s check (G2) needs no change.
- **The contract:** unchanged. The IR already has every field this uses. The new `DerivationKind` variant is
  additive under RFC 011's rules, and must be checked to be so; if it is not, contract 0.3.1 carries it.
- **The guide:** for Subversion → Git, `--branch-lines` is the documented path. A decode without it still encodes
  (the whole-tree form) where every atom is reachable, and the guide says what that form is.

### The release plan, amended

- **Batch G5,** the Subversion decoder's `--branch-lines` (D-9), comes before G4.
- **G4's Subversion evidence** uses it: wordpress-core, plus a repository with a deleted branch.
- **0.8.0 is cut after G4,** as planned.

## Non-goals (0.8.0)

- **Not in this release:**
  - incremental or continuous synchronization;
  - pushing to a hosting service;
  - `git-svn` metadata (`git-svn-id`);
  - submodules and LFS;
  - SHA-256 object-format repositories. `--object-format` is a later, separate decision.
- **Git → Git** is a test, not a promised migration path.

## Threat model

`brygge-03` is **updated**: there is a new output path and a new subprocess.
- **TB-8:** brygge → `git`, a trusted external tool (C-2b), with a fixed environment;
- **C-7:** atomic creation of the repository directory;
- **C-1g:** the encoder's output is brygge's own, labelled by D-2's trailers, and checked by D-6.

## Release plan for 0.8.0 ("Git as a target")

- **Batch D:** `faster-hex` 0.10.1. This is a lockfile-only change: it clears RUSTSEC-2026-0306, and the
  threat-model note is corrected.
- **Batch G1:** the crate, `encode git`, D-2, D-3 and D-7.
- **Batch G2:** D-6 and `verify --against-target git`.
- **Batch G3:** D-4. It runs only if OQ-2 is accepted.
- **Batch G4:** the guide "Migrating to Git", the evidence of D-8, and the cut.

## Open questions for the owner

- **OQ-1 — The encode report inside the repository too?**
  - **Recommended: no.** It goes beside the repository, and the trailers point to the artifact;
  - **the cost of yes:** a ref such as `refs/brygge/report` is not cloned by default either, so it adds a place to
    look without making the record harder to lose.
- **OQ-2 — The supported authors map (D-4) in 0.8.0?**
  - **Recommended: yes.** Without it, every Subversion and CVS commit carries `name <>`, and a user must write a
    transform plugin first. With it, a migration is two commands: `transform authors`, then `encode git`;
  - **the cost:** one more batch (G3).
- **OQ-3 — A bare repository?**
  - **Recommended: yes.** A migration result is cloned or pushed, not edited in place, and a checkout of a large
    history costs time and disk for nothing;
  - the guide shows `git clone <dir>`.

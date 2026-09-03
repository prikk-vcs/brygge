# Handoff — `brygge-decode-git` implementation (RFC 004)

**Realizes:** accepted **RFC 004** (Git decoder), under **RFC 009** (dependency policy) and against the
**brygge-ir** core (RFCs 001/002/003). Read RFC 004's decisions D-1…D-7 first; this handoff turns them
into a build.
**Status.** Inherits Accepted; gix adoption owner-approved with the security review in this folder. Build
against it now.
**Scope (increment 1 = ROADMAP M1, 0.1.0):** `brygge decode git` as a library — read a local Git object
database, produce a `brygge_ir::Ir` with the decode fidelity computable from it, entirely *Stated* except
opt-in marked-*Derived* rename hints. Plus the RFC 009 D-7 isolation test.
**Out (queued):** the `brygge verify --against-source` CLI (RFC 004 D-7 defines *what* it checks; the CLI
wiring is a later increment), rename-inference tuning (D-3/OQ-A — ships **off**), large-repo streaming
(OQ-D), and the full CLI command surface.

## 1. Crate & layout (RFC 009 D-1)

`crates/brygge-decode-git/` — the **only** crate linking `gix` (already wired: `gix = { workspace = true }`,
default features + `blob-diff` + `revision`, no network feature). `#![forbid(unsafe_code)]`,
`#![warn(missing_docs)]`, workspace lints. 2018 module style, tests as siblings (`foo.rs` + `foo/tests.rs`).

Modules:
- `lib.rs` — crate docs, re-exports, the `Error` type, `decode(path, &Options) -> Result<Ir>`.
- `open.rs` — open the repo with the **locked-down configuration** (D-6 / RFC 009 D-4).
- `walk.rs` — enumerate commits in **parent-first** topological order (see §3).
- `atoms.rs` — commit → `AtomDraft` (metadata, source identity, signature, status).
- `diff.rs` — tree diff (parent→commit) → `Vec<PathOp>`, blobs into the content store.
- `refs.rs` — branches + tags → `RefRecord`s; ref-namespace policy (D-2/D-5, OQ-B).
- `floor.rs` — the floor policy + the refusal mechanism (D-4).
- `loss.rs` — the representation-class `LossBoundary` (D-5).
- `options.rs` — `Options` (rename detection off by default; floor policy; recorded into provenance params).

## 2. The settled decisions (build to these; don't re-litigate)

- **Everything is `Stated`** except a rename hint the operator asked for, which is
  `Derived(InferredRename)` and sits **beside** the literal delete+create, never replacing it (D-2/D-3).
- **The Git SHA is never an identity** — it goes into `SourceIdentity.atom_id` opaquely; the IR's `AtomId`
  is brygge-ir's own SHA-256 over canonical bytes (RFC 001 D-4). Same for signatures → `.signatures`,
  shown Unverifiable (D-2/SRC-G3).
- **Refuse, never approximate**, below the floor — a named reason + the floor outcome (D-4/FA-3).
- **No source-provided code, no network, no ambient config** (D-6/RFC 009 D-4).
- **Byte-deterministic** for the same repo + brygge version + options; physical packing is irrelevant
  (D-6/VF-1).

## 3. The mapping spec (gix → brygge-ir)

Build with `brygge_ir::builder::IrBuilder`. **Parent-first ordering is mandatory**: an atom's `AtomId` is
computed from its parents' `AtomId`s, so a commit's parents must be added before it. Collect the commit
set, order it parent-first (a Git-SHA → `AtomId` map threaded as you go), then per commit:

- **Commit → `AtomDraft`** with `status = Stated`:
  - `parents`: look each Git parent SHA up in the SHA→`AtomId` map (all parents already added).
  - `ops`: from the tree diff (below).
  - `rename_hints`: empty unless `Options.detect_renames` is on (then one `Derived(InferredRename)` per
    detected pair, params = threshold + mode; **never** remove the corresponding Delete/Add).
  - `metadata: MetadataClaims` — author/committer (`name`,`email`), message, author_time & commit_time as
    epoch seconds (`i64`). Claims, never verified (PR-3).
  - `source: SourceIdentity { kind: Git, repo_id, atom_id: <commit SHA bytes>, signatures }` — `repo_id`
    = the lexicographically smallest **root-commit** SHA bytes (a content-stable repo fingerprint, so it
    is deterministic — do **not** use a path or UUID). `signatures` = the commit's GPG signature bytes if
    any (extracted from the raw commit header), else empty.
  - Record the returned `AtomId` in the SHA→`AtomId` map.
- **Tree diff (first-parent for merges) → `Vec<PathOp>`**, all `Stated` (D-2), via gix `blob-diff`:
  - added path → `Add { path, blob, mode, status: Stated }`;
  - content/mode change → `Modify { .. }`; removed → `Delete { path, status: Stated }`.
  - `blob` = `builder.add_blob(<raw blob bytes>)` — **raw**, no smudge/EOL filtering (D-6).
  - `mode` = the Git entry mode (`100644`/`100755`/`120000`). A **`160000` gitlink is a submodule →
    floor-refuse** (D-4). A root commit (no parents) diffs against the empty tree → all `Add`s.
- **Refs → `RefRecord`** (all `Stated`): local branches → `Branch`; tags → `Tag` (annotated tag's object,
  message, and signature preserved in the ref's `source`). `target` = the `AtomId` the ref's commit maps
  to. Emit only after the target atom is added (the builder validates this). Namespace policy per §4.
- **Provenance** (`ImportProvenance`): `brygge_version` = brygge's version, `decoder` = `"brygge-decode-git"`,
  `decoder_version` = `decoder_version()`, `params` = the `Options` rendered as sorted key→value strings
  (PR-5/CF-01), `import_time` = `None` for M1 (provenance-only if ever set — RFC 003 D-4/ID-4).

## 4. Ref namespaces, floor, loss

- **Carried:** `refs/heads/*` (Branch), `refs/tags/*` (Tag, lightweight & annotated).
- **Floor-refused (D-4, owner-ratified):** submodules (gitlink `160000`), `refs/replace/*` + grafts
  (`info/grafts`), shallow (`shallow` file present). Octopus: **carry all parents** for M1 (the parent
  ceiling N is prikk/OQ-2, unset — the encoder's floor holds it), so decode refuses no merge on count.
- **Dropped-with-record (D-5, representation):** packfile/delta layout & physical object store, index &
  working tree, reflogs. Ref namespaces `refs/remotes/*`, `FETCH_HEAD`, stash → dropped-with-record
  (OQ-B; `refs/notes/*` also dropped-with-record for M1). Each drop is one `DropRecord`
  (`class = Representation`, `what`, `reason`) in the `LossBoundary` — nothing silent (PR-9).
- **Refusal mechanism:** a distinct `Error::FloorRefusal { feature, reason }` (add to the crate's error
  enum) or a decode outcome carrying refusals; the caller maps it to the CL-08 "hit the floor" class.
  A refusal names the feature and why (FA-3).

## 5. The locked-down open (D-6 / RFC 009 D-4)

Open the repo read-only and **disable everything that executes source code or reads ambient config**: no
hooks, no filter/smudge/`.gitattributes` processing (hash blob bytes raw), no submodule fetch, no network,
no global/system config, no credentials. Use gix's object-database access; do not shell out. This one
configuration serves determinism *and* the untrusted-input guarantee.

## 6. Build order (each green before the next)

1. `open.rs` — open a fixture repo with the locked-down config; unit-test it opens and reads HEAD.
2. `walk.rs` — parent-first ordering over a fixture; test the order is a valid topo order and stable.
3. `diff.rs` — tree diff → ops on a fixture with add/modify/delete/mode-change; test blob bytes & modes.
4. `atoms.rs` + assemble a linear history → `Ir`; test atom count, ancestry, metadata claims.
5. `refs.rs` — branches + tags; `loss.rs`; `floor.rs` refusals.
6. Determinism + isolation tests (§7).

## 7. Tests & gates (acceptance checklist)

Create fixtures by driving `gix` (or a committed tiny bare repo) in a temp dir — **no network**. Cover:
- **Faithful core:** a small repo (linear + one branch + one merge + a tag) decodes to an `Ir` whose
  atoms, ancestry, ops, and metadata match; every record is `Stated`; the fidelity report (from
  `brygge_ir::honesty::summary`) shows **zero derived** with renames off.
- **Signature opaque:** a signed commit's signature is preserved in `SourceIdentity.signatures` and the
  atom is not marked verified anywhere (SRC-G3).
- **Determinism (VF-1):** decode twice → identical artifact bytes (`brygge_ir::to_bytes`). Bonus:
  `git repack`/`gc` the fixture, decode again → **same bytes** (pack-layout independence, D-6).
- **Floor (FA-3):** a fixture with a submodule (gitlink) / a grafts file / a shallow marker → a named
  refusal, not an approximation.
- **Loss:** the `LossBoundary` lists the representation drops; nothing silently omitted.
- **Rename off by default:** a delete+add that looks like a rename yields **no** rename hint unless
  `Options.detect_renames` is set; when set, a `Derived(InferredRename)` hint appears *beside* the
  literal ops, carrying its parameters.
- **Isolation (RFC 009 D-7):** a test (in `brygge-ir` or a dedicated crate) builds/serializes/`from_bytes`
  round-trips an IR with **no `brygge-decode-git` dependency present** — proving the IR + internal path
  need no decoder. (The decode→IR fixture may live behind a `#[cfg(test)]` dev-dependency, never a
  normal dep of the core.)

**Gates (all must pass, `--locked`):** `cargo fmt --check`; `cargo clippy --workspace --all-targets
--all-features --locked -D warnings`; `cargo test --workspace --locked`; `cargo deny check`; `cargo audit`.

## 8. Acceptance criteria

- `decode()` produces a byte-deterministic, all-*Stated* IR from a Git repo; renames off by default and,
  when on, marked `Derived` beside the literal ops (D-2/D-3).
- The four floor features are refused with named reasons (D-4); the loss boundary records the
  representation drops (D-5); nothing is silently omitted (PR-9).
- The Git SHA and signatures are preserved opaquely; nothing reads as target-verified (SRC-G3/NG-3).
- gix is linked only here; the core builds and the isolation test passes with no decoder present
  (RFC 009 D-1/D-7).
- All gates green.

## 9. Queued next (not this increment)

- `brygge verify --against-source` re-reading the source via this crate (RFC 004 D-7, VF-2), and the
  `brygge decode`/`inspect`/`summary` CLI surface (external design CL-01/02/05, CL-08 exit classes).
- Rename-detection algorithm + default threshold (OQ-A); ref-namespace confirmations (OQ-B); large-repo
  streaming (OQ-D).
- Then RFC 005 (Mercurial) toward M2, which stresses the IR cross-source (RFC 003 D-7 freeze precondition).

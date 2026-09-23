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

**Every object is verified**: a commit, tree, blob, or tag is re-hashed
against its claimed id before its content is trusted, and history is walked with the on-disk
`commit-graph` cache disabled, taking every commit's parents from its own verified content — so a
crafted or corrupt repository cannot present content under an id it does not hash to, or use a stale or
crafted `commit-graph` to drop or fabricate a parent. A repository declaring
`extensions.objectFormat = sha256` is refused (`sha256-object-format`): this build's `gix` dependency
only reads SHA-1 object stores. An annotated tag's tagger/time/message is carried as its ref's
`annotation`, and every other commit header (`mergetag`, and anything this decoder does not otherwise
model) is carried as a labelled `Extra`; `gpgsig`/`gpgsig-sha256` are carried as labelled `Signature`s.

## Usage

```rust,no_run
let ir = brygge_decode_git::decode(
    std::path::Path::new("/path/to/repo/.git"),
    &brygge_decode_git::Options::default(),
)?;
println!("{}", brygge_ir::honesty::summary(&ir).render_human());
# Ok::<(), brygge_decode_git::Error>(())
```

Or run the example against any repository:

```
cargo run -p brygge-decode-git --example decode_repo -- /path/to/repo [--infer-renames]
```

## Ceilings

Every ceiling is checked before the memory it protects is allocated, and a hit is a typed refusal (exit
20), never an OOM, a stack overflow, or a hang (RFC 010 D-4):

| Ceiling | Default | Checked |
|---|---|---|
| Blob size | 1 GiB | from the object header (no inflation) before the blob is read |
| Commit count | 10,000,000 commits | during the walk, before any atom is built |
| Path length | 4,096 bytes | per path, while walking trees |
| Tree nesting depth | 256 levels | per level, while walking trees (`walk_tree` is iterative — an explicit stack — so an attacker-chosen depth cannot overflow the process stack) |
| Tag chain length | 32 tags | while peeling a tag-of-a-tag ref to its final target |

## Floor

A repository or object hitting one of these is **refused** (exit 20), never approximated. The identifier
is what appears in the refusal and in every artifact's `params["floor"]`; it is the same identifier for
the same concept in every decoder.

| Identifier | What is refused | What you can do |
|---|---|---|
| `submodule` | a tree entry that is a submodule (gitlink) pointing outside the repository | decode the submodule's own repository separately; brygge does not follow the gitlink |
| `replace-ref` | any `refs/replace/*` ref, which rewrites the object graph a reader would see | delete the replace ref if it is unwanted (`git replace -d <object>`), or, in a **copy** of the repository, make the rewrite permanent (`git filter-repo`) and then delete it |
| `grafts` | an `info/grafts` file, which rewrites ancestry | delete `info/grafts` if it is unwanted, or, in a **copy** of the repository, make the graft permanent (`git filter-repo`) |
| `shallow-clone` | a shallow clone (a `shallow` file): truncated history that would look whole | in a **copy** of the repository, `git fetch --unshallow`, then decode the copy |
| `object-alternates` | a repository borrowing objects from another store (`objects/info/alternates` or `http-alternates`) | in a **copy** of the repository, `git repack -a -d` to copy borrowed objects in, then remove the alternates file from the copy |
| `redirected-git-directory` | a `.git` file (`gitdir:` redirect, linked worktree or submodule checkout), a git directory containing `commondir`, or a symlinked `.git`, `objects`, `objects/info`, `objects/pack` or pack entry | point brygge at the repository's main git directory (`git rev-parse --git-common-dir`), or replace the symlink with the real directory |
| `non-utf8-path` | a path in a tree that is not valid UTF-8 (invalid bytes are shown as `\xNN`) | in a **copy** of the repository, rewrite the history to rename the path (for example with `git filter-repo`), then decode the copy |
| `non-utf8-ref-name` | a branch or tag name that is not valid UTF-8 | in a **copy** of the repository, rename the ref, then decode the copy |
| `non-utf8-commit-header-name` | a commit carrying an extra header whose name is not valid UTF-8 | none without rewriting the commit (in a **copy** of the repository); report the repository if it is genuine |
| `sha256-object-format` | a repository with `extensions.objectFormat = sha256`; this build reads SHA-1 object stores only | convert to a SHA-1 repository, or wait for a release that reads SHA-256. There is no in-place conversion; for example, `git fast-export --all` from the repository into a new SHA-1 repository (`git init --object-format=sha1`, then `git fast-import`) works, and gives every commit a new SHA-1 id |

Status: **Built (ROADMAP M1); not yet released** — commits→atoms, tree-snapshot diff→literal ops,
opaque SHA/signature, branches+tags, the floor above (refused, never approximated), the representation loss boundary, byte-deterministic (pack-independent) output,
`verify --against-source`, and the full CLI surface. Rename inference is off by default and, when on,
marked *Derived* beside the literal ops. A **symbolic ref** (e.g. `refs/remotes/origin/HEAD`, which an
ordinary `git clone` always creates) is not carried as a ref in any namespace — the IR has no alias
concept — and is recorded as a `symbolic refs (N)` drop; its target, if any, is a distinct ref this same
decode carries or drops on its own merits. An unparseable author/committer time
becomes an absent claim, never fabricated as `0` or salvaged from a malformed token. Built
against the handoffs and the gix security review in `rfcs/handoffs/004-git-decoder/`.

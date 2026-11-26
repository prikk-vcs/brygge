# Importing a Git repository

```
brygge decode git <repository> --out <artifact> [--infer-renames]
```

brygge reads a local Git repository directly (pure Rust, no `git` binary) and carries its history as the
repository recorded it. Before the run it states: *"Git: content, history and messages are carried as the
source recorded them. Renames are inferred only with --infer-renames, and are then marked derived.
Authorship is Unverifiable."*

## Memory while decoding

brygge keeps file content on disk while decoding, not in memory: a large repository needs free disk
space of roughly its content's uncompressed size while decoding (the spill file, removed when brygge
exits), plus the artifact, which is several times smaller. `--spill-dir <DIR>` chooses where; the
default is `--out`'s own directory. A RAM-backed directory (`tmpfs`) moves the memory cost rather than
removing it. The option never changes the artifact. Compressing the artifact uses up to 8 threads and
about 100 MiB more memory than before (not growing with the repository), and makes decoding take about
1.5× as long.

**A ceiling on the size of one tree** (0.8.1): a repository where one commit's tree holds more than
10,000,000 path entries is refused, exit 20. This is checked as each tree is walked, before an entry is
stored — it cannot be reached by a real repository's own files, but it bounds a crafted tree whose
entries reuse one shared subtree: a tiny repository, stated in terms of real objects, can otherwise make
a flat path list many times larger than the objects themselves. The ceiling counts the entries this one
walk visits, not the number of distinct Git objects involved.

## What brygge carries

- Every commit reachable from a branch (`refs/heads/*`) or a tag (`refs/tags/*`), as a stated atom that
  keeps its commit id. Each file change is carried as the literal add, modify or delete between the commit
  and its first parent.
- The author and the committer, each with its own time and timezone offset. Messages are carried as
  bytes, and a commit's declared `encoding` is carried on its message.
- Signatures (`gpgsig`, `gpgsig-sha256`), preserved as bytes. A preserved signature verifies nothing in
  any target. Every other commit header (such as `mergetag`) is carried too, by name, in header order.
- Branches and tags as refs. An annotated tag also carries its tagger, time, message and signature.
- **Every object is checked against its id.** A commit, tree, blob or tag whose content does not hash to
  its id stops the decode, so a preserved commit id is always a true link to its content. History is
  walked from the commits themselves, never from the commit-graph cache.

## Submodules

A submodule (a gitlink, mode `160000`) is carried as exactly what Git records at that path: the path
itself, and the commit id it pins in another repository. brygge never reads, fetches, imports or
verifies that other repository — it is never opened, and a commit id no object in this repository has
is normal for a submodule, not an error.

Wherever one appears — `decode`, `inspect`, `encode git` — the report states:

> `submodules: P path(s) in the history, T at the branch and tag tips; their own repositories were not
> read, imported or verified`

`.gitmodules`, the file mapping each submodule path to a URL, is ordinary content: brygge carries it like
any other file, change for change. A submodule pointing at a commit its own repository no longer has is
carried as the source holds it — Git, not brygge, decides later whether that commit is still fetchable.

## What it does not carry

Each of these is recorded in the artifact with its count, never dropped silently.

**Representation.** These records describe how Git stores and works with history, not history itself. The
first three are always present.

- `packfile and delta layout, physical object store`
- `index and working tree`
- `reflogs`
- `remote-tracking refs (N)`, `notes refs (N)`, `stash refs (N)`, `other refs (N)`: refs outside
  `refs/heads/` and `refs/tags/`.
- `commits reachable only from dropped refs (N)`: for example stash commits, or remote-tracking commits
  never merged into a branch. They are **not imported**. If the count cannot be computed, the record
  reads `(count unavailable)`.
- `symbolic refs (N)`: an alias to another ref, such as `refs/remotes/origin/HEAD`. The target ref is
  carried on its own.

**Other.** These records mark source-stated data that was not carried.

- `refs to non-commit objects (N)`: a tag or branch naming a tree or blob.
- `nested tag objects not carried (N)`: in a tag of a tag, only the outer tag's annotation is carried.
- `unparseable author/committer/tagger times (N)` and `unparseable author/committer/tagger timezone
  offsets (N)`: the time, or just its offset, is left absent rather than guessed.
- `undecodable encoding headers (N)`: an `encoding` header that is not valid UTF-8.

A decode whose records are all representation records exits `0`. Any record in the second group gives
exit `10` (recorded loss).

## Options

- `--infer-renames` is off by default, so a default import is entirely as the source stated it.
  - With it on, a file deleted at exactly one path and re-added with **identical** content at exactly one
    other path, in the same commit, gets a rename hint.
  - The hint is marked **derived**, with `rename_algorithm = exact-content-move` and `rename_threshold =
    100`. It sits beside the literal delete and add, which stay.
  - Ambiguous cases (the same content at several paths) are left unmarked.

## Repositories brygge refuses

A refusal exits `20`, names the identifier below, and writes nothing.

| Identifier | Refused | What you can do |
|---|---|---|
| `replace-ref` | any `refs/replace/*` ref | `git replace -d <object>` if unwanted, or make it permanent (`git filter-repo`) |
| `grafts` | an `info/grafts` file | make it permanent (`git filter-repo`), or delete it if unwanted |
| `shallow-clone` | a shallow clone | `git fetch --unshallow`, then decode |
| `object-alternates` | objects borrowed from another store | in a copy, `git repack -a -d`, then remove the alternates file |
| `redirected-git-directory` | a `.git` file, a `commondir`, or a symlinked `.git`/`objects` path | point brygge at the main git directory (`git rev-parse --git-common-dir`) |
| `non-utf8-path` | a path that is not valid UTF-8 (shown as `\xNN`) | rename the path |
| `non-utf8-ref-name` | a branch or tag name that is not valid UTF-8 | rename the ref |
| `non-utf8-commit-header-name` | a commit header whose name is not valid UTF-8 | none without rewriting the commit |
| `sha256-object-format` | a SHA-256 repository | no in-place conversion exists; `git fast-export --all` into a new SHA-1 repository, or wait for a release that reads SHA-256 |

brygge also refuses (exit `20`) a repository beyond its ceilings rather than exhausting memory:
- a blob over 1 GiB, checked before the blob is read;
- more than 10,000,000 commits;
- a path over 4,096 bytes;
- a tree nested more than 256 levels deep;
- a chain of more than 32 tags.

## Checking an import

- `brygge verify <artifact>` runs the honesty and consistency checks that need nothing but the artifact.
- `brygge verify <artifact> --against-source <repository>` also decodes the repository again, with the
  options recorded in the artifact, and reports whether it **corresponds**. A mismatch exits `50`.
- `brygge inspect <artifact>` prints the fidelity report: what was carried, derived and dropped.

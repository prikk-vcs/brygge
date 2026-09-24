# Importing a Mercurial repository

brygge imports **what the repository would publish**: the changesets `hg clone` would share. Secret and
hidden (obsolete) changesets stay behind, and the report counts them. brygge reads the local store
directly (no `hg` binary is needed), and it never modifies anything it reads.

```
brygge decode hg <repository> --out <artifact>
```

`<repository>` is the working directory that contains `.hg` (or the `.hg` directory itself).

## What brygge carries

- **Every published changeset**, as a *stated* atom: its parents, its file operations, and its message,
  byte for byte.
  - **The user** is carried as the author, with the email split out when present. Mercurial records one
    user and one date, so there is no separate committer or commit time: brygge never invents one.
  - **The time** keeps its timezone offset.
- **Renames and copies that Mercurial recorded** (`hg mv`, `hg cp`), as stated copies. Each copy names the
  changeset its source really came from: the first parent, the second parent in a merge, or an earlier
  changeset. A copy is never placed on a guess.
- **Changeset extras** (every one except `branch`, which becomes the named branch), carried as labelled
  bytes in their stored order.
  - `close` with value `1` means the changeset closed its branch (`hg commit --close-branch`).
- **Refs:**
  - every **bookmark** that names a published changeset;
  - the **head of every named branch**, computed over the published changesets. A closed branch still
    has a head, and its `close` extra says it was closed.
- **Tags** travel as the `.hgtags` file in the history, as Mercurial stores them, not as refs.
- **Verified identity.** Every revision brygge reads (changeset, manifest, file) is re-hashed the way
  Mercurial hashes it, and a store whose content does not match its node is refused. The repository's
  identity is taken from its smallest published root, so a repository and its `hg clone` get the same
  identity.

## What it does not carry

Each of these is recorded in the artifact's loss boundary, never dropped silently. `inspect` shows them:

| Record | When |
|---|---|
| `revlog physical layout and delta chains` | always: storage detail; the logical revisions are all carried |
| `dirstate and working copy` | always: local state, not history |
| `phases (public/draft/secret)` | the repository has phase data |
| `obsolescence markers` | the repository has an obsstore; the markers are advisory and never become ancestry |
| `changesets not published: secret or archived (N)` | secret or archived changesets were left out |
| `hidden (obsolete) changesets (N)` | rewritten or pruned changesets were left out. A changeset that is both secret and obsolete is counted once, as not published |
| `copy sources not resolvable (N)` | a recorded copy whose source could not be placed on an imported changeset; the copy is omitted, and the file is still carried |
| `copy metadata on an existing path not carried (N)` | a copy recorded onto a path that already existed (`hg cp -f`); the file's change is carried, the copy relationship is not |
| `bookmarks naming unimported changesets (N)` | a bookmark names a secret, hidden or missing changeset |
| `unrepresentable timezone offsets (N)` | a stored offset that is not a whole number of minutes; the time is carried, the offset is not |

The first two are storage representation only. If anything else is recorded, `decode` exits `10`
(recorded loss) instead of `0`.

## Options

- `--infer-renames` does not apply to Mercurial: Mercurial records its renames itself, and brygge carries
  them as stated. Giving it is a usage error (exit `2`).
- `--reconstruct-refs` does not apply to Mercurial (it has real branches and bookmarks). Giving it is a
  usage error (exit `2`).

## Repositories brygge refuses

A refusal exits `20`, names what was refused, and writes nothing (an existing artifact at `--out` is left
untouched).

| Refused | Identifier | What you can do |
|---|---|---|
| subrepositories (`.hgsub`, `.hgsubstate`) | `subrepo` | decode each subrepository separately |
| the `largefiles` extension | `largefiles` | convert the large files to ordinary files (`hg lfconvert --to-normal <repository> <new copy>` writes a converted copy), then decode the copy |
| the `lfs` extension | `lfs` | in a copy of the repository, convert the tracked files back to ordinary files, then decode the copy |
| a censored revision | `censored-revision` | none: the content no longer exists; keep the source repository |
| a merge in progress (`.hg/merge/state2`) | `unfinished-merge` | finish the merge (`hg resolve`, then `hg commit`) or abort it (`hg merge --abort`), then decode again |
| a changeset extra whose key is not valid UTF-8 | `non-utf8-extra-key` | none without rewriting history; keep the source repository |
| a file path that is not valid UTF-8 (shown as `\xNN`) | `non-utf8-path` | rename the file in a copy of the repository |
| a narrow clone's ellipsis revision | `ellipsis-revision` | decode the full repository the narrow clone came from |
| an externally stored revision | `external-storage-revision` | as for `lfs` |
| a revision flag Mercurial does not define | `unknown-revision-flag` | none; Mercurial refuses it too |

These also exit `20`:
- **A format this build does not read:** a `.hg/requires` entry such as `revlogv2`, `treemanifest`, a
  narrow clone, or anything unrecognized; or an obsstore other than version 1.
- **A store without `fncache`** (Mercurial before 1.1, from 2008), whose file names are encoded differently
  (`store-without-fncache`). Clone it with a current Mercurial (`hg clone --pull <old> <new>` writes a
  current store), then decode the clone.
- **A size ceiling:** a revision over 1 GiB once decompressed, or an obsstore or a small metadata file
  (`phaseroots`, `bookmarks`, `localtags`) over 64 MiB.

A corrupt or crafted store (for example, a revision that does not match its node) is a read error, exit
`1`.

## Checking an import

- `brygge inspect <artifact>` prints the fidelity report: what was carried, derived, dropped and refused.
- `brygge verify <artifact>` checks the artifact's own honesty and integrity, with no source needed.
- `brygge verify <artifact> --against-source <repository>` decodes the repository again and confirms the
  artifact corresponds to it (`corresponds`). Authorship is always *Unverifiable*: brygge carries what the
  source says, and verifies no one's identity.

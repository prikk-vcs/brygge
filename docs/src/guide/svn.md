# Importing a Subversion repository

brygge reads Subversion history from one of two forms. It picks the form from what you give it:

- **A dumpfile** (a file): the output of `svnadmin dump` (with or without `--deltas`) or of `svnrdump dump`
  that you made yourself. This is the preferred form. A dumpfile is the canonical, deterministic input: decoding the same dumpfile always gives
  byte-identical output, and no external tool runs.
- **A live repository directory**: brygge runs a read-only `svnadmin dump <repo> --quiet` itself, with no
  shell and no network. This needs `svnadmin` on your `PATH`, and brygge records its version
  (`svnadmin --version --quiet`) in the artifact's provenance. Two `svnadmin` versions could frame a dump
  differently.

Either way, provenance records which form was used (`source_form`: `dumpfile` or `svnadmin-dump`).

```
svnadmin dump /path/to/repo > repo.dump          # once, on the machine that has the repository
svnrdump dump https://host/repo > repo.dump      # or, for a repository you can only reach over the network
brygge decode svn repo.dump --out repo.ir        # the preferred form
brygge decode svn /path/to/repo --out repo.ir    # or let brygge run svnadmin dump
```

## Memory while decoding

brygge keeps file content on disk while decoding, not in memory: a large repository needs free disk
space of roughly its content's uncompressed size while decoding (the spill file, removed when brygge
exits), plus the artifact, which is several times smaller. `--spill-dir <DIR>` chooses where; the
default is `--out`'s own directory. A RAM-backed directory (`tmpfs`) moves the memory cost rather than
removing it. The option never changes the artifact. Compressing the artifact uses up to 8 threads and
about 100 MiB more memory than before (not growing with the repository), and makes decoding take about
1.5× as long.

**The running tree shares unchanged directories between revisions (0.9).** A branch or tag copy no
longer clones the tree it copies: it inserts the source directory where the copy lands, and a later
change copies only the directories on its own path back down to the root. Holding a snapshot — the
previous revision's tree, or one `copyfrom` may still name later — now costs nothing beyond that one
handle, however large the tree is. Memory therefore follows **the size of the history** (how much ever
changed), not **the number of tags times the number of files** it last held, which is what made a
tag-heavy repository expensive before. For a history with many tags, `--branch-lines` keeps the
resulting artifact small too, on top of this: a tag's own creation costs no operations at all (a copy
that creates a root has no ops), only the edits a branch or tag line makes afterward.

**Two ceilings remain on what still costs real work, each refused, exit 20:**
- **`max_tree_entries`** (10,000,000): checked where a tree is **expanded into entries** — a directory
  copy's expansion (the stated layer still records one `Add` per copied file, as it always has), and
  `--branch-lines`'s own per-file walk when a new branch or tag line's first revision is built from a
  copy. Not checked on an ordinary single-file change: a shared tree's size costs nothing to hold, so
  only turning it into IR entries needs a ceiling.
- **`max_tree_nodes`** (50,000,000): the **cumulative** count of directory nodes and file entries the
  decoder has ever allocated across the whole decode — every directory actually copied or newly created,
  every new file. Never decreasing, so it bounds the total copy-on-write work a dump can cause, not any
  one snapshot's size. Measured against a real, tag-heavy repository (codeblocks: 6,221 revisions, 316
  copy-source revisions), this stays about 433× under the default.

`max_copy_expansion` (10,000,000, one directory copy's own growth) is unchanged. None of these three are
reachable by real history: a dump that copies a directory, and later restates files inside the copy as
copies of their own, repeated many times over, is what crosses them — not a real repository's own
history, however large.

## Dump forms

brygge reads all three dumps of one repository, and **gives the same artifact for each**, byte for byte:

- `svnadmin dump`: fulltext;
- `svnadmin dump --deltas`: text and properties as deltas against each node's previous state;
- `svnrdump dump <url>`: deltas as well, and no SHA-1 checksums, only MD5.

**Deltas are read.** A text delta is svndiff (version 0), applied to the node's base: the copy source for a node
that is copied, the path's current content for a change, and nothing for a new file. A property delta sets and
removes properties against the base's. brygge never runs `svnrdump` and never touches the network: run it
yourself and give brygge the file (this is the remedy for a URL source, below).

- **Checksums are checked.** Every checksum a dump states (`Text-content-md5`/`-sha1`, `Text-delta-base-*`,
  `Text-copy-source-*`) is checked against the text brygge rebuilt, on every node, fulltext nodes included; a
  mismatch is a read error naming the path and the header. This is a **consistency** check (it also checks
  brygge's own delta application), **not authenticity**: a dump is untrusted and can state any checksum it
  likes. Verify who made a dump by other means.
- **svndiff versions 1 (zlib) and 2 (lz4) are refused by name.** Re-dump with `svnadmin dump` (fulltext or
  `--deltas`) or with `svnrdump`, which write version 0.
- **An incremental delta dump cannot be decoded alone.** `svnadmin dump --deltas --incremental -r N:M` writes
  deltas against revisions that are not in the dump; brygge stops with a read error naming the path, and never
  guesses the base. Dump the whole history.
- **One node's text is at most 1 GiB**, and the text all deltas reconstruct is bounded by the dump ceiling (8 GiB):
  a small delta dump cannot expand without limit. Over either is refused (`ResourceLimit`).

## What brygge carries

- **Every revision**, in order, as one history entry each, with its file operations read literally from
  the dump.
  - A `replace` is carried as a **replace**, even when the new content is identical, because Subversion
    stated a new node.
  - A copy (`copyfrom`) is carried as a stated copy that names the revision it came from.
- **File content**, as the dump stores it. Stored text is the repository's normal form; keyword and
  end-of-line expansion are working-copy transforms and are not applied.
- **File modes:**
  - `svn:executable` makes a file executable;
  - `svn:special` makes it a symlink, whose content is the bare link target (the `link ` prefix Subversion
    stores is removed). A symlink whose content does not start with `link ` is a malformed dump and is
    rejected, not guessed.
  - The two are tracked separately, so a file can have both, and removing `svn:special` from it leaves it
    executable. Setting or clearing `svn:special` without changing the text follows the new flag (the content
    gains or loses the `link ` prefix).
- **Claims:**
  - `svn:author` is the author's name, with no email (Subversion records none);
  - `svn:log` is the message;
  - `svn:date` is the author time, in UTC.
  - Subversion states no separate committer or commit time, so none is invented.
- **Authorship is Unverifiable**: brygge carries what the dump says and verifies nothing about who wrote it.

## What it does not carry

Everything below is recorded in the artifact's loss boundary, never dropped silently:

| Recorded as | What it means |
|---|---|
| `SVN physical storage and dumpstream framing` | the dump's framing and the repository's storage layout; the logical revisions are kept |
| `svn:mergeinfo` | advisory merge tracking, often incomplete; it never becomes a merge parent |
| `svn:eol-style / svn:keywords / svn:ignore (working-copy hints)` | working-copy settings (also `svn:global-ignores`); the stored bytes are carried as they are |
| `custom (user-defined) properties` | any property brygge does not recognize; not carried in this version |
| `empty directories (N)` | directories that had no file under them after the revision that created them; the IR has no empty-directory entity |
| `unparseable svn:date (N)` | revisions whose `svn:date` did not parse; they carry no time rather than a wrong one |
| `deleted or moved branches/tags not represented (N)` | with `--reconstruct-refs` only; see below |

The framing, working-copy-hint and empty-directory records describe representation only. Any of the
others makes the decode exit `10` (recorded loss), unless a flag makes it exit `30` (below).

## Branches and tags

Subversion has no branches or tags, only directories that follow a convention. With `--reconstruct-refs`,
brygge reconstructs them from the standard layout:
- `trunk` is a branch;
- each directory directly under `branches/` is a branch;
- each directory directly under `tags/` is a tag.

Each ref points at the last revision that touched its directory. Every reconstructed ref is marked
**derived** (brygge's judgment, not something Subversion recorded), with the layout recorded beside it.

- **Only live refs.** A branch or tag directory that no longer has any file in the final revision
  (deleted, moved to a new name, or never filled) gets no ref. It is counted as
  `deleted or moved branches/tags not represented (N)`. A branch moved to a new name that follows the
  layout appears under the new name.
- **Tags are not guaranteed immutable.** An SVN tag is an ordinary directory, and commits can land in it
  after it is created. Each tag ref records this.
- **Convention violations exit `30`**, and are flagged in the artifact:
  - `trunk/branches/tags layout not found`: the repository does not follow the layout at all, and no ref
    is fabricated;
  - `paths outside the trunk/branches/tags layout (N)`: the layout was found, but N files in the final
    revision belong to no branch or tag.

Without `--reconstruct-refs`, no refs are produced, and the history is carried as the plain directory
tree it is.

## Layout

The standard `trunk`/`branches`/`tags` convention is the default, but not every repository follows it
by that name, or follows it at the repository's own root. Three options (RFC 020 D-3), each with
`--reconstruct-refs` or `--branch-lines`, name where each container actually is:

```
brygge decode svn repo --branch-lines --layout-trunk mainline
brygge decode svn repo --branch-lines --layout-branches devel/branches --layout-tags devel/tags
```

- **Each option changes only its own container.** `--layout-trunk mainline` alone still expects the
  standard `branches` and `tags` — an option you do not give is never silently removed, unlike
  `git svn`'s `-T`/`-b`/`-t` (see the mapping below).
- **`--layout-trunk .` means the whole repository is the trunk, with no separate branches or tags
  container at all.** Giving `--layout-branches` or `--layout-tags` alongside it is a usage error (exit
  `2`): there is nothing left for them to name. This is how a repository with **no** layout at all
  reaches Git (see "Limits in 0.8.0" in the Git-target guide): `--layout-trunk . --branch-lines` decodes
  it as one line, since a copy that creates a root has no ops of its own (RFC 019 D-9) — the whole
  history still carries, just without a reconstructed branch/tag split.
- **A path value** is a repository-relative directory, never the whole filesystem path and never
  starting with `/`: no leading `/`, no `..` or empty segment, and no trailing `/`. `.` is accepted only
  as the whole value of `--layout-trunk`.
- **Two containers may not collide** once the options you did not give are filled in with their
  standard names: the same path twice, or one inside another (`--layout-trunk branches/main` is refused
  with the standard `branches`, since it is inside that container), is a usage error.
- **A project in a subdirectory of a larger repository:**
  ```
  brygge decode svn repo --branch-lines \
      --layout-trunk proj-a/trunk --layout-branches proj-a/branches --layout-tags proj-a/tags
  ```
  decodes `proj-a` alone; every path under a sibling project (`proj-b/…`) is outside the layout, so it
  is flagged and recorded as dropped — never silently lost.
- **What is recorded.** `params["layout"]` carries whichever containers were actually used — the
  standard ones if none were given — so `verify --against-source` re-derives with the same policy a later
  reader would otherwise have to guess. The default label is unchanged from before this RFC
  (`trunk=trunk,branches=branches,tags=tags`), so every existing artifact and default decode stays
  byte-identical.
- **For `git svn` users:** the flags map directly, with one difference.

  | `git svn` | brygge |
  |---|---|
  | `-T <path>`/`--trunk=<path>` | `--layout-trunk <path>` |
  | `-b <path>`/`--branches=<path>` | `--layout-branches <path>` |
  | `-t <path>`/`--tags=<path>` | `--layout-tags <path>` |
  | `-s`/`--stdlayout` | no option — the standard layout is the default |

  **The one difference:** in `git svn`, giving `-T` alone means the repository has no branches at all —
  the options not given are dropped, not defaulted. In brygge, an option you do not give always keeps
  its standard name; nothing is ever silently removed.

## One line of history per branch (`--branch-lines`)

Without `--branch-lines`, every commit's tree is the *whole* repository — `trunk/`, `branches/x/`,
`tags/y/`, all under one root — because that is what a Subversion revision actually is: one state of
the whole tree. That is faithful, but it is not a usable migration: a branch's own history, compared
against its own files at the root of its tree, is what a Git user expects.

`--branch-lines` (RFC 019 Amendment A) implies `--reconstruct-refs`, and changes what an atom *is*:
instead of one atom per revision for the whole repository, there is one atom per revision **per branch
or tag it touches**, holding only that root's own files, at the root of its tree.

- **Parents.** A branch/tag's atoms chain to the previous atom on its own line. The line's first atom,
  when the branch was created by copying another branch or tag (`svn cp <src> <new-branch>`), has that
  source's own atom (at the copied revision) as its parent — so the new line starts from the copied
  content, not from nothing. A branch created with no copy (a plain `mkdir`) starts a line with no
  parent.
- **Every line gets a ref**, live or not:
  - a **live** line (the branch/tag still exists in the final revision) gets the ordinary ref a branch
    or tag always gets;
  - a **dead** line (an earlier life of a branch that was deleted and recreated, or a branch deleted and
    never recreated) gets its own ref too, named `<branch-or-tag>@r<its last revision>` (for example
    `branches/foo@r812`) — so no reconstructed history is silently lost to Git's garbage collector.
    `deleted or moved branches/tags not represented (N)` therefore never fires under `--branch-lines`:
    every line, live or dead, has a ref.
- **A file copied from one branch to another** is carried as an addition with a copy record naming the
  source branch's own atom, not a separate unrelated history.
- **A revision that touches no branch or tag** (revision 0 is the usual case) gives no atom at all; it
  is counted as `revisions with no branch change (N)`.
- **Paths outside every branch/tag** (an explicit flag already covers the layout as a whole) are also
  recorded one at a time, naming the revision and the path, since they belong to no line.
- **A branch or tag copied from a path outside every root** (not from another branch or tag) starts a
  fresh line, with no parent, and its first atom is exactly the copied content — counted in the same
  convention-violation flag as "a root created from outside the layout (N)".
- **A repository with no `trunk`/`branches`/`tags` layout at all** gets no refs from the standard
  convention, and `--branch-lines` has nothing to build lines from — unless `--layout-trunk .` is given
  (see "Layout" above), which makes the whole repository the one trunk line.

## Repositories brygge refuses

A refusal exits `20` and says why; nothing is written.

| Refused | What you can do |
|---|---|
| `svn-externals`: any node with an `svn:externals` property, which reaches into other repositories | in a **copy** of the repository, remove the property, or export the referenced content into the tree; then dump the copy again |
| `remote-source`: a URL instead of a local repository | make a local copy with `svnrdump dump <url> > repo.dump`, run yourself, and give brygge the dumpfile |
| a dump whose text deltas are **svndiff version 1 or 2** (compressed) | dump again with `svnadmin dump` (fulltext or `--deltas`) or with `svnrdump` |
| a dump format version other than 1–3 | dump with a current `svnadmin` |
| a dump over 8 GiB, or one node's text over 1 GiB | refused before it is read into memory (or allocated); larger dumps are not supported (streaming is deferred by measurement, RFC 010) |

An **incremental delta dump** (a delta against a revision the dump does not hold) is a read error, not a
refusal of a feature: exit `1`, naming the path (see "Dump forms").

**A non-UTF-8 path is a malformed dump**, not a repository shape. A dumpstream's paths are UTF-8 by the
format's own definition. brygge stops with a read error (exit `1`), showing the offending bytes as `\xNN`.

## Checking an import

`brygge verify repo.ir` checks the artifact on its own. `brygge verify repo.ir --against-source <source>`
also decodes the source again and compares the two:

- **Give the same form the artifact was made from.** An artifact made from a dumpfile is checked against
  a dumpfile, and one made from a repository against a repository. Otherwise the comparison is
  `not-checked` (`the artifact was made from a <form>; verify against the same form`, where `<form>`
  is `dumpfile` or `svnadmin-dump`), the verdict is
  `incomplete`, and the exit is `1`.
- **A different `svnadmin` version is noted, not failed.** If the history is identical, verify passes and
  says `svnadmin versions differ (<a> vs <b>); the history is identical`. If the history differs, the
  failure detail names both versions, which is often the reason.

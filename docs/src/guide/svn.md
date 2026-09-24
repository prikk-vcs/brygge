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

## Repositories brygge refuses

A refusal exits `20` and says why; nothing is written.

| Refused | What you can do |
|---|---|
| `svn-externals`: any node with an `svn:externals` property, which reaches into other repositories | in a **copy** of the repository, remove the property, or export the referenced content into the tree; then dump the copy again |
| `remote-source`: a URL instead of a local repository | make a local copy with `svnrdump dump <url> > repo.dump`, run yourself, and give brygge the dumpfile |
| a dump whose text deltas are **svndiff version 1 or 2** (compressed) | dump again with `svnadmin dump` (fulltext or `--deltas`) or with `svnrdump` |
| a dump format version other than 1–3 | dump with a current `svnadmin` |
| a dump over 8 GiB, or one node's text over 1 GiB | refused before it is read into memory (or allocated); larger dumps are planned for a later release |

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

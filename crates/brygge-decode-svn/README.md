# brygge-decode-svn

brygge's **Subversion source decoder** (RFC 006, milestone M3). Reads an SVN history from a **dumpstream**
— either a user-supplied dumpfile or a read-only local `svnadmin dump` brygge invokes — and produces a
[`brygge_ir::Ir`].

- **Tier D (RFC 006 D-1):** a pure-Rust dumpstream parser. **No linked SVN library, no FFI, no network.**
  The only external touch is running `svnadmin dump` as a producer subprocess (skipped entirely when a
  dumpfile is supplied). `svnadmin dump` emits a uniform stream from FSFS or BDB, so the on-disk backend is
  never touched.
- **What it carries:** a **`Stated`** linear revision spine; **`Stated`** copies (`copyfrom` →
  `CopyRecord`, with the correct source revision resolved as `from_atom`); symlinks and exec bits; an
  **opt-in, `Derived`** branch/tag layer reconstructed by layout convention (off by default).
- **What it refuses (the floor, see the table below):** `svn:externals`, URL/remote sources, unknown
  dump-format versions, and svndiff versions 1 and 2 (compressed; see below). **Delta dumps are read**
  (`svnadmin dump --deltas`, and every `svnrdump dump`: svndiff version 0 and property deltas), and every
  checksum a dump states is checked against the text rebuilt from it (a consistency check, not
  authenticity). A convention-violating layout is **imported and flagged** (the CLI exits 30), not refused.
- **What it drops-with-record:** `svn:mergeinfo` (advisory, never a merge parent), `svn:eol-style` /
  `svn:keywords` (the stored normal-form bytes are carried), custom properties, empty directories.

The only crate that reads SVN (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none of it. It
reads untrusted input, so every parser is bounds-checked and panic-free. See
`rfcs/handoffs/006-subversion-decoder/` for the implementation handoff and the security review.

## Floor

What brygge refuses rather than approximates. Each refusal exits 20 and names its feature by a stable
identifier (lowercase, kebab-case — the same identifier means the same thing in every decoder); the
identifiers are also recorded in every artifact's provenance as `params["floor"]`.

| Identifier | What is refused | What you can do |
|---|---|---|
| `svn-externals` | any node carrying an `svn:externals` property, because it references other repositories | in a **copy** of the repository (for example, load a dump of it into a scratch repository), remove or replace the property (or export the referenced content into the tree), then dump the copy |
| `remote-source` | a URL or other remote source | make a local copy of the repository (or `svnadmin dump` it yourself) and give brygge that |

Two related refusals are not floor features but dump-format errors, also exit 20: an **unknown dump-format
version**, and a dump whose text deltas are **svndiff version 1 (zlib) or 2 (lz4)** (re-dump with `svnadmin dump`,
fulltext or `--deltas`, or with `svnrdump`, which write version 0). A delta dump made by `svnadmin dump --deltas
--incremental` cannot be decoded alone: a delta whose base is not in the dump is a read error (exit 1) naming the
path.

**Non-UTF-8 paths are a read error, not a floor refusal.** A dumpstream's paths are UTF-8 by the format's
own definition, so a path that is not is a *malformed dump*, not a property of your repository's shape (Git
and CVS repositories, by contrast, really can contain such paths, and brygge refuses them as `non-utf8-path`).
The error exits 1, and shows the offending bytes as `\xNN` rather than substituting characters.

## Ceilings

Checked before the memory it protects is allocated; a hit is a typed refusal (exit 20), never an OOM or a
hang (RFC 010 D-4):

| Ceiling | Default | Checked |
|---|---|---|
| Dumpstream size | 8 GiB | from a dumpfile's metadata, or bounded while reading `svnadmin dump`'s stdout — before either is fully read |
| `svnadmin dump` stderr | 64 KiB | while reading `svnadmin dump`'s stderr, concurrently with stdout so a full pipe on either stream cannot deadlock the child |

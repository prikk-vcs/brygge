# Handoff — a source path that does not exist says so (0.3.0 cut prep)

**Found by the architect** in review 041. It predates 0.3.0 and is small. **One review request**, then the
0.3.0 cut.

## Today

`brygge decode <kind> <path>` with a path that does not exist:

| Kind | Message today (exit 1) |
|---|---|
| svn | `cannot open SVN source: \`svnadmin dump <path>\` failed: svnadmin: E000002: Can't open file '<path>/format': No such file or directory`. **It runs `svnadmin` on a path that is not there.** |
| git | `cannot open Git repository at <path>: "<path>" does not appear to be a git repository` |
| hg | `<path> is not a Mercurial repository (no .hg)` |
| cvs | `<path> is not a directory (expected a local CVS repository of ,v files)` |

None of them says the plain fact. The SVN message points at a file inside the path, and blames `svnadmin`.

## The change

- **One rule, one wording, for all four kinds:** when the local source path does not exist (a
  `symlink_metadata` of `NotFound`), the error is an open error (exit 1, unchanged), with the message
  **`source not found: <path>`**, the path shown the way the CLI already shows paths.
- **Where:** in each decoder's source resolution, **after** that decoder's own URL and remote refusal, so
  that refusal keeps precedence and its text, and **before** anything else (for SVN, before `svnadmin` is
  run).
  - Keep the wording in one place: one message constructor, used by the four decoders, or one small shared
  helper. Say which in the request.
  - `decode svn svn://…` must still be the `remote-source` floor refusal with its `svnrdump` remedy.
- **SVN only:** a path that exists but is neither a file nor a directory (a FIFO, a socket, a device) is an
  open error: `SVN source is neither a dumpfile nor a repository directory: <path>`. Today it goes to
  `svnadmin` as a "repository", and a FIFO would be read as a dumpfile.
- **`verify --against-source <missing>`** re-decodes through the same code, so it gets the same wording in
  its `not-checked` detail. Check it, and do not add a second message.

## Tests

- **For each kind:** a missing path gives `source not found: <path>` and exit 1, and **no subprocess is
  started** for SVN (assert on the message, not on `svnadmin`'s).
- `decode svn svn://example.org/r` is still `remote-source`, with exit 20 and the same text as today.
- Unix: an SVN FIFO gives the neither-file-nor-directory error, and it does not block.
- `verify --against-source` with a missing source: `not-checked`, with the same wording.
- Every existing suite passes unchanged.

## CHANGELOG `[Unreleased]`, Fixed

"A source path that does not exist is reported as `source not found: <path>` for every source kind; `decode
svn` no longer runs `svnadmin` on it."

## Review request

`.git-exclude/review-request/033-source-not-found.md`. After approval, commit and push, and append CI. Leave
the architect's `docs/src/brygge-03-threat-model-v0.1.md` change unstaged; it goes in the cut commit.

# brygge-encode-git-example

**An unsupported example; not part of brygge.** It is never released, never published, and carries no
compatibility promise. It exists to show that an encoder can be built on `brygge-ir` alone (RFC 017
D-1/D-2, requirement `PU-3`) — see `cargo tree -p brygge-encode-git-example`: only `brygge-ir`'s own
dependency closure, no Git library at all.

```
brygge-encode-git-example --artifact <path> [--out <file>]
```

Reads a brygge IR artifact and writes a `git fast-import` stream, to `--out <file>` or stdout. Pipe the
result into `git fast-import` yourself, in a fresh or existing repository:

```
brygge-encode-git-example --artifact repo.ir | git -C some-repo fast-import
```

This is also the interface `brygge encode` runs a plugin through (RFC 017 D-3): the same binary,
unmodified, works invoked directly or as a `brygge encode` plugin.

brygge **verified the artifact** before this program ever saw it (its integrity digest, its canonical
form, every blob's content address). brygge verified nothing about what this program does with it — the
mapping below, and the honesty rules it follows, are this program's own, not brygge's. See
`docs/src/reference/writing-an-encoder.md` for the rules in general.

## The IR → Git mapping

| IR | Git |
|---|---|
| An atom, in canonical order | A commit, `mark`ed in that same order (parents always precede children) |
| `atom.parents[0]` | `from :<mark>` |
| `atom.parents[1..]` | one `merge :<mark>` each |
| A path op's mode `0o100644`/`0o100755`/`0o120000` | `M 100644`/`100755`/`120000` | 
| Any other mode | refused by name — this encoder never guesses a mode it does not recognize |
| `PathOp::Add`/`Modify`/`Replace` | `M <mode> :<blob-mark> <path>` (the blob is written once, by mark, the first time any op references it) |
| `PathOp::Delete` | `D <path>` |
| A rename | not a distinct Git command — the IR's own literal `Delete` + `Add` already reproduce it |
| `metadata.author`/`author_time` | `author <name> <email> <epoch> <offset>` |
| `metadata.committer`/`commit_time`, when distinct from the author | `committer <name> <email> <epoch> <offset>` |
| ...when absent (the IR's one-claim rule: no distinct committer means the source drew no distinction) | the author's own claim, supplied as the committer and marked `Encoded-supplied: committer, commit-time` |
| A missing time zone | supplied as `+0000`, marked `Encoded-supplied: …-time-zone` |
| A missing email | supplied as `<>` (empty, but present — Git requires the brackets), marked `Encoded-supplied: …` |
| `metadata.message` | the commit message, followed by one blank line and this program's own trailers (below) |
| `RefKind::Tag` with an `Annotation` | `tag <name>` (an annotated tag object), `tagger`/`data` from the annotation, supplying and marking whatever the annotation itself left absent |
| `RefKind::Tag` with no `Annotation` | `reset refs/tags/<name>` (a lightweight tag) |
| `RefKind::Branch`/`Bookmark`/`NamedBranch`/`Other` | `reset refs/heads/<name>` — all flattened to one namespace; this program does not invent a second namespace per source ref-kind (a known limitation) |

Every commit is written under one scratch ref (`refs/heads/brygge-encode-git-example-scratch`) so that an
interior commit never needs to know, up front, which real branch will eventually contain it — the DAG
shape comes entirely from the explicit `from`/`merge` marks above. Once every atom and ref is written,
the scratch ref is itself `reset` with no `from`, which removes it; it never appears in the result.

## Trailers

Every commit message gets, after one blank line:

- `Encoded-by: brygge-encode-git-example (an unsupported example; not part of brygge)` — always.
- `Encoded-supplied: <fields>` — only when this program invented a value the IR did not carry (see the
  mapping table above), naming exactly what.
- `Brygge-Derived: <scope> <kind> <params>` — one per `Derived` assertion the commit carries (the atom
  itself, or one of its path operations, scoped `atom` or `op:<path>`), naming the derivation's kind and
  its exact parameters. Never present on a fully `Stated` atom.
- `Brygge-Artifact: sha256:<digest>` — the artifact's own integrity digest (RFC 017 D-2's addition to
  `brygge_ir::artifact::Decoded`), so the resulting repository records exactly what it was built from.

## What this program does not do

- It writes no submodule/gitlink entry (mode `0o160000` is refused, by name, like any other mode it does
  not recognize) — the IR carries no submodule concept to write one from in the first place.
- It does not sanitize or transcode message/identity text beyond folding a literal newline to a space
  (Git's own line-oriented format cannot represent one); non-UTF-8 tag messages are not supported (`git
  fast-import` itself requires UTF-8 there).
- It does not validate ref names against Git's naming rules before writing them; an invalid name fails
  inside `git fast-import`, not inside this program.

None of the above is a brygge limitation — they are this example's own, left here rather than engineered
around, because the point of this program is to be a complete, readable reference, not a production
encoder.

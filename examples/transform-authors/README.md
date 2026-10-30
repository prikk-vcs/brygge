# brygge-transform-authors-example

**An unsupported example; not part of brygge.** It is never released, never published, and carries no
compatibility promise. It exists to show that a transform can be built on `brygge-ir` alone (RFC 018
D-6 §4) — see `cargo tree -p brygge-transform-authors-example`: only `brygge-ir`'s own dependency
closure, no third-party crate at all.

```
brygge-transform-authors-example --artifact <path> --out <file> --map <file>
```

Reads a brygge IR artifact and a mailmap-style map file, and writes a new artifact in which every atom
whose author or committer claim byte-matches one of the map's own entries gets that one claim replaced.
Everything else — every op, copy, ref, and every atom the map does not touch — passes through unedited.

This is also the interface `brygge transform` runs a plugin through (RFC 018 D-6 §3): the same binary,
unmodified, works invoked directly or as a `brygge transform authors-example` plugin.

brygge **verified the input artifact** before this program ever saw it, and will itself re-check this
program's own output (`brygge_ir::transform::check`) before accepting it — this program's own honesty is
checked, not assumed, on either path. See `docs/src/reference/writing-a-transform.md` for the rules a
transform must follow in general.

## The map file

One mapping per line:

```
Old Name <old@example.com> = New Name <new@example.com>
# a comment line; blank lines are skipped too
Another Old Name <old2@example.com> = New Name <new@example.com>
```

Both sides require a name and an angle-bracketed email. Matching against an atom's own author/committer
claim is always by **exact bytes** — never case- or whitespace-folded, since the IR's own claims are the
source's raw bytes (`PR-3`), and this program makes no guess about what the source might have meant by
two near-spellings.

## What the new transform record carries

The step this program makes records, in its own `params`:

- `map_sha256` — the map file's own content hash (hex), so a reader can confirm which map ran without
  trusting this program's say-so.
- `map_entries` — how many mapping lines the map had.

**Never the map file's own path.** The artifact alone must never reveal where on disk this program was
run; the hash is how a reviewer confirms which map file was used, by comparing it against the map file
itself, not by trusting a path that may not even resolve on another machine.

## What this program does not do

- It does not rewrite a `message`, a path, a blob, or any other claim — only `author` and `committer`.
  Every other field of every atom, op, copy, and ref it touches passes through byte-identical.
- It does not remove, add, or move an atom, op, copy, or ref — it has nothing to decide there, since the
  map names identities, not history shape.
- An atom neither author nor committer matches passes through with no `Transformed` mark at all — this
  is the honest result of a map with nothing to say about that atom, not a bug.
- A no-op run (a map that matches nothing in this artifact) still records one transform step with every
  count at zero: `brygge_ir::transform::Edit` always records the step it was asked to make, even when
  nothing in it actually changed anything (RFC 018 D-6's own "accept" lean on this case).

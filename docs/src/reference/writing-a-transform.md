# Writing a transform

A **transform** reads a brygge IR artifact and writes another one, derived from it (RFC 018 D-6):
rewriting authors, squashing atoms, dropping a path's history — anything that changes the IR while
staying the IR. It is a distinct thing from an **encoder** ([Writing an encoder](writing-an-encoder.md)):
an encoder's output leaves the IR for some other target; a transform's output is itself an IR artifact,
and must honestly record everything it changed so a later reader — human or `brygge transform`'s own
`check` — can see exactly what happened and what did not.

brygge supports no transform itself (it is not part of brygge's shipped surface) — this page is for
whoever writes one, on `brygge-ir` alone. The one real example,
`examples/transform-authors` (a mailmap-style author/committer rewrite), is unsupported and not part of
brygge, but it is complete and worth reading alongside this page — every rule below is one it actually
follows.

## 1. What a transform is, and is not, allowed to do

A transform may:

- change an atom's `author`, `author_time`, `committer`, `commit_time`, or `message`;
- change an existing op's blob or mode, or move it to a new path;
- remove an atom, an op, or a copy, recording why;
- remove a ref, or rename it, or change its annotation, recording why for a removal;
- do nothing at all — an honest no-op step is accepted (see §3).

A transform may **not**:

- invent an atom, an op, or a ref that has no counterpart in the input — only an input's own atoms, ops,
  and refs can ever appear in the output, under the matching rules in §3;
- change what the input already recorded as **dropped**, or any earlier transform step in the chain —
  provenance, flags, and every existing `LossBoundary` drop pass through byte-identical, always;
- retarget a ref to an atom that is not the *same* atom, by identity (`SourceIdentity`) — a ref may be
  removed, or renamed, but not pointed somewhere new; and
- change a copy record in place — a copy is removed (recorded) or it survives byte-identical (mapped
  through the atom matching); there is no partial edit of one.

## 2. `Edit`: the sanctioned way to build one

`brygge_ir::transform::Edit` computes every mark from the *difference* with the input itself — never
from what you say happened — so a transform built with it passes `check` (§3) by construction. This is
`examples/transform-authors`'s entire core:

```rust,ignore
let mut params = BTreeMap::new();
params.insert("map_sha256".to_string(), BlobId::of(map_bytes).to_hex());
params.insert("map_entries".to_string(), map.len().to_string());

let mut edit = Edit::begin(input, "authors-example", env!("CARGO_PKG_VERSION"), params);
for (in_atom, atom_edit) in input.ir.atoms.iter().zip(edit.atoms()) {
    if let Some(author) = &in_atom.metadata.author {
        if let Some(new) = remap(map, author) {
            atom_edit.set_author(Some(new.clone()));
        }
    }
    // ...committer, the same way...
}
let output_ir = edit.finish()?; // computes every mark, rebuilds, and runs `check` on its own result
```

`Edit::begin` takes the input, the step's own `name`/`version`/`params` (the new `TransformRecord`), and
hands back a working copy of every atom and ref. `atoms()`/`atom_mut(index)` give you an `AtomEdit` to
change in place (`set_author`, `set_op_blob`, `move_op`, `remove_op`, …); `ref_mut`/`rename_ref`/
`remove_ref` do the same for refs. Nothing here lets you add an atom or an op the input did not already
have — every setter edits something that already exists. `finish()` then:

1. diffs your working copy against the input to compute exactly what changed, atom by atom and ref by
   ref;
2. rebuilds through `IrBuilder`, so every id and canonical ordering is recomputed, never carried over
   from the input;
3. writes the new `TransformRecord` (`consumed` = the input artifact's own digest) and every drop your
   removals asked for; and
4. runs `check` (§3) on its own result, returning `Error::Invariant` if it ever refuses what `finish`
   just built — a bug in `Edit` itself, never in your own edits, since every edit above only ever
   produces a difference `check` already accepts.

**Removing an atom reparents its children**, in order, without duplicates: `Edit`'s own one policy
choice for what "removed" means for the DAG shape (RFC 018 D-6). If a surviving ref still targets a
removed atom, `finish` refuses — `remove_ref` it explicitly instead; there is no retarget to fall back
to.

## 3. `brygge_ir::transform::check`: the honesty rule

```rust,ignore
pub fn check(input: &Decoded, output: &Decoded, name: &str) -> Result<StepSummary, Violation>
```

Accepts `output` as an honest transform of `input`, performed by a plugin named `name`, only if:

1. **the chain** — `output`'s own `transforms` is `input`'s, unchanged, plus exactly one new record
   naming `name` and consuming `input`'s own digest;
2. **everything else passes through wholesale** — provenance, flags, and every earlier drop, unchanged;
3. **atoms are matched by `SourceIdentity`** — never by `AtomId` (which is content-derived and changes
   the moment anything upstream of an atom does) — and every unmatched input atom has exactly one `atom`
   drop recording it, no more, no fewer;
4. **ops are matched by path within each matched atom**, and **copies are matched by `to`** — the same
   invent/disappear/drop accounting as atoms, one level down;
5. **every surviving atom's parents equal the input's own, mapped through the matching** — a removed
   parent is substituted by its own parents (§2) — and `ChangedPart::Parents` is marked only when a real
   substitution happened, never merely because an id value changed;
6. **refs are matched by `(kind, name)`**, with the same marking/drop accounting, plus: a target is
   compared by the *atom's own identity*, not by id value, so `ChangedPart::Target` is marked only on a
   genuine retarget; and
7. **blobs are trivially content-addressed** — nothing to check beyond what the artifact reader already
   guarantees.

**An honest no-op step is accepted**: if nothing in the input actually changed, `check` still requires
the new transform record to exist (rule 1), but every atom, op, ref, and drop passes through unmarked.
`Edit::finish` always writes the step it was asked to make, even when every count in its own
`StepSummary` comes back zero — recording that a transform ran and found nothing to do is itself honest,
and distinct from the transform never having run at all.

`duplicate_source_identity(input)` — `check`'s own first rule, exposed separately — lets a caller refuse
an input with two atoms sharing one `SourceIdentity` before ever running a plugin on it: nothing
downstream could match atoms by it at all. `brygge transform`'s own CLI does exactly this.

## 4. The `Transform`-class drop grammar

A `LossClass::Transform` drop's `what` field follows a fixed grammar, checked at decode time
(`Error::Decode` on violation — normative in [IR artifact format](ir-artifact-format.md)):

| Form | Names |
|---|---|
| `atom <id>` | a removed atom, by its own (input-side) `AtomId`, 64 lowercase hex characters |
| `op <id> <path>` | a removed op, by its atom's input id and the op's own path |
| `copy <id> <to>` | a removed copy, by its atom's input id and the copy's own `to` |
| `ref <kind> <name>` | a removed ref, by kind (`branch`/`tag`/`bookmark`/`named-branch`/`other:<label>`) and name |

A single space separates each part; the path or name runs to the end of the line. Every `AtomId` here is
always the **input** artifact's own — a transform's drops describe what it removed from what it was
given, not anything about its own output's (different) ids.

## 5. The `params` rule

A transform's own `params` (on its `TransformRecord`) are evidence a reader can check *without trusting
the transform's own say-so* — a content hash, a count, a flag — never a path, a timestamp, or anything
else that only makes sense on the machine that ran it. `examples/transform-authors` records
`map_sha256` and `map_entries`, never the map file's own path: the artifact alone must never reveal
where on disk a transform ran, and a hash is how a reviewer confirms *which* map ran, by comparing it
against the map file itself.

## 6. Running as a `brygge transform` plugin

`brygge transform <name> <artifact> --out <file> [-- <plugin arguments>…]` runs your transform as an
external plugin (RFC 018 D-6 §3), found on `PATH` — the same convention `brygge encode` uses for an
encoder ([Writing an encoder](writing-an-encoder.md) §5), with one addition: your own output is written
to a file, and brygge checks it before accepting it.

- **The binary name is `brygge-transform-<name>`.** The same lookup rules as `encode`'s target: `PATH`
  only, in order, skipping empty and relative entries; `.exe` only on Windows, never `.bat`/`.cmd`. The
  same name may exist as both a `brygge-encode-<name>` and a `brygge-transform-<name>` plugin with no
  conflict — the two are looked up under different prefixes.
- **The name is `[a-z0-9][a-z0-9-]*`, at most 64 bytes.** Anything else is a usage error before any
  lookup happens.
- **The arguments you receive:** `brygge-transform-<name> --artifact <artifact> --out <tmp-file>
  [<plugin arguments>…]`. `<tmp-file>` is a private, already-reserved path — write your output there
  directly (truncate-and-write is fine; brygge reserved the name, not the writing). `<plugin arguments>`
  are exactly what the user wrote after `--`, verbatim, in order; brygge runs no shell.
- **Your stdin, stdout and stderr are the user's**, inherited and untouched.
- **brygge verifies the input artifact before it ever starts you**, exactly as `encode` does, then
  refuses an input with two atoms sharing one `SourceIdentity` (§3) before you run at all.
- **A banner brackets your run, on stderr**, the same shape `encode`'s is.
- **Your own exit code is passed straight through** on a non-zero exit or a signal — nothing of yours is
  trusted further, and the temp file is removed. On exit `0`, brygge reads what you wrote back, runs its
  own internal checks on it, then `brygge_ir::transform::check`; **any** failure there removes the temp
  file, prints why, and exits `50` — `--out` is never touched. Only once your output has passed every
  one of those does brygge rename the temp file over `--out`, exactly as `decode --out` behaves
  (silently, atomically overwriting whatever was already there), and print the one result line `check`
  itself computed.
- **`brygge transform --list [--format human|machine]`** shows every `brygge-transform-*` plugin found
  this way, the same shape `encode --list` does — see [Machine-readable output](machine-output.md).

## 7. What brygge checks, and what it cannot

brygge's own `check` verifies the **shape** of honesty: every change marked, every removal recorded,
nothing invented, nothing disappeared without a trace. It cannot verify that your transform did the
*right* thing — that a mailmap entry maps to the correct person, that squashing was the squash the user
actually wanted. That judgment is yours; `check` only ever refuses a transform that lied about what it
did, never one that did something unwise but told the truth about it.

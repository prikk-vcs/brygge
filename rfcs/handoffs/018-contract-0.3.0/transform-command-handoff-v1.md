# Handoff — RFC 018 D-6: `brygge transform`, the output check, and an editor for transform authors (batch P)

**Governing:** RFC 018 D-6 (OQ-3, ruled: yes, in 0.7.0), with two contract refinements below, recorded in RFC 018's
status. **After batch T** (review 102's T-1 and T-2 pushed and green). **On a topic branch,** `batch-p-transform`.
It spawns processes and reuses the per-OS lookup, so the Windows and macOS jobs must show the tests running, as for
batch E. It is the last batch before the 0.7.0 cut.

## 0. First steps

1. **Commit the architect's files on their own,** as "RFC 018: the batch P handoff; two contract refinements":
   - this file;
   - `rfcs/accepted/018-contract-0.3.0.md`.
2. **Report:** can any decoder give two atoms the same `SourceIdentity`?
   - Answer from the code;
   - and from the corpus artifacts in `/home/nabbisen/Desktop/prikk/corpus-scratch/`, and every test fixture.

   §3 depends on the answer: atoms are matched by it.

## 1. Two contract refinements (0.3.0 is unreleased)

The output check (§3) needs the contract to say exactly what changed and what went. Batch T left two gaps:

- **R-a — `parents` joins the atom vocabulary.** Removing an atom rewires its children's parents. Without a word for
  it, that change could not be marked. `ChangedPart::Parents`, rendered `parents`.
- **R-b — a `Transform` drop's `what` has a grammar, which the reader checks:**
  - `atom <input AtomId, 64 lowercase hex>`;
  - `op <input AtomId> <path>`;
  - `copy <input AtomId> <to>`;
  - `ref <kind> <name>`, where `<kind>` is `branch`, `tag`, `bookmark`, `named-branch` or `other:<label>`.

  - The single space is the separator. A path or a name runs to the end of the line.
  - **The `AtomId`s are the input artifact's:** the one whose digest the step's record cites.
  - `reason` stays free text.

  So every loss a transform records names exactly one thing, and a reader can count them. Other loss classes are
  unchanged.

Update `ir-artifact-format.md` (the vocabulary and the grammar), the decode-time checks, the tests and the worked
example. A grammar violation is `Decode`.

## 2. `brygge-ir`: `transform::check`, and `transform::Edit`

A new public module, `brygge_ir::transform`. It is the honesty rule as a function, usable by anyone holding both
artifacts, prikk included, and not only by the CLI.

### `pub fn check(input: &Decoded, output: &Decoded, name: &str) -> Result<StepSummary, Violation>`

`Violation` names the first difference, in words a user can act on: "atom … (source …): author changed but not
marked". Every rule below must hold, with *k* = `input.ir.transforms.len()`.

1. **The chain.**
   - `output.transforms` = the input's chain plus exactly one record;
   - the new record's `name == name`, and its `consumed == input.digest`.
2. **Unchanged wholesale:** `provenance`, `flags`, every earlier record, and every drop that has no step or an
   earlier step. **Every new drop** is a `Transform` drop at step *k*.
3. **Atoms,** matched by `SourceIdentity`:
   - every output atom matches exactly one input atom. One that matches none was invented, and is refused;
   - an input atom with no match must have an `atom <its id>` drop at *k*. Conversely, every `atom` drop names an
     input atom that has no match;
   - **an unmarked output atom** equals its input atom in every field. Its parents are the input's parents, mapped
     through the matching;
   - **a marked output atom** has the status `Transformed { step: k, changed, prior }`:
     - `prior` equals the input atom's status;
     - **`changed` equals exactly the set of parts that differ.** Naming a part that did not change is refused
       too, because a mark must be true in both directions;
     - `parents` differs when the mapped parent list differs.
4. **Ops,** inside each matched atom, matched by path:
   - an output op at a path the input atom has: unmarked means identical; marked means `Transformed` at *k*, with
     `prior` = the input op's status and `changed` exactly the differing parts among `blob` and `mode`;
   - **an input op whose path is gone:**
     - either it is named by an `op <input id> <path>` drop at *k*;
     - or it is one of the "moved" ones;
   - **an output op at a path the input atom lacks** must be `Transformed` at *k*, with `path` in `changed`.
   - **The number of moved output ops must equal the number of gone input ops not dropped.** Then no op is
     invented, and none disappears unrecorded. The atom's `changed` includes `ops` if any op differs.
5. **Copies,** inside each matched atom:
   - identical, or the atom's `changed` includes `copies`;
   - a copy that is gone has a `copy` drop.
6. **Refs,** matched by `(kind, name)`, with the same rules as ops: identical, or marked with exactly the
   differing parts; renamed refs are counted against gone ones; gone refs that are not renamed are named by
   `ref` drops. A ref's `target` maps through the atom matching.
7. **Blobs:** nothing to check beyond the ops (content-addressed).

`StepSummary` gives the counts (atoms, ops and refs changed, and the drops), for the CLI's last line.

- **Duplicate `SourceIdentity` values in the input:** `check` refuses with a `Violation` that says so.
- **If §0's answer is "a decoder can produce them,"** say which decoder, and stop. The architect then decides the
  matching key before you go on.

### `pub struct Edit`: the honest way to write a transform

- **Every author would otherwise repeat** what review 102's T-1 had to do by hand:
  - recompute `AtomId`s;
  - remap parents, copies' `from_atom` and ref targets;
  - mark exactly what changed;
  - write the drops in the grammar.
- **The API:**
  - `Edit::begin(input: &Decoded, name, version, params) -> Edit` holds the input and a working copy;
  - **methods:**
    - `atoms()` / `atom_mut(index)`: an `AtomEdit` with setters for the five claims and for ops (`set_op_blob`,
      `set_op_mode`, `move_op(from_path, to_path)`);
    - `remove_atom(index, reason)`, `remove_op(atom, path, reason)`, `remove_copy(atom, to, reason)`;
    - `ref_mut(kind, name)` / `remove_ref(kind, name, reason)` / `rename_ref`;
    - `add_blob(bytes) -> BlobId`;
  - **there is no way to add an atom, or an op without an input op.**
- **`finish(self) -> Result<Ir>`:**
  - computes every mark from the difference with the input, exactly as `check` would. `changed` comes from the
    difference, never from what the author said;
  - writes the record with `consumed = input.digest`, and the drops;
  - rebuilds through `IrBuilder`, so ids and canonical orders are recomputed;
  - **then runs `check(input, output)` itself,** and returns an `Invariant` error if it fails. So a transform built
    with `Edit` passes the CLI's check by construction.
- **Parents of a removed atom:** its children inherit the removed atom's parents, in order and without duplicates.
  Document this, because it is the only choice `Edit` makes for the author.

## 3. The CLI: `brygge transform`

- **The commands:**
  - `brygge transform <name> <artifact> --out <file> [--format human|machine] [-- <plugin arguments>…]`;
  - `brygge transform --list [--format …]`.
- **The lookup:** `plugin.rs` gains a `Kind` (`Encode` → `brygge-encode-`, `Transform` → `brygge-transform-`). Every
  rule is shared, with no copy: `PATH` only, absolute entries, the execute bit, `.exe` only on Windows, the name
  grammar, no shell. `--list` and shadowing work per kind. No transform name is reserved.
- **Refused before anything runs** (exit 2):
  - `--out` missing;
  - `--out` the same file as `<artifact>` (compared after canonicalizing);
  - an existing `--out`: treat it exactly as `decode --out` does, and say in the request what that is.
- **Then, in order:**
  1. **verify first,** with `verify`'s own exit code, as `encode` does;
  2. **check the input's `SourceIdentity` uniqueness** (exit 50 with the message, before the plugin runs);
  3. **the banner on stderr:** "transforming with the external plugin `brygge-transform-<name>` (`<path>`): not
     part of brygge; brygge checks its output against the input before writing `<out>`";
  4. **a temporary file** in `--out`'s directory, made the way the spill file is (`create_new`, 0600 on Unix,
     a unique name, starting with `.brygge-transform-`);
  5. **run** `brygge-transform-<name> --artifact <artifact> --out <temporary file> [<plugin arguments>…]`, with
     stdin, stdout and stderr inherited;
  6. **a non-zero exit** (or a signal): delete the temporary file, report "`brygge-transform-<name>` exited with
     <code>", and exit with the plugin's code (1 for a signal, named);
  7. **exit 0:** read the temporary file with `from_file`, run `verify`'s internal checks on it, then
     `transform::check`:
     - **any failure:** delete the file, print "the transform's output was refused: <violation>", and exit **50**.
       Nothing is written to `--out`;
     - **pass:** rename the temporary file to `--out` (the same directory, so the rename is atomic). Print one
       line: "`brygge-transform-<name>` exited with 0; output checked: step <k>, <a> atom(s), <o> op(s), <r>
       ref(s) changed, <d> dropped". Exit with the fidelity class `decode` would give for the output artifact.
- **The temporary file is removed on every path but the last,** including brygge's own error paths. Test each one.
- **`machine-output.md`:** a section for `transform` (the banner, the result line, the refusal), in the style of
  `encode`'s.

## 4. `examples/transform-authors`

- **The crate:** `brygge-transform-authors-example`, `publish = false`, depending on `brygge-ir` only.
- **What it does:** a mailmap-style author mapping, built with `Edit`.
- **The interface:** `--artifact <path> --out <path> --map <file>`. Each line of the map is
  `Old Name <old@email> = New Name <new@email>`, and identities are compared as bytes.
- **The record's `params`** hold what governs the output, not where it came from:
  - `map_sha256` (the map file's digest);
  - `map_entries`;
  - **never the map's path.** State this as a rule for transform authors.
- **Labelled** like encode-git: README, `--help`, and the record's version text say "an unsupported example; not
  part of brygge".
- **Never released.** The workspace's `examples/*` already covers it.

## 5. The guide: `docs/src/reference/writing-a-transform.md`

- **What it covers:**
  - what a transform is, and the rule: everything it changes is marked, everything it removes is recorded;
  - what it may not do (add an atom, add an op without an input op, change provenance or flags);
  - `Edit`, with the example's core;
  - the params rule;
  - running as a plugin: the name, the arguments, the exit codes, the banner, `.exe` only on Windows;
  - **what brygge checks** (§2's rules, summarized), and what it cannot (a transform run outside `brygge
    transform`; the stated residual).
- **What encoders and readers see:** link `writing-an-encoder.md`'s carrying rule.
- **Also:** add it to `SUMMARY.md`. The README gets one sentence: transforms are external, and brygge supports
  none of them.
- **The external design:** add a CL line for `transform`, worded like CL-03.

## 6. Proof

1. **The example as a plugin, through the CLI:**
   - put the example on a temporary `PATH`, and run `brygge transform authors-example <artifact> --out <file> --
     --map <map>` over a decoded fixture repository;
   - then `verify` the output (pass), and `verify --against-source` (`root-corresponds`);
   - `inspect` shows step 0 with its counts;
   - `encode git-example` gives `Brygge-Transformed:` trailers, and `git fast-import` accepts the result.
2. **`check`'s refusals, one test each,** built by writing a bad output by hand with `IrBuilder`:
   - an unmarked change to each claim;
   - a mark naming an unchanged part;
   - a missing part in `changed`;
   - a wrong `prior`;
   - an invented atom;
   - an invented op;
   - more moved ops than gone ones;
   - a gone op with no drop;
   - an `atom` drop for an atom still present;
   - changed parents unmarked;
   - a changed `provenance`;
   - a changed flag;
   - an edited earlier drop;
   - a wrong `consumed`;
   - a wrong name;
   - two new records;
   - a ref renamed without a mark;
   - duplicate source identities.
3. **`Edit`:** each method's result passes `check`, and `finish` refuses nothing it can produce. Also:
   - removing a middle atom gives its children the documented parents, marked `parents`;
   - property-style: a sequence of random edits on the richest fixture always passes `check`.
4. **The CLI's paths, with test plugins:**
   - the test plugin crate gains a transform mode: copy the input unchanged; or write a dishonest output; or exit
     3; or kill itself;
   - **assert, for each:** the exit code; nothing at `--out`; no temporary file left behind (list the directory);
   - **the honest copy still passes,** as a zero-change step with all counts 0. Decide whether to accept or refuse
     an empty step, and say which. The architect's lean is **accept**: it is a recorded no-op;
   - plus `--out` equal to the input, and every lookup rule, as in batch E: an invalid name, empty and relative
     `PATH` entries, a non-executable file, `.bat` and `.cmd`, shadowing.
5. **Mutation:** the `Decoded` pairs that `check` reads come from untrusted files. Add a seed that runs `check` on
   a mutated output against a fixed input, and assert that it never panics. Run the deep sweep.
6. **The usual gates,** on the topic branch. The Windows and macOS logs name the transform tests.

## 7. CHANGELOG `[Unreleased]`

- **Added:** "`brygge transform <name> <artifact> --out <file>`: runs an external transform plugin,
  `brygge-transform-<name>`, found on `PATH` only. brygge verifies the input first, then checks the output
  against it: every change marked, every removal recorded, nothing invented. It refuses the output otherwise,
  and writes nothing. `brygge transform --list` shows the plugins found (RFC 018)."
- **Added:** "`brygge_ir::transform`: `check` (the same output check, for any two artifacts) and `Edit` (builds a
  transformed artifact that passes it)."
- **Added:** "`examples/transform-authors`: an unsupported example transform, an author mapping. It is not part of
  brygge, and is never released."
- **Changed:** "In contract 0.3.0, a transform's recorded losses name exactly what was removed, and `parents` can
  be marked as changed."

## 8. Review request

`.git-exclude/review-requests/063-transform-command.md`, with the topic-branch routine:
- push the branch;
- CI green, with the Windows and macOS logs naming the transform tests;
- then fast-forward `main` after approval.

After it lands, RFC 018's scope is complete, and the architect plans the 0.7.0 cut.

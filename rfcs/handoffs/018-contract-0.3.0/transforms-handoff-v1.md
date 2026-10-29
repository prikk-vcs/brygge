# Handoff — RFC 018 D-5: transforms that record themselves, in the contract (batch T)

**Governing:** RFC 018, accepted 2026-10-29, D-5 as clarified after prikk's letter 005:
- each record's output digest is implied;
- the losses can be enumerated from the artifact alone.

**After batch C** (review 101's F-1 green). **Batch P** (`brygge transform`) follows, and its handoff after this one
closes. One review request, on `main`. Contract 0.3.0 is unreleased, so these critical additions go under 0.3.0
itself.

**This batch adds the records, their checks, and their carrying. It adds no command:** nothing in brygge writes a
transformed artifact yet. Its tests build them with `brygge-ir`.

## 0. First, commit the architect's files on their own

Commit as "RFC 018: the batch T handoff; Transformed on ops":
- this file;
- `rfcs/accepted/018-contract-0.3.0.md`: D-5 now names ops, and the vocabulary is closed.

## 1. The records

- **`Ir` field 6: `transforms`, `list<TransformRecord>`, oldest first,** omitted when empty (rule 3). So a decoder's
  artifact is byte-identical to batch C's, and the golden test must not move.
- **`TransformRecord`:**

  | id | Field | Type | Kind |
  |---|---|---|---|
  | 1 | name | text | req: `[a-z0-9][a-z0-9-]*`, at most 64 bytes (the plugin grammar) |
  | 2 | version | text | req: non-empty |
  | 3 | params | map | list |
  | 4 | consumed | id32 | req: the digest (D-3) of the artifact this step read |

- **`EpistemicStatus` variant 2: `Transformed`,** a record of its own (not spliced, unlike `Derived`):

  | id | Field | Type | Kind |
  |---|---|---|---|
  | 1 | step | uvarint | req: an index into `transforms` |
  | 2 | changed | `list<text>` | req: non-empty, strictly ascending, from the closed vocabulary below |
  | 3 | prior | `EpistemicStatus` | req: the status before this step |

  - **The vocabulary,** closed in 0.3.0:
    - on an atom: `author`, `author-time`, `committer`, `commit-time`, `message`, `ops`, `copies`;
    - on an op: `blob`, `mode`, `path`;
    - on a ref: `annotation`, `kind`, `name`, `target`.

    Anything else is refused.
  - **The Rust API:** a `ChangedPart` enum, rendered to and from these strings, so a caller cannot misspell one.
- **Where `Transformed` may appear:** an atom's, an op's and a ref's status.
  - **An op that a transform changed** (a new path, blob or mode) is `Transformed` at that step, with `prior` its
    input op's status. So replaced content never passes for stated.
  - **An op that a transform removed** is a `Transform` drop.
  - **An atom whose ops changed in any way** also lists `ops`.
  - **A transform may not add an op** that has no input op: that would be inventing content. A path or blob change is
    a changed op, not a new one; batch P's check matches ops by their input path.
- **`LossClass` variant 3: `Transform`.** `DropRecord` gains field 4, `step` (uvarint): **required for `Transform`,
  absent for every other class.**
  - **The canonical order of drops** becomes `(class variant, step, what)`, with an absent step first.
  - For a decoder's artifact, which has no steps, the order is unchanged.

## 2. The structural checks (the reader; `check_structure`)

All of these are refusals (`NonCanonical`, or `Decode` for a vocabulary violation), from both readers:
- **at most 64 records,** so a chain, and the nesting of `prior`, are bounded. Check `prior`'s depth
  iteratively, not by recursion;
- **a name** matching the grammar;
- **every `step`** less than the chain's length;
- **a `prior` that is `Transformed`** has a step strictly less than its own step;
- **every `Transform` drop's `step`** is in range, and no other class has a step;
- **an empty chain** comes with no `Transformed` status and no `Transform` drop anywhere;
- **`changed`** is non-empty, ascending and unique, and the right vocabulary for an atom, an op or a ref;
- **a 0.2.0 artifact holding any of these** is refused: `Ir` field 6, status variant 2, loss class 3, drop field 4.
  They do not exist in 0.2.0, and the shared metadata decoder must know the contract it reads.

**A note on what a reader cannot check alone:** whether an unmarked atom is truly unchanged. That needs the input,
and it is batch P's output check. Say so in the format page.

## 3. `brygge-ir`'s API

- **The model types:** `TransformRecord`, `EpistemicStatus::Transformed { step, changed, prior }`, `ChangedPart`,
  `LossClass::Transform`, and `DropRecord::step`. They are public, as the other model types are.
- **`pub fn artifact::digest_of(ir: &Ir) -> Result<[u8; 32]>`:** the D-3 digest that `write_to` would write,
  computed by streaming the logical stream into the hash. No artifact is written, and one blob is held at a time.
  - `write_to` and `digest_of` share one code path, so they cannot diverge;
  - test that `digest_of(ir)` equals `Decoded::digest` after a write and a read, for every test IR.
- **The fidelity summary** (`honesty::summary`):
  - it counts `Transformed` atoms and refs per step, and `Transform` drops per step;
  - `render_human` and `render_machine` show them only when the chain is non-empty. A decoder's report is
    byte-identical to today's (test it).

## 4. `verify`

- **The internal checks** (`commands.rs`): `check_derivations` and every other check that reads a status look
  through `Transformed` to its `prior` chain. A `Derived` prior is still checked for its required parameters. No
  check treats `Transformed` as `Stated`.
- **`--against-source` on an artifact with a chain:**
  1. re-decode and align, exactly as today;
  2. compute `digest_of` of the re-decode;
  3. compare it with `transforms[0].consumed`:
     - **equal:** a new outcome, **`root-corresponds`** (CVS: **`root-reproduces`**). Human: "the chain's root
       corresponds to the source; *N* transform(s) after it were not re-run". The exit code is the same as for
       `corresponds`;
     - **not equal:** `does-not-correspond` (or `does-not-reproduce`), with the detail "the chain's root is not
       this source's decode".
  - It never prints plain `corresponds` for an artifact that has a chain.
  - Add both outcomes to `machine-output.md`, and to the verify page's verdict list.
- **An artifact without a chain** is compared as today: `Ir` equality.

## 5. `inspect`

- **After the provenance:** "transforms: *N*", and one line per step:
  `<k>. <name> <version> — consumed sha256:<hex>; changed <a> atom(s), <r> ref(s); dropped <d>`.
  Add the parameters with `--atoms`.
- **The machine format:** `transforms=<N>`, then `transform.<k>.name`, `.version`, `.consumed`, `.changed_atoms`,
  `.changed_refs`, `.dropped`, following `machine-output.md`'s conventions. Add the section.
- **`--atoms`:** a transformed atom shows its step, its changed parts, and its prior status, as a chain:
  "transformed at 1 (author) ← stated".

## 6. The carrying rules (D-5)

- **`writing-an-encoder.md`, a new rule** beside the derived-marks rule: carry every `Transformed` mark with its
  step, record name, version and changed parts, and its prior status, or state in the target that they were
  dropped. Also report the chain to the user.
- **`examples/encode-git`:**
  - **a `Transformed` atom:** a trailer `Brygge-Transformed: <step> <name> <version> (<changed, comma-separated>)`;
  - **its prior:** carried as today's trailer would carry it. A `Derived` prior gets its `Brygge-Derived:`.
    Statuses nested deeper give one trailer each, newest first;
  - **a transformed ref:** a stderr line, since a Git ref has no message. An annotated tag also gets the trailer in
    its message;
  - **the chain:** listed on stderr, with each record's consumed digest. The README's trailer table gains the
    trailer.
  - **A test:** a transformed IR built in the test (a changed author at step 0, over a `Derived` atom) gives both
    trailers, and `git fast-import` accepts the stream.

## 7. Documents

- **`ir-artifact-format.md`:**
  - the three records and the new fields;
  - the drop order;
  - the checks in §2;
  - **the two clarifications:** an output digest is implied (record *k*'s output is record *k + 1*'s `consumed`;
    the last one's output is the artifact's own digest), and the losses can be enumerated;
  - the limit of what a reader can check alone;
  - a worked example with a one-record chain.
- **`writing-an-encoder.md`:** §6's rule.
- **Do not edit `brygge-03`,** as before.

## 8. Proof

1. **The round trip:** an IR with a two-record chain, and the following, round-trips through both readers, and
   `digest_of` matches:
   - `Transformed` atoms, ops and refs;
   - a `Transformed` with a `Transformed` prior over a `Derived` one;
   - `Transform` drops at both steps.
2. **No change for decoders:** the golden 0.3.0 file and every decoder's existing artifact are byte-identical,
   and their `inspect` output is unchanged.
3. **Refusals:** each rule in §2 gets its own test, including:
   - a `prior` at the same step;
   - 65 records;
   - an op's `changed` using an atom's vocabulary;
   - an unknown and an unordered `changed`;
   - a step on a `Representation` drop;
   - field 6 in a 0.2.0 artifact.
4. **`--against-source`, with a fixture repository:**
   - decode, then change an author with `brygge-ir` in the test, marked at step 0 with `consumed` = the
     original's digest. Write it, and `verify --against-source` gives `root-corresponds` with a clean exit;
   - **the same transformed IR with its chain removed** (a forgery) gives `does-not-correspond`;
   - **a chain whose `consumed` is wrong** gives `does-not-correspond`, with the root detail;
   - also once for CVS (`root-reproduces`).
5. **Mutation:** the seeds gain a transformed IR, so the new records reach the parser. Run the deep sweep before
   the push. Record its time and findings.
6. **The usual gates.**

## 9. CHANGELOG `[Unreleased]`

- **Added:** "IR contract 0.3.0 can record transforms (programs that turn one artifact into another): a chain of
  records, each citing the digest of the artifact it read; a `Transformed` status that keeps the status before it;
  and a `Transform` loss. `inspect` shows the chain; `verify --against-source` checks its root against the source
  (`root-corresponds`). brygge itself writes no transformed artifact yet (RFC 018)."
- **Added:** "`brygge_ir::artifact::digest_of`: an IR's digest, without writing it."

## 10. Review request

`.git-exclude/review-requests/062-transform-records.md`. Commit and push to `main` after approval.

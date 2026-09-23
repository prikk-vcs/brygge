# Machine-readable output (`--format machine`)

Every brygge command can print its result in a stable, versioned, line-oriented form for scripts and CI
gates, instead of the human report. This page is that contract: the external design's machine-readable
output (CL-07) and report format (CT-04).

## 1. Format

- **Lines of `key=value`,** one per line, each ending in a newline, **on stdout only**.
- **Everything else goes to stderr.** That includes:
  - the faithfulness statement `decode` prints before it runs;
  - the `wrote <path> (<n> bytes)` line;
  - notes such as "field(s) from a newer contract were not understood";
  - every error and refusal message.

  A consumer can read stdout without filtering. On an error or refusal stdout is **empty**, and the exit
  code (§4) says what happened.
- **Keys never contain text from a source repository.** Keys are fixed words, fixed labels (listed
  below), and decimal indexes (`atom.3.id`, `loss.0.what`).
- **Naming rule.** A key is dot-separated segments in `snake_case` (every key matches
  `^[a-z0-9_]+(\.[a-z0-9_]+)*$`, which is tested). An enumerated value, a *label*, is `kebab-case`. Where a
  label becomes part of a key, its hyphens become underscores: the label `inferred-rename` is counted
  under the key `derived.inferred_rename`. Any text that can come from a source (a ref
  name, a message, a path, a drop or flag description) appears **only as a value**.
- **Values that can carry source text are percent-encoded.**
  - Every byte outside the safe set `A-Z a-z 0-9 - . _ ~ / @ : +` becomes `%XX`, with uppercase hex. That
    includes `%`, `=`, space, newline, and each byte of a non-ASCII UTF-8 character.
  - So an encoded value can never contain `=` or a line break, and can never forge a line or a key.
  - To recover the text, percent-decode the value, then read the bytes as UTF-8.
  - The *Enc.* column below marks the keys whose values are encoded this way. Other values are numbers,
    lowercase hex, or labels drawn only from the safe set.
- **The order of lines is fixed** for a given artifact, as listed in the tables. Indexed groups count up
  from `0` with no gaps. Grouped counts (`derived.*`, `dropped.*`, `flagged.*`) appear in ascending order
  of their label, and only when non-zero.
- **Hex values** (`atom.N.id`, `ref.N.target`, and the rest) are lowercase and unencoded. IR ids are 64 hex
  digits (SHA-256).
- **`atom.N.source_id` is the hex of the source's own identifier bytes:**
  - a Git commit SHA-1 or an hg changeset node: 40 digits;
  - an SVN revision number, as an 8-byte big-endian integer: 16 digits;
  - a CVS reconstructed changeset, which has no native id: the hex of its sorted `path@revision` list,
    with entries joined by newlines.

## 2. Versioning

Each block of output starts with the version key that governs it. The versions are **independent of the
brygge release and of the IR contract**.

| Version key | Governs | Current value |
|---|---|---|
| `report_version` | the fidelity report: the lines `decode` and `inspect` print (§3.1) | `3` |
| `inspect_version` | the per-atom listing `inspect --atoms` appends (§3.2) | `4` |
| `verify_version` | `verify`'s lines (§3.3) | `4` |

**The rule (normative):**
- A version is **bumped** whenever any key or value domain of the output it governs changes:
  - a key is added, removed or renamed;
  - a label is added to or removed from a value domain;
  - the meaning of a key or value changes;
  - the order guarantee changes.
- Nothing is changed silently under an unchanged version.
- **A consumer checks the version key first,** and treats an unknown version as unknown output rather than
  guessing.

`contract_version` (in the atom listing) is the IR contract version of the artifact format this build
reads and writes. It is not an output version.

## 3. Keys

### 3.1 The fidelity report — `decode` and `inspect` (governed by `report_version`)

`decode` prints this after writing the artifact. `inspect <artifact>` prints the same lines for any
artifact.

| Key | Value | Enc. |
|---|---|---|
| `report_version` | `3` | |
| `atoms` | number of atoms (commits / changesets / revisions) | |
| `refs` | number of refs | |
| `blobs` | number of distinct content blobs | |
| `content_bytes` | total bytes of those blobs | |
| `skipped_non_critical_fields` | fields from a newer IR contract that this build skipped when reading the artifact (always printed; `0` when none, and always `0` from `decode`) | |
| `derived.<kind>` | count of derived records of that kind, for each kind present. `<kind>` is one of `inferred_rename`, `reconstructed_changeset`, `reconstructed_branch`, `inferred_merge`, `normalized_metadata`, `other` | |
| `dropped.<class>` | count of drop records of that class, for each class present. `<class>` is one of `representation`, `advisory_unreliable`, `other` | |
| `flagged.<kind>` | the summed `count` of flags of that kind, for each kind present. `<kind>` is one of `convention_violation`, `below_confidence_floor` | |

### 3.2 The atom listing — `inspect --atoms` (governed by `inspect_version`)

The listing is appended after §3.1's lines. It does not repeat `atoms`: every key appears once.

| Key | Value | Enc. |
|---|---|---|
| `inspect_version` | `4` | |
| `contract_version` | the IR contract version, `major.minor.patch` (currently `0.2.0`) | |
| `atom.N.id` | the atom's IR id (hex) | |
| `atom.N.status` | `stated`, or `derived:<kind>` with `<kind>` a label (`inferred-rename`, `reconstructed-changeset`, `reconstructed-branch`, `inferred-merge`, `normalized-metadata`, `other`) | |
| `atom.N.status_other` | present only when the status is `derived:other`: the kind's own name | ● |
| `atom.N.source_id` | the source's own id for the atom (hex; empty if none) | |
| `atom.N.ops` | number of path operations | |
| `atom.N.parents` | number of parents | |
| `atom.N.message` | the message's exact bytes (empty when there is none) | ● |
| `atom.N.copy.M.from` | copy source path | ● |
| `atom.N.copy.M.from_atom` | the IR id of the atom the copy came from (hex) | |
| `atom.N.copy.M.to` | copy destination path | ● |
| `atom.N.copy.M.status` | `stated` or `derived:<kind>` | |
| `atom.N.copy.M.status_other` | present only when the status is `derived:other`: the kind's own name | ● |
| `atom.N.copy.M.move` | `true` (the source was deleted in the same atom: a rename) or `false` | |
| `ref.N.name` | ref name | ● |
| `ref.N.kind` | `branch`, `tag`, `bookmark`, `named-branch`, `other` | |
| `ref.N.kind_other` | present only when the kind is `other`: the kind's own name | ● |
| `ref.N.target` | the IR id of the atom it names (hex) | |
| `ref.N.status` | `stated` or `derived:<kind>` | |
| `ref.N.status_other` | present only when the status is `derived:other`: the kind's own name | ● |
| `loss.N.class` | `representation`, `advisory-unreliable`, `other` | |
| `loss.N.what` | what was dropped, with its count | ● |
| `flag.N.kind` | `convention-violation`, `below-confidence-floor` | |
| `flag.N.what` | what was flagged | ● |
| `flag.N.count` | how many | |

Atoms appear in the artifact's order (parents before children); copies, refs, drops and flags appear in
artifact order.

**`atom.N.message` is byte-exact.** Percent-decode it to get the message's bytes, whatever they are,
including a message that is not UTF-8. (The human form of `inspect` shows control characters escaped, as
all human output does.) An absent message and an empty one both print an empty value.

### 3.3 `verify` (governed by `verify_version`)

| Key | Value | Enc. |
|---|---|---|
| `verify_version` | `4` | |
| `verify.skipped_non_critical_fields` | fields from a newer IR contract this build skipped when reading the artifact (always printed; `0` when none) | |
| `verify.check.<name>` | `pass`, `fail`, or `not-checked` (the check could not run). `<name>`, in this order: `integrity`, `structure`, `replay`, `derivations`, `source_invariants`, `provenance`, `loss_boundary` | |
| `verify.check.<name>.detail` | printed right after its check's line, only when the check is not `pass`: why | ● |
| `verify.authorship` | always `unverifiable` | |
| `verify.internal` | `pass` if every check passed, else `fail` | |
| `verify.against_source` | `not-run` (no `--against-source`); for Git, hg and SVN `corresponds` or `does-not-correspond`; for CVS `reproduces` or `does-not-reproduce`; or `not-checked` (requested but could not run) | |
| `verify.against_source.detail` | present only when `verify.against_source` is `does-not-correspond`, `does-not-reproduce` or `not-checked`: why | ● |
| `verify.against_source.note` | present only when the source comparison succeeded with something worth noting (e.g. `svnadmin versions differ (<a> vs <b>); the history is identical`). A note never means failure | ● |
| `verify.result` | `pass`, `fail` (something that ran did not hold), or `incomplete` (a requested source comparison could not run, and nothing failed) | |

The seven `verify.check.<name>` lines always appear, in the order above, each followed by its `.detail`
when it did not pass. `not-run` (for `verify.against_source`) means "not requested"; `not-checked` means
"requested or due, but could not run". `verify.result` is always last.

## 4. Exit codes

The exit code carries the outcome class, so a gate can branch without parsing output.

| Code | Meaning | From |
|---|---|---|
| `0` | clean: completed, with at most representation-class drops (storage layout, nothing of the history) | any command |
| `10` | recorded loss: completed, and something other than representation was dropped and recorded | `decode` |
| `20` | refused: a source feature brygge refuses rather than approximates (a floor feature, named in the message), an unsupported format, or a resource ceiling | `decode` |
| `30` | flagged: completed, but a condition needs attention (an SVN layout brygge could not follow, or CVS reconstructions under the confidence floor) | `decode` |
| `40` | reserved (partial or interrupted import); not produced by 0.1.0 | — |
| `50` | `verify` result `fail` | `verify` |
| `1` | runtime failure: unreadable input, I/O, an internal decoder fault; also `verify` result `incomplete`. They are told apart by stdout: a runtime failure prints nothing, while an `incomplete` verify prints its lines ending in `verify.result=incomplete` | any command |
| `2` | usage error: bad arguments, or an option given to a source kind it does not apply to | any command |

When several classes apply to one `decode`, `30` takes precedence over `10`.

## 5. Examples

Real output of brygge 0.1.0 on a two-commit Git repository with a rename and an annotated tag, decoded
with rename inference. Ids depend on the commits' exact bytes.

`brygge decode git repo --out repo.ir --infer-renames --format machine` (exit `0`):

```text
report_version=3
atoms=2
refs=2
blobs=1
content_bytes=6
skipped_non_critical_fields=0
derived.inferred_rename=1
dropped.representation=3
```

`brygge inspect repo.ir --atoms --format machine` (exit `0`; the §3.1 lines, then):

```text
inspect_version=4
contract_version=0.2.0
atom.0.id=40c88458f2adab841b4353ad79f99c845609fdc74b5e4d5a3910406f5947175d
atom.0.status=stated
atom.0.source_id=9dcbe4988473481fe5490b12403c1603b1a814c1
atom.0.ops=1
atom.0.parents=0
atom.0.message=first%20commit%0A
atom.1.id=4f14ba03b534cbe7961cd07dc3ac24519610fb75fff2f1b891bb47a862119019
atom.1.status=stated
atom.1.source_id=7c702384c1b99eb1b3a9ccd42daeee929ed8de91
atom.1.ops=2
atom.1.parents=1
atom.1.message=rename%20%3D%20move%0A
atom.1.copy.0.from=a.txt
atom.1.copy.0.from_atom=40c88458f2adab841b4353ad79f99c845609fdc74b5e4d5a3910406f5947175d
atom.1.copy.0.to=b.txt
atom.1.copy.0.status=derived:inferred-rename
atom.1.copy.0.move=true
ref.0.name=master
ref.0.kind=branch
ref.0.target=4f14ba03b534cbe7961cd07dc3ac24519610fb75fff2f1b891bb47a862119019
ref.0.status=stated
ref.1.name=v1
ref.1.kind=tag
ref.1.target=4f14ba03b534cbe7961cd07dc3ac24519610fb75fff2f1b891bb47a862119019
ref.1.status=stated
loss.0.class=representation
loss.0.what=index%20and%20working%20tree
loss.1.class=representation
loss.1.what=packfile%20and%20delta%20layout%2C%20physical%20object%20store
loss.2.class=representation
loss.2.what=reflogs
```

`brygge verify repo.ir --against-source repo --format machine` (exit `0`):

```text
verify_version=4
verify.skipped_non_critical_fields=0
verify.check.integrity=pass
verify.check.structure=pass
verify.check.replay=pass
verify.check.derivations=pass
verify.check.source_invariants=pass
verify.check.provenance=pass
verify.check.loss_boundary=pass
verify.authorship=unverifiable
verify.internal=pass
verify.against_source=corresponds
verify.result=pass
```

The same artifact truncated to 200 bytes, `brygge verify truncated.ir --format machine` (exit `50`):

```text
verify_version=4
verify.skipped_non_critical_fields=0
verify.check.integrity=fail
verify.check.integrity.detail=brygge-ir%20integrity%20digest%20mismatch%20%28tampered%20or%20truncated%29
verify.check.structure=not-checked
verify.check.structure.detail=the%20artifact%20failed%20to%20decode%3B%20this%20check%20cannot%20run
verify.check.replay=not-checked
verify.check.replay.detail=the%20artifact%20failed%20to%20decode%3B%20this%20check%20cannot%20run
verify.check.derivations=not-checked
verify.check.derivations.detail=the%20artifact%20failed%20to%20decode%3B%20this%20check%20cannot%20run
verify.check.source_invariants=not-checked
verify.check.source_invariants.detail=the%20artifact%20failed%20to%20decode%3B%20this%20check%20cannot%20run
verify.check.provenance=not-checked
verify.check.provenance.detail=the%20artifact%20failed%20to%20decode%3B%20this%20check%20cannot%20run
verify.check.loss_boundary=not-checked
verify.check.loss_boundary.detail=the%20artifact%20failed%20to%20decode%3B%20this%20check%20cannot%20run
verify.authorship=unverifiable
verify.internal=fail
verify.against_source=not-run
verify.result=fail
```

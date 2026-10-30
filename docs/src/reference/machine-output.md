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
| `inspect_version` | the transform-chain section (§3.2) and the per-atom listing `inspect --atoms` appends (§3.3) | `4` |
| `verify_version` | `verify`'s lines (§3.4) | `4` |
| `encode_list_version` | `encode --list`'s lines (§3.5) | `1` |
| `encode_version` | `encode`'s own banner and exit lines (§3.6) | `1` |
| `transform_list_version` | `transform --list`'s lines (§3.7) | `1` |
| `transform_version` | `transform`'s own banner, exit, and result lines (§3.8) | `1` |

**The rule (normative):**
- A version is **bumped** whenever any key or value domain of the output it governs changes:
  - a key is added, removed or renamed;
  - a label is added to or removed from a value domain;
  - the meaning of a key or value changes;
  - the order guarantee changes.
- Nothing is changed silently under an unchanged version.
- **A consumer checks the version key first,** and treats an unknown version as unknown output rather than
  guessing.

`contract_version` (in the atom listing) is the artifact's **own** IR contract version (RFC 018: this
build writes `0.3.0` but still reads `0.2.0`, so an older artifact's `inspect` reports `0.2.0`, not this
build's own `0.3.0`). It is not an output version.

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
| `dropped.<class>` | count of drop records of that class, for each class present. `<class>` is one of `representation`, `advisory_unreliable`, `other`, `transform` | |
| `flagged.<kind>` | the summed `count` of flags of that kind, for each kind present. `<kind>` is one of `convention_violation`, `below_confidence_floor` | |
| `transformed_atoms.<step>` | count of atoms a transform marked `Transformed` at that step, for each step present (RFC 018 D-5). Present only when the artifact has a non-empty transform chain | |
| `transformed_refs.<step>` | as `transformed_atoms.<step>`, for refs | |
| `transformed_drops.<step>` | as `transformed_atoms.<step>`, for `Transform`-class drops | |

### 3.2 The transform chain — `inspect` (governed by `inspect_version`)

Printed right after §3.1's lines, for **any** `inspect <artifact>` call (RFC 018 D-5, batch T) —
unlike §3.3, this section is not gated behind `--atoms`. It is omitted entirely when the artifact's
transform chain (`Ir.transforms`) is empty, which is every artifact brygge itself writes today (RFC 018
D-5: brygge writes no transformed artifact yet) — so a decoder's own `inspect` output is unchanged.

| Key | Value | Enc. |
|---|---|---|
| `transforms` | the chain's length, `N` | |
| `transform.<k>.name` | step `k`'s own transform name | ● |
| `transform.<k>.version` | step `k`'s own transform version | ● |
| `transform.<k>.consumed` | the D-3 digest (hex) of the artifact step `k` read | |
| `transform.<k>.changed_atoms` | how many atoms carry a `Transformed` layer naming step `k`, anywhere in their status chain — an atom with a multi-step chain counts once at each step it actually names, not only its outermost one | |
| `transform.<k>.changed_refs` | as `changed_atoms`, for refs | |
| `transform.<k>.dropped` | how many `Transform`-class drops name step `k` | |

Steps appear in chain order, `0` to `N - 1`. There is no `transform.<k>.params.*` key in this form — a
step's own parameters are a human-form-only detail (`inspect --atoms`'s human rendering, under
`show_params`); this machine form carries exactly the six keys above, every time.

**This section carries no `inspect_version=` line of its own.** The key is printed once, together with
§3.3's atom listing, whenever `--atoms` is given; when it is omitted, this section's keys still appear
(if the chain is non-empty) but with no version line in that call's own output — a consumer pinning this
section's schema reads the `inspect_version` value stated here (`4`) directly, or adds `--atoms` to see
the line printed.

### 3.3 The atom listing — `inspect --atoms` (governed by `inspect_version`)

The listing is appended after §3.1's lines. It does not repeat `atoms`: every key appears once.

| Key | Value | Enc. |
|---|---|---|
| `inspect_version` | `4` | |
| `contract_version` | the artifact's own IR contract version, `major.minor.patch` (this build writes `0.3.0`; an older artifact may report `0.2.0`) | |
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

### 3.4 `verify` (governed by `verify_version`)

| Key | Value | Enc. |
|---|---|---|
| `verify_version` | `4` | |
| `verify.skipped_non_critical_fields` | fields from a newer IR contract this build skipped when reading the artifact (always printed; `0` when none) | |
| `verify.check.<name>` | `pass`, `fail`, or `not-checked` (the check could not run). `<name>`, in this order: `integrity`, `structure`, `replay`, `derivations`, `source_invariants`, `provenance`, `loss_boundary` | |
| `verify.check.<name>.detail` | printed right after its check's line, only when the check is not `pass`: why | ● |
| `verify.authorship` | always `unverifiable` | |
| `verify.internal` | `pass` if every check passed, else `fail` | |
| `verify.against_source` | `not-run` (no `--against-source`); for Git, hg and SVN `corresponds` or `does-not-correspond`; for CVS `reproduces` or `does-not-reproduce`; `root-corresponds`/`root-reproduces` (RFC 018 D-5: the artifact has a non-empty transform chain, so only its chain's own *root* — `Ir.transforms[0].consumed` — was compared against a fresh decode, never the whole artifact); or `not-checked` (requested but could not run) | |
| `verify.against_source.detail` | present only when `verify.against_source` is `does-not-correspond`, `does-not-reproduce` or `not-checked`: why | ● |
| `verify.against_source.note` | present only when the source comparison succeeded with something worth noting (e.g. `svnadmin versions differ (<a> vs <b>); the history is identical`; or, for `root-corresponds`/`root-reproduces`, always: `<N> transform(s) after it were not re-run`). A note never means failure | ● |
| `verify.result` | `pass`, `fail` (something that ran did not hold), or `incomplete` (a requested source comparison could not run, and nothing failed) | |

The seven `verify.check.<name>` lines always appear, in the order above, each followed by its `.detail`
when it did not pass. `not-run` (for `verify.against_source`) means "not requested"; `not-checked` means
"requested or due, but could not run". `verify.result` is always last.

### 3.5 `encode --list` (governed by `encode_list_version`)

| Key | Value | Enc. |
|---|---|---|
| `encode_list_version` | `1` | |
| `encoder.N.target` | the plugin's target name | ● |
| `encoder.N.path` | its resolved, absolute path | ● |
| `encoder.N.shadowed` | `true` if an earlier `PATH` directory already has a plugin of this target (brygge would never run this one); else `false` | |

Plugins appear in `PATH` order (and, within one directory, by target name — a directory listing's own
order is unspecified). On stdout, like every other command's result; `encode --list` runs no plugin, so
nothing competes for stdout the way `encode <target>` itself does (§3.6).

### 3.6 `encode <target>` (governed by `encode_version`)

`encode <target>`'s own status — unlike every other command's machine output — goes to **stderr, not
stdout**: stdout is the plugin's, inherited and untouched, for the whole run. Nothing about the plugin's
own output is in this contract; only brygge's own banner (printed once, before the plugin starts) and
the line after it ends are.

**If the mandatory verify-first check (OQ-4) does not pass,** the plugin never starts, and brygge prints
`verify`'s own lines (§3.4) to stderr instead of this section — look for `verify_version`, not
`encode_version`, to tell the two apart.

**Otherwise, before the plugin starts:**

| Key | Value | Enc. |
|---|---|---|
| `encode_version` | `1` | |
| `encode.target` | the target name | ● |
| `encode.plugin_path` | the plugin's resolved, absolute path | ● |
| `encode.banner` | always `not_part_of_brygge` | |

**After the plugin ends,** exactly one of:

| Key | Value | Enc. |
|---|---|---|
| `encode.exit_code` | the plugin's own exit code (brygge's own process exits with the same code) | |
| `encode.signal` | Unix only: the signal number that killed the plugin (brygge's own process then exits `1`) | |

### 3.7 `transform --list` (governed by `transform_list_version`)

| Key | Value | Enc. |
|---|---|---|
| `transform_list_version` | `1` | |
| `transformer.N.name` | the plugin's own name | ● |
| `transformer.N.path` | its resolved, absolute path | ● |
| `transformer.N.shadowed` | `true` if an earlier `PATH` directory already has a plugin of this name (brygge would never run this one); else `false` | |

The same shape §3.5 (`encode --list`) uses, over `brygge-transform-*` plugins instead of
`brygge-encode-*` ones. On stdout; `transform --list` runs no plugin.

### 3.8 `transform <name>` (governed by `transform_version`)

`transform <name>`'s own status — like `encode <target>` (§3.6) — goes to **stderr, not stdout**: the
plugin's own stdout/stderr are inherited and untouched for the whole run.

**If the mandatory verify-first check does not pass, or the input has two atoms sharing one source
identity,** the plugin never starts, and brygge prints `verify`'s own lines (§3.4) — or, for the
source-identity refusal, a single line starting `refused:` — to stderr instead of this section.

**Otherwise, before the plugin starts:**

| Key | Value | Enc. |
|---|---|---|
| `transform_version` | `1` | |
| `transform.name` | the plugin's own name | ● |
| `transform.plugin_path` | the plugin's resolved, absolute path | ● |
| `transform.banner` | always `not_part_of_brygge` | |

**After the plugin ends,** exactly one of:

| Key | Value | Enc. |
|---|---|---|
| `transform.exit_code` | the plugin's own exit code (brygge's own process exits with the same code) | |
| `transform.signal` | Unix only: the signal number that killed the plugin (brygge's own process then exits `1`) | |

**On exit `0`, once the plugin's own output has passed every internal check and
`brygge_ir::transform::check`**, exactly these, naming the step `check` itself just accepted:

| Key | Value | Enc. |
|---|---|---|
| `transform.step` | the new transform record's own index | |
| `transform.atoms_changed` | how many atoms this step changed | |
| `transform.ops_changed` | how many ops this step changed | |
| `transform.refs_changed` | how many refs this step changed | |
| `transform.dropped` | how many new drops this step made | |

If the plugin's own output fails any check instead, brygge prints one line, `the transform's output was
refused: <why>`, deletes the plugin's temp output, leaves `--out` untouched, and exits `50` — none of the
five keys above appear.

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

**`encode <target>`'s own exit code is not in this table:** if its mandatory verify-first check does not
pass, it exits with `verify`'s own code above; otherwise it exits with the plugin's own code, unchanged
(or `1`, naming the signal, if the plugin was killed by one). A bad or reserved target, or a target not
found on `PATH`, is `2`.

**`transform <name>`'s own exit code is not in this table either**, and follows more of its own path
than `encode`'s does: if verify-first fails, `verify`'s own code; if the input has two atoms sharing one
source identity, `50`; otherwise the plugin runs, and a non-zero exit or a signal passes through exactly
as `encode`'s does; on a clean plugin exit, `50` if the plugin's own output fails any check (§3.8), else
the fidelity class `decode` would have given that same output (this table's `0`/`10`/`30` rows). A bad
name, or a name not found on `PATH`, is `2`; `--out` naming the same file as `<artifact>` is also `2`.

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
contract_version=0.3.0
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

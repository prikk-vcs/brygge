# brygge-ir

The **intermediate representation** at the heart of [brygge](https://github.com/prikk-vcs/brygge) — the
faithfulness-with-provenance model that version-control history is decoded *into* and target formats are
encoded *from*.

`brygge-ir` is the **light core**: it holds the IR model, its canonical **content-addressed, versioned,
integrity-digested** artifact, and the honesty machinery (the derived-vs-stated marking, the loss
boundary, and the recoverable fidelity report). It links **no source-decoder dependency** (only
`sha2`), so a target can check a brygge import on its own surface — the concrete meaning of "brygge
carries the dependency weight, the target does not".

Two ideas it enforces at the type level:

- **Faithfulness with provenance, not neutrality.** Every assertion is `Stated` (the source recorded
  it) or `Derived` (a decoder inferred it, with parameters); a rename/copy is stored as the source's
  literal delete+create **plus** a marked [`CopyRecord`], never collapsed.
- **Evidence for identity, never identity.** There is no node-identity type; a node-identity target
  authors identity itself, visibly, from the IR's evidence.

**IR contract `0.2.0`** (RFC 011): the wire format is **tagged records with a critical bit** — every
field carries an id and a critical flag, in a strict canonical form enforced on read (RFC 011 §2.2). An
unrecognized field fails the read if it is critical, and is skipped (and counted) if it is not — the
forward-compatibility mechanism a future minor version relies on. The full wire format, its canonical
rules, and an annotated minimal artifact are published at
[`docs/src/reference/ir-artifact-format.md`](../../docs/src/reference/ir-artifact-format.md) — the
contract a foreign encoder or reader author needs (`PU-3`).

Design: brygge RFC 001 (IR foundations), 002 (honesty machinery), 003 (determinism/format, superseded in
part), 011 (contract re-cut), under 009 (dependency policy). See the repository's `rfcs/` and `docs/`.

```sh
cargo run -p brygge-ir --example ir_roundtrip   # build → serialize → verify → report, no source needed
```

License: Apache-2.0.

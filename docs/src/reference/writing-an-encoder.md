# Writing an encoder

brygge's half of the pipeline ends at the IR artifact (requirement `PU-3`): a decoder reads a source
repository and writes an artifact; `inspect`/`verify` read it back and report on it; an **encoder** reads
it and produces something in a target system. brygge supports no encoder itself (it is not part of
brygge's shipped surface) — this page is for whoever writes one, on `brygge-ir` alone, for any target.

It assumes you know Rust and your target format, but have never read brygge's own code. The one real
example, `examples/encode-git` (IR → a `git fast-import` stream), is unsupported and not part of brygge,
but it is complete, tested against real fixtures from all four of brygge's decoders, and worth reading
alongside this page — every rule below is one it actually follows.

## 1. Reading an artifact

```rust,ignore
let decoded = brygge_ir::from_file(&artifact_path)?; // or `read_from` over any `Read + Seek`
let ir: &brygge_ir::Ir = &decoded.ir;
```

Before `from_file`/`read_from` return anything, they have already checked, in this order (normative in
the [IR artifact format](ir-artifact-format.md)):

1. the container's magic and format byte;
2. the contract **version** — an unknown contract is refused (`Error::UnsupportedContract`), before
   anything else is even attempted;
3. the integrity **digest** over the whole stored body — nothing below is accepted before it matches
   (`Error::DigestMismatch`);
4. the metadata's and blob store's **canonical form** — strictly ascending blob ids, no orphan or
   missing blob, every atom id unique, every parent/ref target known;
5. each blob's own **content address** — a blob's bytes are hashed and checked against its claimed id as
   they stream past.

So by the time you have a `Decoded`, the `Ir` inside it is exactly what the artifact's own digest and
every blob's own content address vouch for. **A blob read later is re-hashed against its content address
too** (the artifact-backed `ContentStore`): if the underlying file changes after the pass that verified
it, the next read of that blob returns a typed `ArtifactChanged` error, never silently-different bytes.
Read blobs **one at a time**, with `ContentStore::get` — the whole point of the artifact-backed store
(RFC 015 D-3/D-4) is that nothing beyond the metadata and a small index needs to be in memory at once;
buffering every blob up front throws that away for no benefit.

## 2. The order

`ir.atoms` is already in **canonical order**: parents before children. You do not need to sort or
topologically walk it yourself — one pass, in the given order, is enough to know that every atom's
parents have already been seen. Process `ir.refs` after every atom (every ref's target atom is already
behind you), then surface the loss boundary (`ir.loss`) and flags (`ir.flags`) to whoever is watching —
see §3.

## 3. The honesty obligations of an encoder

brygge's IR distinguishes what a source **stated** from what a decoder **derived**
(`EpistemicStatus`) and records what it explicitly **dropped** (`ir.loss`) rather than silently omitting
it. An encoder's job is to carry that distinction into the target, not to flatten it:

- **Carry `Derived` marks and their parameters into the target, or say they were dropped.** Every
  `Derived` assertion names its kind and the exact parameters that produced it (`DerivationKind`). If
  your target has nowhere to put that (no trailer, no note, no custom field), say in your own output, in
  words, that something was inferred and dropped — never let it disappear without a trace.
  `examples/encode-git` puts one `Brygge-Derived: <scope> <kind> <params>` trailer per derivation on the
  commit it concerns.

  ```text
  Brygge-Derived: atom reconstructed-changeset window_secs=180,cluster_keys=author+message,date_rule=…
  ```

- **Never invent a value the IR does not carry.** A missing committer, a missing time zone, a missing
  email — if your target's format *requires* something the IR left absent (most targets require more
  structure than the IR promises, by design — see §5), you may supply a placeholder, but you must mark
  that you did. `examples/encode-git` adds one `Encoded-supplied: <fields>` trailer naming exactly what
  it invented (for example `committer, commit-time-zone`), and never writes that trailer when nothing was
  supplied.

- **Report the loss boundary and flags.** `ir.loss.dropped` names every class of thing the decoder
  explicitly chose not to carry, and why; `ir.flags` names conditions that need the user's attention
  (a convention violation, a confidence floor). `brygge_ir::summary` turns both, plus the derived counts,
  into one `FidelityReport` — render it (`render_human`/`render_machine`) and show it to whoever is
  running your encoder, the same way `brygge inspect`/`verify` already do. `examples/encode-git` prints
  it to stderr once, after writing its main output.

- **Present the result as your own work, never as something brygge verified.** brygge verified the
  *artifact* — its integrity, its canonical form, every blob's content address. It verified nothing about
  what you do with it. Label your output as such. `examples/encode-git`'s every commit trailer, its
  `--help`, and its `README.md` all say the same thing: *"an unsupported example; not part of brygge."*
  Say the equivalent for your own encoder, with your own target's name in place of "example."

[status]: https://docs.rs/brygge-ir (crate docs — `EpistemicStatus`)
[kind]: https://docs.rs/brygge-ir (crate docs — `DerivationKind`)
[summary]: https://docs.rs/brygge-ir (crate docs — `summary`)
[report]: https://docs.rs/brygge-ir (crate docs — `FidelityReport`)

## 4. Versioning

Pin the **IR contract version** your encoder supports (`brygge_ir::version::CURRENT` is the one the
`brygge-ir` you built against produces) — never the brygge tool's own release version. The two move on
different schedules (`IX-07`): a contract version changes only when the wire format itself does: a
3-release-old `brygge` binary and today's can both write contract `0.2.0` artifacts. `brygge-ir`'s own
reader already refuses an artifact whose contract it does not know
([`Error::UnsupportedContract`][contract]) before parsing a byte of its metadata — you inherit that
refusal for free by reading through `from_bytes`/`read_from`/`from_file` rather than parsing the
container yourself. The full wire format, if you ever do need to parse it directly, is normative in
[IR artifact format](ir-artifact-format.md).

## 5. Running as a `brygge encode` plugin

<!-- See batch E (RFC 017 D-3): this section is written once that lands. -->

## 6. What brygge does not do for you

brygge supports no third-party encoder — there is no plugin API beyond a process convention (§5), no
registry, no compatibility promise for anything you build. And brygge verifies the **artifact**: its
integrity, its canonical form, its content addressing. It never verifies an encoder's output. Whatever
your encoder writes to its target is your own claim, not brygge's.

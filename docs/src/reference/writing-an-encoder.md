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
   anything else is even attempted; this build writes contract `0.3.0` and still reads `0.2.0`
   (RFC 018 D-7);
3. the integrity **digest** — nothing below is accepted before it matches (`Error::DigestMismatch`).
   For a `0.3.0` artifact (compact artifacts, RFC 018 D-1) this is the SHA-256 of the **logical**
   (decompressed) content, never of the on-disk, possibly-DEFLATE-compressed bytes; a `0.2.0` artifact's
   digest is the prior definition, over the stored bytes directly. `Decoded::digest` is always the one
   the reader actually checked, whichever contract it is;
4. the metadata's and blob store's **canonical form** — every atom id unique, every parent/ref target
   known, no orphan or missing blob, and (for a `0.3.0` artifact) the blob table in **store order** —
   see §2 — rather than ascending by id;
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

If your encoder reads every blob in bulk rather than lazily per op (an encoder for a target with its own
content store, for instance), use `brygge_ir::artifact::store_order(ir)` for the order to read them in:
it is the same order a `0.3.0` artifact's blob frames are packed in, so reading in that order touches
each compressed frame exactly once, instead of re-inflating one repeatedly out of order.
`examples/encode-git` reads every blob this way, before writing any commit.

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

- **Mark a value you had to *change*, even when the IR did carry it — distinctly from one you
  invented.** A target's own syntax can force a change on a value the IR stated in full: Git's
  line-oriented identity syntax cannot carry a literal newline, an angle bracket, a NUL, or non-UTF-8
  bytes, so `examples/encode-git` folds, strips, or replaces them to write the line at all. That is not
  invention — the IR carried a real value — but it is not exactly what the source said either, so it gets
  its own trailer, `Encoded-altered: <field> (<why>)`, never folded into `Encoded-supplied:`:

  ```text
  Encoded-altered: author-name (not UTF-8, replaced), author-email (newline folded)
  ```

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
3-release-old `brygge` binary and today's can both write contract `0.3.0` artifacts. `brygge-ir`'s own
reader already refuses an artifact whose contract it does not know
([`Error::UnsupportedContract`][contract]) before parsing a byte of its metadata — you inherit that
refusal for free by reading through `from_bytes`/`read_from`/`from_file` rather than parsing the
container yourself. The full wire format, if you ever do need to parse it directly, is normative in
[IR artifact format](ir-artifact-format.md).

## 5. Running as a `brygge encode` plugin

`brygge encode <target> <artifact> [-- <plugin arguments>…]` runs your encoder as an external plugin
(RFC 017 D-3), found on `PATH` — brygge does not ship one, does not register one, and does not know
anything about it beyond the convention below.

- **The binary name is `brygge-encode-<target>`.** On Unix, any executable regular file of that name on
  `PATH`; on Windows, `brygge-encode-<target>.exe` specifically — **never `.bat` or `.cmd`**, which run
  through `cmd.exe` and its own argument-splitting rules, not the exact bytes brygge spawns with. brygge
  resolves the path itself and spawns it directly: never the bare name, so the current directory and a
  source repository are never where it looks (`PATH` only, in order, skipping empty and relative
  entries — not even the directory brygge itself runs from).
- **The target name is `[a-z0-9][a-z0-9-]*`, at most 64 bytes.** Anything else is a usage error before
  any lookup happens. **`prikk` is reserved** for the built-in encoder (Track B1, RFC 008) and is never
  looked up on `PATH`, even if a `brygge-encode-prikk` exists there.
- **The arguments you receive:** `brygge-encode-<target> --artifact <artifact> [<plugin arguments>…]`.
  `<plugin arguments>` are exactly what the user wrote after `--`, verbatim, in order — brygge runs no
  shell, so there is no quoting or expansion to account for.
- **Your stdin, stdout and stderr are the user's**, inherited and untouched. Whatever you print to stdout
  is the user's output; brygge writes nothing there itself while you run.
- **brygge verifies the artifact before it ever starts you** (OQ-4): the same checks `brygge verify
  <artifact>` runs (never `--against-source`). If the verdict is not a clean `pass`, brygge prints that
  report instead and exits with `verify`'s own exit code (50 for a failed check; 1 for an unreadable
  file, an unsupported contract, or a pre-release artifact) — **you are never started at all.** If you
  read the artifact yourself through `brygge-ir`, you get the same checks over again (including
  `ArtifactChanged` if it changes under you); if you parse the bytes yourself, you get nothing from
  brygge — the guarantee in §1 is `brygge-ir`'s, not the file's.
- **A banner brackets your run, on stderr, not yours to print:** before you start, brygge names you and
  your resolved path and states you are not part of it; after you end, it states your exit code (or, on
  Unix, the signal that killed you). Because brygge's own failures (a bad or reserved target, not found,
  a failed verify) all happen *before* that banner, a user who sees it knows the exit code that follows
  is yours, not brygge's.
- **Your own exit code is passed straight through** — brygge's process exits with it unchanged. If a
  signal kills you (Unix), brygge exits `1` and names the signal. If brygge cannot even start you (a
  permission error, a race where the file vanished), that is brygge's own failure: exit `1`, with the OS
  error and the path.
- **`brygge encode --list [--format human|machine]`** shows every `brygge-encode-*` plugin found this
  way, in `PATH` order, so a user can see what is actually reachable; one hidden by an earlier `PATH`
  directory's plugin of the same target is marked shadowed — the machine form is in
  [Machine-readable output](machine-output.md).

## 6. What brygge does not do for you

brygge supports no third-party encoder — there is no plugin API beyond a process convention (§5), no
registry, no compatibility promise for anything you build. And brygge verifies the **artifact**: its
integrity, its canonical form, its content addressing. It never verifies an encoder's output. Whatever
your encoder writes to its target is your own claim, not brygge's.

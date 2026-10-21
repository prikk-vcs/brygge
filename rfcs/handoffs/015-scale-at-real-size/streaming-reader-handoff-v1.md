# Handoff — RFC 015 D-4: a streaming reader for `verify`, `inspect` and `--against-source` (batch V)

**Governing:** RFC 015 D-4, and reviews 074 and 076. **Crates:** `brygge-ir` (the reader), and the CLI. One review
request, on a topic branch (it touches file I/O). **Targets:** fish-shell's `verify` and `inspect` (P4/P5), from
9.3 GiB to **under 1 GiB**; `verify --against-source` (P6) **under 1 GiB**. **Every reader check keeps its exact
semantics and its errors.**

## 1. One sequential pass, and every later read re-verified (the security design)

- **The trap:** "hash the file, then parse it" is two passes. A file that changes between them (another writer, a
  hostile filesystem) could pass the digest, then be parsed as different bytes. **That is not acceptable for the
  check a target relies on** (C-3b).
- **The design:**
  1. **one sequential pass,** over a single open handle, does everything at once:
     - it feeds every byte read into the artifact digest;
     - it parses the header and the version (the version gate first, as today);
     - it reads the **metadata** into memory (it is small: 15 MiB for fish-shell);
     - it reads the **blob section's** headers, checks the canonical order (strictly ascending ids), and
       **hashes each blob's bytes as they stream past** (its content address), keeping only an index: `BlobId`
       → (offset, length) in the file;
     - at the end, it compares the digest. **Nothing is accepted before the digest matches.**
  2. **Every later blob read** (when a check or a caller needs the bytes) **re-hashes the bytes it reads, and
     compares them with the `BlobId`.** A mismatch is a typed error: "the artifact changed while it was being
     read" (a new, specific error). So nothing read later can differ silently from what the pass verified.
- **The result** is an `Ir` whose `ContentStore` uses S-1's file backing, **with the artifact itself as the
  file:** the index points into the artifact's blob section, and no blob is copied. Add the constructor S-1 left
  room for, `pub(crate)`, with the hash-on-read rule built into the backing's `get` for this source.
- **The API:** `artifact::read_from<R: Read + Seek>(r) -> Result<Decoded>`, or a `from_file(&Path)`; choose, and
  say why. `from_bytes(&[u8])` stays, as **the same parser over a `Cursor`**, so the two can never diverge.

## 2. What runs on it

- **`verify`** (all its checks) and **`inspect`** read through the streaming reader. A check that needs blob bytes
  (replay, a symlink target, content checks) reads them on demand, one at a time.
- **`verify --against-source`:**
  - its re-decode uses **`Storage::Spill`**, by default in the **artifact's directory**;
  - **`verify` accepts `--spill-dir <DIR>`** too, with the same rules as `decode` (it must exist, exit 2
    otherwise, never in the IR);
  - its comparison runs against the streaming-read artifact. `Ir` equality is unchanged (blob id sets, review
    076).
- **The pre-release gate, `PreReleaseArtifact`, the unknown-contract refusal and the skipped-non-critical-field
  count** behave exactly as today.

## 3. Proof

1. **The same verdicts, everywhere:** for every fixture artifact, the bench corpora's artifacts and **all nine
   real-corpus artifacts**, `verify` and `inspect` give **identical stdout and exit codes** on `main` and on this
   tree (`cmp` the outputs, in both the human and the machine format).
2. **The same errors, for every malformed input:**
   - every existing reader test (the digest mismatch, the canonical-order cases, the referential integrity, the
     pre-release, the unsupported contract);
   - **and the mutation tests** (C-2g) run through **both** entry points, `from_bytes` and the streaming reader,
     on the same mutated bytes (a `Cursor` for the streaming one, so it is fast). The two must give the same
     `Ok` or the same error variant on every input. Add that equivalence as the mutation check.
3. **The changed-during-read test:** read an artifact through the streaming reader, then modify one blob's
   bytes in the file **after** the pass (the test holds a second handle). The next read of that blob gives the
   new "changed while being read" error, never the modified bytes.
4. **The A/B** with `brygge-bench profile`: P4, P5 and P6 (peak and time) for fish-shell, TortoiseHg and the
   synthetic targets. The targets are above. If any row's time worsens by more than 10%, report it.
5. **`verify --spill-dir`:** the same tests as `decode`'s (missing → exit 2, a file → exit 2, a good directory
   used, nothing left behind).
6. **The usual gates,** and the mutation tests at both depths. A topic branch, with the Windows job's log naming
   the reader tests.

## 4. CHANGELOG `[Unreleased]`

- **Added:** "`brygge-ir`: a streaming artifact reader (`…`); `brygge verify --spill-dir`."
- **Changed:** "`brygge verify` and `inspect` read an artifact in one streaming pass, holding only the metadata
  and an index (fish-shell: 9.3 GiB → N MiB). Every blob read later is re-checked against its content address.
  The verdicts are unchanged."

## 5. Review request

`.git-exclude/review-requests/053-streaming-reader.md`. **After V lands, RFC 015's scope is complete,** and the
architect prepares the 0.5.0 cut.

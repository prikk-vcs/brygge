# Handoff — RFC 019 D-4 (OQ-2, accepted): `brygge transform authors` and `inspect --authors` (batch G3, request 073)

**Governing:** RFC 019 D-4, accepted 2026-11-21 (OQ-2: in 0.8.0). **After:** G2, on `main` at `5fc6a33`.
**On `main`, no topic branch:** an in-process transform, with no new process or platform code.

## 0. Evidence plan first (N-3)

- **Level 1:** the map parser, and the transform over hand-built `Ir`s.
- **Level 2:** decode a Subversion and a CVS fixture, then `transform authors`, then `encode git`, which runs G2's
  check, then `git log` shows the mapped identities and `Brygge-Transformed`.
- **Level 3, to run in this batch:** wordpress-core and openbsd-games, mapped with a map that `inspect --authors`
  generated and you filled in by hand with placeholder addresses (`<name>@example.invalid`). Then encoded and
  checked.

## 1. `brygge inspect --authors <artifact>`

- **What it lists:** every distinct identity the artifact carries, as an atom's author or committer or an annotated
  tag's tagger.
  - Each with its count per role, sorted by total count (descending), then by bytes.
  - An identity is its name plus its email if present. Non-UTF-8 bytes are shown escaped, as `inspect` already
    shows them.
- **Human output: one line per identity, already in map-file form,** so a user can redirect it to a file and fill
  in the right-hand sides. For example:

  ```
  # brygge inspect --authors: 3 identities; fill in each right-hand side, delete lines you do not want to map
  deraadt =                # 812 author, 812 committer
  millert =                # 301 author, 301 committer
  Alice Example <alice@example.org> =      # 4 author
  ```

  The `#` comment is the map format's own (§2), so the file parses as soon as it is filled in.
- **Machine output:** `authors.N.name`, `authors.N.email` (absent if none), `authors.N.count.author|committer|tagger`,
  under a new `inspect_authors_version`.

## 2. The map file

- **Format:** UTF-8, one mapping per line. `#` starts a comment, to the end of the line. Blank lines are ignored.
- **A mapping:** `<source> = <Name> <<email>>`.
  - **`<source>` is a bare name**, matching an identity with exactly that name and **no email** (the Subversion and
    CVS case), **or** `<Name> <<email>>`, matching exactly that name and email (Git and hg).
  - A bare name never matches an identity that has an email, and the reverse. Two rules, no fuzzy match.
  - **The right side** needs a non-empty name and a non-empty email. It is checked against what `encode git`
    accepts, so a mapped identity is never altered later (no LF, `<`, `>` or NUL).
- **Refused, every problem listed with its line number:**
  - a line with no `=`;
  - an empty side;
  - a right side without `<email>`;
  - the same `<source>` twice;
  - a file that is not UTF-8.

## 3. `brygge transform authors <in> --map <file> --out <out> [--require-all]`

- **A built-in transform.** `authors` is reserved like `encode`'s `git`: a `brygge-transform-authors` on `PATH` is
  not run, and `transform --list` shows it as shadowed by the built-in. `examples/transform-authors` stays the
  plugin reference example, under its own name.
- **It runs in-process,** through `brygge_ir::transform::Edit`, exactly as a plugin would:
  - the step's name is `authors`, its version brygge's;
  - its params are `{map_sha256: <the map file's SHA-256>, require_all: true|false}`;
  - then the existing output check (`transform::check`), the atomic write, and `--out`'s rules, the same as
    `transform`.
- **What it changes:**
  - an atom's author and committer;
  - an annotated tag's tagger (`set_annotation`);
  - nothing else: times, messages and trees are untouched. Each change is marked `Transformed`, by the editor, as
    RFC 018 D-5 defines.
- **Unmapped identities:** listed on stderr with their counts. With `--require-all`, they refuse the transform
  (exit 2) and nothing is written.
- **A mapping that matched nothing:** a warning naming the line. It is not an error, because a shared map may cover
  several repositories.
- **The report:** how many atoms and tags changed, how many identities were mapped and how many left unmapped, in
  the `transform.*` machine keys (`machine-output.md`).

## 4. What `encode git` shows

**Nothing new in the encoder.** A mapped commit carries:
- `Brygge-Transformed: atom 0 authors <version> (author, committer)`, from the existing trailer;
- **no** `Brygge-Supplied: author-email`, because the email now exists. Test both.

G2's check passes over a mapped artifact, because it compares the transformed IR, which is the artifact `encode git`
reads.

## 5. Documentation and CHANGELOG

- **`docs/src/guide/git-target.md`:** a "Mapping authors" section with the two-command workflow:

  ```
  brygge inspect --authors repo.ir > authors.map    # then fill in each right-hand side
  brygge transform authors repo.ir --map authors.map --out mapped.ir --require-all
  brygge encode git mapped.ir --out repo.git
  ```

  Also: the bare-name and name-with-email rules, and that every mapped commit says so in its trailers.
- **`docs/src/reference/writing-a-transform.md`:** note the built-in `authors` and the reserved name.
- **`machine-output.md`:** the new keys and `transform --list`'s shadowing.
- **`CHANGELOG.md` `[Unreleased]`, `### Added`:** "`brygge transform authors`: a built-in transform that maps source
  identities to Git identities from a map file, recorded as a transform step, and `brygge inspect --authors`, which
  writes the starting map."

## 6. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- `cargo test --workspace --locked` on stable and 1.87. Report the **sum** over every binary;
- links, the book, the release tools, `cargo deny`;
- the deep mutation depth, if `mutation.yml`'s path filter matches.

## 7. Request

- **File** `.git-exclude/review-requests/073-authors-map.md`.
- **Push after approval only** (114).

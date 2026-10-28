# Handoff — RFC 017 D-3: `brygge encode <target>` and external encoder plugins (batch E)

**Governing:** RFC 017 D-3, with OQ-1 (`PATH` only) and OQ-4 (always verify first). **After batch G** (the guide and
`examples/encode-git`), which this batch uses as its test plugin. **Crate:** `brygge` (the CLI). One review request,
**on a topic branch** (it adds process spawning and per-OS executable lookup, so the Windows and macOS jobs must
show the tests run).

## 1. The command

- **`brygge encode <target> <artifact> [-- <plugin arguments>…]`**, and **`brygge encode --list [--format
  human|machine]`**.
- **`<target>` must match `[a-z0-9][a-z0-9-]*`,** at most 64 bytes. Anything else is a usage error (exit 2), and
  nothing is looked up.
- **`prikk` is reserved** for the built-in encoder (Track B1, RFC 008). Until it lands, `brygge encode prikk …`
  keeps today's message ("not available yet: the prikk encoder follows prikk's import foundations …"), exit 2,
  and is **never** looked up on `PATH`. So no plugin can stand in for the reserved name.
- **The arguments brygge passes,** directly, with no shell:
  `brygge-encode-<target> --artifact <artifact> [<plugin arguments>…]`. The plugin arguments are exactly those
  after `--`, verbatim. Without `--`, there are none; anything else after `<artifact>` is a usage error.
- **stdin, stdout and stderr are inherited.** The plugin's stdout is the user's output, untouched.

## 2. Finding the plugin: `PATH` only

- **brygge resolves the path itself,** then spawns that absolute path. Never pass the bare name to
  `Command::new`: on Windows, the OS search would look in the application's own directory and system directories
  before `PATH`.
- **The search:**
  - walk `PATH` in order;
  - **skip every empty and every relative entry** (an empty entry means the current directory on Unix). So the
    current directory, and a source repository, are never searched;
  - **Unix:** the first `<dir>/brygge-encode-<target>` that is a regular file (after following symlinks) with an
    execute bit;
  - **Windows:** the first `<dir>\brygge-encode-<target>.exe` that is a regular file. **`.exe` only:** never
    `.bat` or `.cmd`, which run through `cmd.exe` and its own argument rules. Say so in the guide.
- **Not found:** "no encoder for '\<target\>': `brygge-encode-<target>` is not on PATH (see `brygge encode
  --list`)", exit 2.
- **`--list`:** every `brygge-encode-*` found by the same rules, whose suffix is a valid `<target>`, with its
  resolved path, in `PATH` order. Mark one hidden by an earlier one as "shadowed". The machine format follows
  `docs/src/reference/machine-output.md`; add its section there.

## 3. Verify first, always (OQ-4)

- **Before spawning,** run `verify`'s own checks on the artifact, without `--against-source`, through the
  streaming reader.
- **If the verdict is not `pass`,** print `verify`'s report (in the chosen `--format`) to stderr, do not run the
  plugin, and exit with **`verify`'s own exit code** (50 for a failed check; 1 for an unreadable file; the existing
  codes for an unsupported contract and a pre-release artifact). This is the RFC's "exit 50", made exact.
- **Say plainly in the guide:** brygge verified the file when it checked it. A plugin that reads it with `brygge-ir`
  gets the same checks again, including `ArtifactChanged`; a plugin that parses the bytes itself gets nothing
  from brygge.

## 4. The banner, and the exit code

- **On stderr, before the plugin runs:**
  "encoding with the external plugin `brygge-encode-<target>` (`<resolved path>`): not part of brygge; brygge
  verified the artifact, not the plugin's output". Always, in both formats; in the machine format, as the
  documented machine form.
- **After it ends,** on stderr: "`brygge-encode-<target>` exited with \<code\>".
- **The exit code is the plugin's own.** If it was killed by a signal (Unix), brygge says which, and exits 1. If
  it cannot be started, that is brygge's failure: exit 1, with the OS error and the path.
- brygge's own failures (a bad or reserved target, not found, a failed verify) all happen **before** the banner,
  so the banner's presence tells a user whose exit code they are reading. Document that in the guide.

## 5. Documents

- **The guide** (`writing-an-encoder.md`): write batch G's "Running as a `brygge encode` plugin" section: the
  name, the lookup rules, the arguments, the exit codes, the banner, `.exe` only on Windows, and that `prikk` is
  reserved.
- **The external design** (`docs/src/brygge-02-external-design-v0.1.md`), CL-03, replaced by exactly this:
  "**CL-03 — `encode` runs external encoder plugins; `encode prikk` is Track B1's.** `brygge encode <target>`
  runs `brygge-encode-<target>`, found on `PATH` only, after verifying the artifact, and labels it as not part of
  brygge (RFC 017 D-3). The target `prikk` is reserved for the built-in encoder, which follows prikk's import
  foundations (Track B1)." Keep FL-03 as it is.
- **The help text and `README.md`:** add the `encode` line; the README says in one sentence that encoders are
  external, and that brygge supports none of them.
- **Do not edit `brygge-03`.** The architect adds the plugin boundary to the threat model at the 0.6.0 cut.

## 6. Proof

1. **The example as a plugin:** put `examples/encode-git`'s binary on a temporary `PATH`, and run `brygge encode
   git-example <artifact> -- --out <file>`. Then `git fast-import` the result, and get the same repository as
   running the example directly. Check the banner and the exit line.
2. **Lookup:**
   - an invalid target (`../x`, `A`, `x/y`, empty, 65 bytes) → exit 2, nothing searched;
   - `prikk` → today's message, exit 2, even with a `brygge-encode-prikk` on `PATH`;
   - an empty and a relative `PATH` entry holding the plugin are skipped;
   - a non-executable file is skipped (Unix); a `.bat` or `.cmd` is ignored (Windows);
   - two directories with the plugin → the first runs, and `--list` shows the second as shadowed.
3. **Verify first:** a corrupted artifact → `verify`'s report, its exit code, and **the plugin never runs** (the
   test plugin writes a marker file if it starts; the marker must be absent).
4. **Pass-through:** a test plugin exiting 0, 3 and 77 → brygge exits the same; one that kills itself with a
   signal → exit 1 with the signal named (Unix).
5. **Arguments:** spaces, quotes, a leading `-`, and non-ASCII reach the plugin verbatim (the test plugin echoes
   its `argv`); no shell runs.
6. **The usual gates,** on a topic branch, with the Windows and macOS jobs' logs naming the encode tests.

## 7. CHANGELOG `[Unreleased]`

- **Added:** "`brygge encode <target> <artifact>`: runs an external encoder plugin, `brygge-encode-<target>`,
  found on `PATH` only. brygge verifies the artifact first, refuses to run the plugin if it fails, and labels
  the plugin as not part of brygge. `brygge encode --list` shows the plugins found (RFC 017)."
- **Changed:** "`encode prikk` is reserved for the built-in prikk encoder, and still reports that it is not
  available yet."

## 8. Review request

`.git-exclude/review-requests/059-encode-plugins.md`, with the topic-branch routine. After it lands, RFC 017's 0.6.0
scope (Parts A and B) is complete, and the architect plans the 0.6.0 cut.

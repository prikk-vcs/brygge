# Handoff — RFC 022 batch W1: the watchdog for every writing command, the next-run notice, and stop statuses (request 097)

**Governing:**
- RFC 022 D-1 to D-4 and D-7, with refinement D-1a;
- reviews 208 to 210;
- HANDOFF §7 and §7a.

**W0 is on `main` (`f75279e`):** `Guard`, `Watchdog::start`, the handshake, D-3 for `.<name>.brygge-tmp-<pid>`, and
`stop_integration.rs`.

**The scope:**
- the three other writing commands;
- D-3's directory forms;
- D-4;
- D-7's statement.

Progress (P) and housekeeping (H) come later.

## 0. How it runs

- **Branch `prove/w1-every-command` from `main`.**
- **This batch deletes directories, so it is in HANDOFF §7a's first row:**
  - its first runs are in CI;
  - locally, before CI is green: build and clippy only;
  - after CI is green: HANDOFF §7's local list, once, through `tools/contained.sh`.
- **The first commit is 210's carried fix:** `log_line` writes with `writeln!(std::io::stderr().lock(), …)`, and
  spawns nothing.

## 1. Evidence plan first (N-3), starting with the horizontal list

**Before writing code, list in the request every temporary path each writing command creates:**
- which command (`decode`, `encode git`, `transform`, `verify --against-source`);
- where;
- under which name;
- whether it is a file or a directory;
- and when it is renamed or removed.

The list must be complete, including `encode git`'s private `HOME` and `GIT_CONFIG_GLOBAL`. **Every path on it is
registered, or the request says why not.** A spill file that is unlinked at once, for example, needs no registration.

**Level 1:**
- **D-3's directory forms:**
  - accepted only for the exact registered path, carrying the right process id;
  - a symbolic link is refused, and never followed;
  - a regular file where a directory was registered, and the reverse, are each refused.
- **The "stops changing" rule:**
  - a tree that changes is not removed and is named;
  - a tree still for 1 s is removed;
  - the 30 s ceiling.

  Test the rule over an injected clock and snapshot function, so that the tests take no real seconds.
- **D-4's scan:**
  - it names each of brygge's forms that carries another process id;
  - it names nothing for this run's own id;
  - it never deletes;
  - it never names a path that only resembles a form.
- **`main`:** the read-only commands (`inspect`, `verify` without `--against-source`, `encode --list`, `--help`,
  `--version`) start no watchdog. Test the decision as a pure function of the parsed command.

**Level 2, in CI on all three systems:**
- extend `stop_integration.rs` to one stopped case per new command: `kill -INT` on Unix and `taskkill /F` on Windows
  are enough. `kill -9` stays decode-only;
- **for `encode git`, the stop lands while the partial repository exists.** Assert that it is gone, or named, as D-3
  says;
- **one D-4 case:** a planted `.<name>.brygge-tmp-<other pid>` in the output directory is named, and left in place.

## 2. What to build

- **`main` starts the guard** for exactly the four writing commands, through one function. Each command takes
  `&mut Guard`, and every unit test passes `Guard::disabled()`.
- **D-3, widened to its directory forms:**
  - `<dir>.partial-<pid>-<n>`;
  - `.brygge-transform-<pid>-…`;
  - `encode git`'s private `HOME` and `GIT_CONFIG_GLOBAL`, with their own `<pid>`.

  **A directory** is removed with `std::fs::remove_dir_all`, which does not follow symbolic links inside the tree
  (fixed in Rust 1.58.1, below our MSRV). It is removed only after the "stops changing" rule (D-3): no change in 1 s,
  checked for at most 30 s.
- **D-4:**
  - **When:** before writing, `decode`, `encode git` and `transform` scan their output directory, at one level, for
    brygge's forms that carry another process id.
  - **What it says, on stderr:** `found <path> (<size>), a temporary file of brygge process <pid>; if no brygge is
    running, remove it with: rm <path>` (`del` on Windows).
  - **A directory** is named as a directory, with no size, and with `rm -r` (on Windows, `rmdir /s`).
  - **Paths** are printed through `display::human`.
  - **It never deletes.**
- **D-7:**
  - **`machine-output.md`** states the statuses: 130 after Ctrl-C, 143 after SIGTERM, and Windows' own; 40 stays
    reserved and unused;
  - **the user guide** gains one short paragraph: what a stop leaves, the sentence, and the next-run notice;
  - **HANDOFF** documents the internal entry once, as internal (D-2).
- **`tools/check-self-exec.sh`:** no new site. `start` is still called only from `main`.

## 3. Checks and request

- **Locally,** HANDOFF §7's list, once, after CI is green.
- **The request** is `.git-exclude/review-requests/097-w1-every-command.md`. It carries:
  - the temporary-path list;
  - the evidence plan;
  - the CI run, with the macOS and Windows log lines of the new stop cases;
  - the list of what ran locally, and under which limit.
- **Keep it short (209 H-3):** what changed, what was checked, and the evidence.

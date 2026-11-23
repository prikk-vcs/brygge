#!/usr/bin/env python3
"""Review 127 F-2: one committed method for a release's corpus figures, used by
docs/src/development/releasing.md's corpus step.

Standard library only: brygge-ir's workspace forbids `unsafe_code`, so brygge-bench cannot call
`wait4` itself, and GNU `time` is not installed on this machine. This is the method that produced
every corpus figure accepted so far (review 099's `os.wait4` fix, approved by review 100): one child
per measurement, its own `ru_maxrss` from `os.wait4`, never summed or subtracted across two calls.

Usage: python3 -I tools/bench/release-corpus.py <brygge-binary> <cache-dir> <real.toml>
           [--decode-args "<args>"] [--encode-git] <name>...

<cache-dir> is `fixtures/corpus` (tools/fixtures.sh's layout): <cache-dir>/<kind>/<name> for git, hg and
cvs entries, <cache-dir>/svn/<name>.dump for svn. <real.toml> (review 128 R-2: `tools/corpus/real.toml`)
is read to find each name's own recorded `kind`, rather than guessing it from whichever directory under
<cache-dir> happens to hold a matching name first. <name>... are the real.toml entry names to measure
(for example: eza fish-shell tortoisehg-thg).

For each name: `<brygge-binary> decode <kind> <source> --out <artifact> [<decode-args>]`, then
`<brygge-binary> verify <artifact>` — the same two command lines request 061's figures used. Prints one
line per name: the artifact's size in bytes, the decode's time (ms) and peak RSS (KiB), and verify's
time (ms) and peak RSS (KiB). This is unchanged when neither new flag is given.

**`--decode-args "<args>"`** (RFC 019 D-8, batch G4): a whitespace-split string of extra decode flags
(for example `"--branch-lines"` or `"--reconstruct-refs"`), appended after `--out <artifact>`.

**`--encode-git`** (RFC 019 D-8, batch G4): after `verify`, run `<brygge-binary> encode git <artifact>
--out <repo>` into a temporary directory, as one more child under `os.wait4` — the same measurement
discipline as decode and verify, never summed or subtracted. Prints three more fields:
`encode_time_ms`, `encode_peak_kib`, and the result's own `<repo>.brygge-report` sidecar's
`encode_git.check` and `encode_git.commits` (read back, not re-derived), so a mismatch between what the
report says and what this script measured would show up as a parse failure, not a silent guess.
"""

import os
import sys
import tempfile
import time
from pathlib import Path


def parse_real_toml_kinds(real_toml: Path) -> dict[str, str]:
    """`{name: kind}` for every `[[entry]]` in real.toml — the same deliberately tiny subset of TOML
    `tools/bench/src/real.rs`'s own parser reads (`key = "value"` lines, one `[[entry]]` per block)."""
    kinds: dict[str, str] = {}
    name = kind = None
    for raw in real_toml.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line == "[[entry]]":
            if name is not None and kind is not None:
                kinds[name] = kind
            name = kind = None
            continue
        if "=" not in line:
            continue
        key, _, value = line.partition("=")
        key, value = key.strip(), value.strip().strip('"')
        if key == "name":
            name = value
        elif key == "kind":
            kind = value
    if name is not None and kind is not None:
        kinds[name] = kind
    return kinds


def locate(cache_dir: Path, name: str, kind: str) -> Path:
    kind_dir = cache_dir / kind
    dump = kind_dir / f"{name}.dump"
    if kind == "svn" and dump.exists():
        return dump
    candidate = kind_dir / name
    if candidate.exists():
        return candidate
    raise SystemExit(f"release-corpus: no cached {kind} entry named {name!r} under {cache_dir}")


def run_one(
    argv: list[str], label: str, log_path: Path, *, allow_exit: set[int] = frozenset({0})
) -> tuple[int, int]:
    """Run argv as one child, timed, with os.wait4 giving that child's own peak RSS (`ru_maxrss`, KiB on
    Linux) — never a sum or a difference across two calls (the RUSAGE_CHILDREN lesson of review 099): this
    is the one child's own usage, not the parent's running total. The child's own stdout/stderr go to
    `log_path`, not this process's, so the script's own output stays exactly one line per repository; on
    failure, the log is printed before raising, so nothing is lost.

    `allow_exit` widens what counts as success: `decode`'s exit code carries its fidelity class
    (machine-output.md §4), and 10 ("recorded loss") and 30 ("flagged") both mean the artifact was
    written, not that the command failed. The caller passes the exact set that keeps the artifact."""
    start = time.monotonic()
    with open(log_path, "wb") as log:
        pid = os.fork()
        if pid == 0:
            os.dup2(log.fileno(), 1)
            os.dup2(log.fileno(), 2)
            os.execvp(argv[0], argv)
            os._exit(127)  # pragma: no cover - only reached if execvp itself fails
    _, status, rusage = os.wait4(pid, 0)
    elapsed_ms = round((time.monotonic() - start) * 1000)
    code = os.waitstatus_to_exitcode(status)
    if code not in allow_exit:
        sys.stderr.write(log_path.read_text(errors="replace"))
        raise SystemExit(f"release-corpus: {label} failed (exit {code}): {argv}")
    return elapsed_ms, rusage.ru_maxrss


def parse_report(report_path: Path) -> dict[str, str]:
    """`<dir>.brygge-report`'s own `key=value` lines (always the machine format, RFC 019 D-2 OQ-1)."""
    fields: dict[str, str] = {}
    for raw in report_path.read_text().splitlines():
        key, _, value = raw.partition("=")
        if key:
            fields[key] = value
    return fields


def main() -> None:
    if len(sys.argv) < 5:
        raise SystemExit(
            "usage: release-corpus.py <brygge-binary> <cache-dir> <real.toml> "
            '[--decode-args "<args>"] [--encode-git] <name>...'
        )
    binary = str(Path(sys.argv[1]).resolve())
    cache_dir = Path(sys.argv[2]).resolve()
    real_toml = Path(sys.argv[3]).resolve()

    rest = sys.argv[4:]
    decode_args: list[str] = []
    encode_git = False
    names: list[str] = []
    i = 0
    while i < len(rest):
        if rest[i] == "--decode-args":
            i += 1
            if i >= len(rest):
                raise SystemExit("release-corpus: --decode-args needs a value")
            decode_args = rest[i].split()
            i += 1
        elif rest[i] == "--encode-git":
            encode_git = True
            i += 1
        else:
            names.append(rest[i])
            i += 1

    kinds = parse_real_toml_kinds(real_toml)
    print(f"# brygge binary: {binary}")
    for name in names:
        if name not in kinds:
            raise SystemExit(f"release-corpus: {real_toml} has no entry named {name!r}")
        kind = kinds[name]
        source = locate(cache_dir, name, kind)
        with tempfile.TemporaryDirectory() as tmp:
            artifact = Path(tmp) / "artifact"
            decode_ms, decode_peak = run_one(
                [binary, "decode", kind, str(source), "--out", str(artifact), *decode_args],
                f"{name}: decode",
                Path(tmp) / "decode.log",
                allow_exit={0, 10, 30},
            )
            size = artifact.stat().st_size
            verify_ms, verify_peak = run_one(
                [binary, "verify", str(artifact)],
                f"{name}: verify",
                Path(tmp) / "verify.log",
            )
            line = (
                f"name={name} kind={kind} artifact_bytes={size} "
                f"decode_time_ms={decode_ms} decode_peak_kib={decode_peak} "
                f"verify_time_ms={verify_ms} verify_peak_kib={verify_peak}"
            )
            if encode_git:
                repo = Path(tmp) / "repo"
                encode_ms, encode_peak = run_one(
                    [binary, "encode", "git", str(artifact), "--out", str(repo)],
                    f"{name}: encode git",
                    Path(tmp) / "encode.log",
                    allow_exit={0, 10, 30},
                )
                report = parse_report(Path(f"{repo}.brygge-report"))
                line += (
                    f" encode_time_ms={encode_ms} encode_peak_kib={encode_peak} "
                    f"encode_git_check={report['encode_git.check']} "
                    f"encode_git_commits={report['encode_git.commits']}"
                )
            print(line)


if __name__ == "__main__":
    main()

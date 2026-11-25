#!/usr/bin/env python3
"""Review 169 F-2: `svn-tags`/`svn-tags-plain`'s scale/time matrix, measured the same way
`release-corpus.py` measures the real corpus — one child per run, its own peak RSS from `os.wait4`
(review 099's fix, never summed or subtracted across two calls). `svn-tags` is synthetic (RFC 020 batch
M/S's own 157-shaped generator inside `brygge-bench`), not a `real.toml` entry, so `release-corpus.py`
itself has nothing to measure it from: this is the same method, for the one scenario it does not cover,
not a second method invented for other reasons.

Usage: python3 -I tools/bench/svn-tags-corpus.py <brygge-binary> <brygge-bench-binary> <n>...
           [--branch-lines]

For each `n`: `<brygge-bench-binary> corpus svn-tags-plain <n> <tmp-dir>` writes the dump to
`<tmp-dir>/in.dump` (the generator's own fixed name, `tools/bench/src/main.rs`'s `prepare()`) — not
timed under `os.wait4`'s allow-list; corpus generation is this harness's own setup cost, not the
decoder's, exactly as `release-corpus.py` does not time locating its own cached fixture. Then
`<brygge-binary> decode svn <dump> [--branch-lines] --out <artifact>`, timed as the one child that
matters. Prints one line per `n`: the artifact's size, the decode's time (ms) and peak RSS (KiB), and
(for comparison against the overhead review 169 asked this script to help isolate) the generation
step's own time, labelled as what it is.
"""

import os
import sys
import tempfile
import time
from pathlib import Path


def run_one(
    argv: list[str], label: str, log_path: Path, *, allow_exit: set[int] = frozenset({0})
) -> tuple[int, int]:
    """[`release-corpus.py`'s] own `run_one`, unchanged: one child, timed, with `os.wait4` giving that
    child's own peak RSS — never a sum or a difference across two calls."""
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
        raise SystemExit(f"svn-tags-corpus: {label} failed (exit {code}): {argv}")
    return elapsed_ms, rusage.ru_maxrss


def main() -> None:
    if len(sys.argv) < 4:
        raise SystemExit(
            "usage: svn-tags-corpus.py <brygge-binary> <brygge-bench-binary> <n>... [--branch-lines]"
        )
    binary = str(Path(sys.argv[1]).resolve())
    bench_binary = str(Path(sys.argv[2]).resolve())
    rest = sys.argv[3:]
    branch_lines = "--branch-lines" in rest
    ns = [int(a) for a in rest if a != "--branch-lines"]

    mode = "--branch-lines" if branch_lines else "plain"
    print(f"# brygge binary: {binary}  mode: {mode}")
    for n in ns:
        with tempfile.TemporaryDirectory() as tmp:
            corpus_dir = Path(tmp) / "corpus"
            gen_ms, _ = run_one(
                [bench_binary, "corpus", "svn-tags-plain", str(n), str(corpus_dir)],
                f"n={n}: generate",
                Path(tmp) / "gen.log",
            )
            dump = corpus_dir / "in.dump"
            artifact = Path(tmp) / "artifact"
            decode_argv = [binary, "decode", "svn", str(dump), "--out", str(artifact)]
            if branch_lines:
                decode_argv.append("--branch-lines")
            decode_ms, decode_peak = run_one(
                decode_argv,
                f"n={n}: decode",
                Path(tmp) / "decode.log",
                allow_exit={0, 10, 30},
            )
            size = artifact.stat().st_size
            print(
                f"n={n} artifact_bytes={size} decode_time_ms={decode_ms} "
                f"decode_peak_kib={decode_peak} generate_time_ms={gen_ms}"
            )


if __name__ == "__main__":
    main()

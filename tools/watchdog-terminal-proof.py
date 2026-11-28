#!/usr/bin/env python3
"""RFC 022 §1's hand-run terminal proof (handoff `w0-platform-proof-handoff-v2.md` §1, "Level 2, by hand,
on Linux, after CI is green"): a real Ctrl-C and a real Ctrl-Z, delivered through an actual pseudo-terminal
under an *interactive* `bash` with job control on — exactly as review 200's prototype did, so the same
clause-by-clause table can be reproduced here, committed, and re-run by hand rather than only described.

This is **not** a CI test, and it is not run by `cargo test` or `tools/contained-test.sh`: it is a manual,
by-hand proof, run once on Linux after CI is green on `prove/w0-watchdog` (HANDOFF §7a: this is spawning
code, so it is proven in CI first). Running it before then would start the real watchdog outside CI, which
the batch's own rules forbid.

Usage:
    tools/watchdog-terminal-proof.py [path-to-brygge-binary]

Default binary path: target/debug/brygge (built with a plain `cargo build`, which this script does not do
itself — build first).

What it drives, one pseudo-terminal session per row, each against a freshly built SVN dump fixture:
    1. a real Ctrl-C while the artifact is being written;
    2. a real Ctrl-Z while the artifact is being written, confirming the *process itself* is stopped (`T`
       in `ps`, not just its terminal job), then `fg` resumes it to a normal finish;
    3. `kill -9` of the decode process itself (not through the pty — a real external kill, review 200's
       "the lifeline could not do this");
    4. a normal, uninterrupted finish, confirming the watchdog prints nothing at all.

Each row prints what it observed, verbatim, never only "it worked".
"""

import os
import pty
import selectors
import signal
import struct
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_BIN = os.path.join(ROOT, "target", "debug", "brygge")


def svn_props(pairs):
    buf = b""
    for k, v in pairs:
        buf += f"K {len(k)}\n{k}\nV {len(v)}\n{v}\n".encode()
    buf += b"PROPS-END\n"
    return buf


def svn_dump_fixture(file_count=20, file_size=4 * 1024 * 1024, seed=b"watchdog-terminal-proof"):
    """A hand-built SVN dumpstream (never `svnadmin`): `file_count` files of `file_size` pseudo-random
    bytes each under trunk/, large enough that the artifact write visibly takes a few seconds — long
    enough for a human to send Ctrl-C/Ctrl-Z while it is in progress."""
    buf = bytearray()
    buf += b"SVN-fs-dump-format-version: 2\n\n"
    buf += b"UUID: 00000000-0000-0000-0000-000000000000\n\n"
    r0 = svn_props([])
    buf += f"Revision-number: 0\nProp-content-length: {len(r0)}\nContent-length: {len(r0)}\n\n".encode()
    buf += r0
    buf += b"\n"
    for d in ("trunk", "branches", "tags"):
        buf += (
            f"Node-path: {d}\nNode-kind: dir\nNode-action: add\nProp-content-length: 10\n"
            f"Content-length: 10\n\nPROPS-END\n\n"
        ).encode()
    r1 = svn_props(
        [
            ("svn:author", "watchdog-terminal-proof"),
            ("svn:date", "2024-01-01T00:00:00.000000Z"),
            ("svn:log", "add large files"),
        ]
    )
    buf += f"Revision-number: 1\nProp-content-length: {len(r1)}\nContent-length: {len(r1)}\n\n".encode()
    buf += r1
    buf += b"\n"
    for i in range(file_count):
        content = os.urandom(file_size)
        header = (
            f"Node-path: trunk/f{i}.bin\nNode-kind: file\nNode-action: add\n"
            f"Prop-content-length: 10\nText-content-length: {len(content)}\n"
            f"Content-length: {10 + len(content)}\n\nPROPS-END\n"
        ).encode()
        buf += header
        buf += content
        buf += b"\n\n"
    return bytes(buf)


class PtySession:
    """One pseudo-terminal running an interactive `bash -i`, so Ctrl-C and Ctrl-Z are delivered exactly as
    a real terminal would: to the foreground process group, under real job control — not simulated by a
    library calling `kill` on our behalf."""

    def __init__(self):
        self.pid, self.master_fd = pty.fork()
        if self.pid == 0:
            # Child: an interactive shell, forced interactive with `-i` regardless of the pty's other
            # heuristics, so job control (`set -m`) is on exactly as it would be for a real user.
            os.environ["PS1"] = "proof$ "
            os.execvp("bash", ["bash", "--norc", "--noprofile", "-i"])
            os._exit(127)  # unreachable unless exec itself fails
        self.buffer = b""
        self.sel = selectors.DefaultSelector()
        self.sel.register(self.master_fd, selectors.EVENT_READ)

    def send(self, s):
        os.write(self.master_fd, s.encode())

    def drain(self, timeout=0.5):
        """Read whatever the pty has to offer right now, appending it to the running transcript."""
        deadline = time.monotonic() + timeout
        read_any = False
        while time.monotonic() < deadline:
            events = self.sel.select(timeout=max(0.0, deadline - time.monotonic()))
            if not events:
                break
            try:
                chunk = os.read(self.master_fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            self.buffer += chunk
            read_any = True
        return read_any

    def wait_for(self, predicate, timeout, poll_interval=0.1):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain(poll_interval)
            if predicate():
                return True
        return False

    def shell_pid(self):
        return self.pid

    def close(self):
        try:
            os.kill(self.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            os.waitpid(self.pid, 0)
        except ChildProcessError:
            pass
        try:
            os.close(self.master_fd)
        except OSError:
            pass


def direct_child_pid(shell_pid):
    """The pid of `shell_pid`'s own direct child (the `brygge` process the shell launched), via
    `pgrep -P`, the same selection review 200 settled on after `pgrep brygge` first matched unrelated
    processes."""
    try:
        out = subprocess.run(
            ["pgrep", "-P", str(shell_pid)], capture_output=True, text=True, check=False
        )
    except FileNotFoundError:
        return None
    pids = [int(p) for p in out.stdout.split() if p.strip()]
    return pids[0] if pids else None


def process_state(pid):
    try:
        out = subprocess.run(
            ["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, check=False
        )
    except FileNotFoundError:
        return None
    s = out.stdout.strip()
    return s or None


def process_alive(pid):
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def report(title, transcript, extra_lines=()):
    print(f"\n==== {title} ====")
    for line in extra_lines:
        print(line)
    print("---- raw pty transcript ----")
    sys.stdout.write(transcript.decode(errors="replace"))
    if not transcript.endswith(b"\n"):
        sys.stdout.write("\n")
    print("---- end transcript ----")


def run_scenario_ctrl_c(brygge_bin, workdir):
    dump = os.path.join(workdir, "ctrlc.svndump")
    with open(dump, "wb") as f:
        f.write(svn_dump_fixture())
    out = os.path.join(workdir, "ctrlc-a.ir")

    s = PtySession()
    s.wait_for(lambda: b"proof$" in s.buffer, timeout=5)
    s.send(f"cd {workdir}\n")
    s.drain(0.3)
    s.send(f"{brygge_bin} decode svn {dump} --out {out}\n")

    def tmp_exists():
        return any(name.startswith(".ctrlc-a.ir.brygge-tmp-") for name in os.listdir(workdir))

    appeared = s.wait_for(tmp_exists, timeout=30)
    time.sleep(0.2)
    s.send("\x03")  # a real Ctrl-C, through the pty, to the foreground process group
    gone = s.wait_for(lambda: not tmp_exists(), timeout=10)
    s.drain(1.0)
    extra = [
        f"temp artifact appeared before Ctrl-C: {appeared}",
        f"temp artifact gone after Ctrl-C: {gone}",
        f"artifact {out} exists after Ctrl-C: {os.path.exists(out)}",
    ]
    report("Ctrl-C while writing", s.buffer, extra)
    s.close()


def run_scenario_ctrl_z(brygge_bin, workdir):
    dump = os.path.join(workdir, "ctrlz.svndump")
    with open(dump, "wb") as f:
        f.write(svn_dump_fixture())
    out = os.path.join(workdir, "ctrlz-a.ir")

    s = PtySession()
    s.wait_for(lambda: b"proof$" in s.buffer, timeout=5)
    s.send(f"cd {workdir}\n")
    s.drain(0.3)
    s.send(f"{brygge_bin} decode svn {dump} --out {out}\n")

    def tmp_exists():
        return any(name.startswith(".ctrlz-a.ir.brygge-tmp-") for name in os.listdir(workdir))

    appeared = s.wait_for(tmp_exists, timeout=30)
    child_pid = direct_child_pid(s.shell_pid())
    s.send("\x1a")  # a real Ctrl-Z
    time.sleep(0.5)
    s.drain(0.5)
    state_after_ctrl_z = process_state(child_pid) if child_pid else None
    still_registered = tmp_exists()
    s.send("fg\n")
    finished = s.wait_for(lambda: not tmp_exists(), timeout=30)
    s.drain(1.0)
    extra = [
        f"temp artifact appeared before Ctrl-Z: {appeared}",
        f"decode process pid: {child_pid}",
        f"process state right after Ctrl-Z (ps -o stat=): {state_after_ctrl_z!r} "
        "('T'/'T+' means truly stopped, not merely backgrounded)",
        f"temp artifact still present right after Ctrl-Z (the work did not keep going): {still_registered}",
        f"after `fg`, the run finished and the temp artifact is gone: {finished}",
        f"final artifact {out} exists: {os.path.exists(out)}",
    ]
    report("Ctrl-Z, then fg", s.buffer, extra)
    s.close()


def run_scenario_kill9(brygge_bin, workdir):
    dump = os.path.join(workdir, "kill9.svndump")
    with open(dump, "wb") as f:
        f.write(svn_dump_fixture())
    out = os.path.join(workdir, "kill9-a.ir")

    s = PtySession()
    s.wait_for(lambda: b"proof$" in s.buffer, timeout=5)
    s.send(f"cd {workdir}\n")
    s.drain(0.3)
    s.send(f"{brygge_bin} decode svn {dump} --out {out}\n")

    def tmp_exists():
        return any(name.startswith(".kill9-a.ir.brygge-tmp-") for name in os.listdir(workdir))

    appeared = s.wait_for(tmp_exists, timeout=30)
    child_pid = direct_child_pid(s.shell_pid())
    killed = False
    if child_pid:
        os.kill(child_pid, signal.SIGKILL)  # a real external kill -9, never through the pty
        killed = True
    gone = s.wait_for(lambda: not tmp_exists(), timeout=10)
    s.drain(1.0)
    extra = [
        f"temp artifact appeared before kill -9: {appeared}",
        f"decode process pid: {child_pid}, SIGKILL sent: {killed}",
        f"temp artifact gone after kill -9: {gone}",
        f"artifact {out} exists after kill -9: {os.path.exists(out)}",
    ]
    report("kill -9 of the decode process itself", s.buffer, extra)
    s.close()


def run_scenario_normal(brygge_bin, workdir):
    dump = os.path.join(workdir, "normal.svndump")
    with open(dump, "wb") as f:
        f.write(svn_dump_fixture(file_count=1, file_size=64 * 1024))  # small: finishes quickly
    out = os.path.join(workdir, "normal-a.ir")

    s = PtySession()
    s.wait_for(lambda: b"proof$" in s.buffer, timeout=5)
    s.send(f"cd {workdir}\n")
    s.drain(0.3)
    s.send(f"{brygge_bin} decode svn {dump} --out {out}; echo DONE:$?\n")
    finished = s.wait_for(lambda: b"DONE:" in s.buffer, timeout=30)
    s.drain(0.5)
    extra = [
        f"ran to completion: {finished}",
        f"artifact {out} exists: {os.path.exists(out)}",
        "expected: no 'interrupted' line anywhere above — the watchdog prints nothing on a normal finish",
    ]
    report("a normal, uninterrupted finish", s.buffer, extra)
    s.close()


def main():
    brygge_bin = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_BIN
    if not os.path.exists(brygge_bin):
        print(f"error: {brygge_bin} does not exist; build it first (cargo build)", file=sys.stderr)
        return 1
    brygge_bin = os.path.abspath(brygge_bin)

    workdir = os.path.join(
        "/tmp", f"brygge-watchdog-terminal-proof-{os.getpid()}-{int(time.time())}"
    )
    os.makedirs(workdir, exist_ok=True)
    print(f"working directory: {workdir}")
    print(f"brygge binary: {brygge_bin}")
    try:
        run_scenario_ctrl_c(brygge_bin, workdir)
        run_scenario_ctrl_z(brygge_bin, workdir)
        run_scenario_kill9(brygge_bin, workdir)
        run_scenario_normal(brygge_bin, workdir)
    finally:
        import shutil

        shutil.rmtree(workdir, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())

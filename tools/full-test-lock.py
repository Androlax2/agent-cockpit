#!/usr/bin/env python3
"""Run a full test suite while holding the lock every clone's full run shares.

Usage: full-test-lock.py LOCK_FOLDER HOLDER COMMAND [ARGUMENTS...]

Full runs in parallel clones loaded the machine enough to make timing tests flaky (research
96acba81), so they take turns. The kernel drops the lock when its holder dies: no stale lock.

A fresh run first passes agent-build's admission gate (todo d021dc46), before the lock: while a
game is open or memory is low it waits without holding the lock, so other clones keep their turns.
A run started inside an admitted job skips it. The admitted run's command carries this process's
admission marker (AGENT_BUILD_ADMITTED), so the builds it starts while holding the lock never wait
on the gate; its priority is unchanged.
"""

import errno
import fcntl
import os
import subprocess
import sys
from pathlib import Path

UNREACHABLE = {errno.EROFS, errno.EACCES, errno.EPERM, errno.ENOENT}
AGENT_BUILD = Path(__file__).resolve().parents[1] / "config/bin/.local/bin/agent-build"
ADMITTED_ENV = "AGENT_BUILD_ADMITTED"


def open_lock(lock_folder):
    os.makedirs(lock_folder, mode=0o700, exist_ok=True)
    # Every sandbox writes this folder; a session that left a symlink here must not make the host
    # overwrite the file it points to.
    return os.open(
        lock_folder / "full-test.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600
    )


def read_holder(lock):
    os.lseek(lock, 0, os.SEEK_SET)
    return os.read(lock, 4096).decode(errors="replace").strip() or "another run"


def main():
    if len(sys.argv) < 4:
        print("usage: full-test-lock.py LOCK_FOLDER HOLDER COMMAND [ARGUMENTS...]", file=sys.stderr)
        return 2
    lock_folder, holder, command = Path(sys.argv[1]), sys.argv[2], sys.argv[3:]
    while True:
        admission = subprocess.call(
            [sys.executable, str(AGENT_BUILD), "--wait-gate", f"the full test run of {holder}"]
        )
        if admission != 0:
            print(
                f"full-test-lock: the admission gate failed (exit {admission}); not running.",
                file=sys.stderr,
                flush=True,
            )
            return 128 - admission if admission < 0 else admission
        try:
            lock = open_lock(lock_folder)
        except OSError as error:
            if error.errno not in UNREACHABLE:
                raise
            print(
                f"full-test-lock: cannot reach {lock_folder} ({error.strerror}); refusing to run "
                "without the shared test lock. Restart the session to share it.",
                file=sys.stderr,
                flush=True,
            )
            return 126
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print(
                f"Waiting for the test lock (held by {read_holder(lock)}) …",
                file=sys.stderr,
                flush=True,
            )
            fcntl.flock(lock, fcntl.LOCK_EX)
        # The lock wait can be long: a game or a hold may have begun meanwhile. Then the run gives
        # the lock back and waits again, never waiting on the gate while holding the lock.
        still_admitted = subprocess.call([sys.executable, str(AGENT_BUILD), "--check-gate"])
        if still_admitted == 0:
            break
        os.close(lock)
        if still_admitted != 75:
            print(
                f"full-test-lock: the admission check failed (exit {still_admitted}); not running.",
                file=sys.stderr,
                flush=True,
            )
            return 128 - still_admitted if still_admitted < 0 else still_admitted
        print(
            "full-test-lock: new heavy jobs became held while this run waited for the lock; "
            "it gave the lock back and waits again.",
            file=sys.stderr,
            flush=True,
        )
    os.ftruncate(lock, 0)
    os.pwrite(lock, f"{holder}\n".encode(), 0)
    # The command does not inherit the lock: a daemon a test leaves behind must not hold it.
    return subprocess.call(command, env=os.environ | {ADMITTED_ENV: admission_marker()})


def admission_marker():
    """<pid>:<start time> of this run: agent-build counts it while this run is an ancestor."""
    text = Path(f"/proc/{os.getpid()}/stat").read_text()
    return f"{os.getpid()}:{text[text.rindex(')') + 2 :].split()[19]}"


if __name__ == "__main__":
    try:
        sys.exit(main())
    except OSError as error:
        print(f"full-test-lock: {error}", file=sys.stderr)
        sys.exit(126)
    except KeyboardInterrupt:
        sys.exit(130)

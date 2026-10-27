#!/usr/bin/env python3
"""Run a full test suite while holding the lock every clone's full run shares.

Usage: full-test-lock.py LOCK_FOLDER HOLDER COMMAND [ARGUMENTS...]

Full runs in parallel clones loaded the machine enough to make timing tests flaky (research
96acba81), so they take turns. The kernel drops the lock when its holder dies: no stale lock.
"""

import errno
import fcntl
import os
from pathlib import Path
import subprocess
import sys

UNREACHABLE = {errno.EROFS, errno.EACCES, errno.EPERM, errno.ENOENT}


def open_lock(lock_folder):
    os.makedirs(lock_folder, mode=0o700, exist_ok=True)
    # Every sandbox writes this folder; a session that left a symlink here must not make the host
    # overwrite the file it points to.
    return os.open(lock_folder / "full-test.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)


def read_holder(lock):
    os.lseek(lock, 0, os.SEEK_SET)
    return os.read(lock, 4096).decode(errors="replace").strip() or "another run"


def main():
    if len(sys.argv) < 4:
        print("usage: full-test-lock.py LOCK_FOLDER HOLDER COMMAND [ARGUMENTS...]", file=sys.stderr)
        return 2
    lock_folder, holder, command = Path(sys.argv[1]), sys.argv[2], sys.argv[3:]
    try:
        lock = open_lock(lock_folder)
    except OSError as error:
        if error.errno not in UNREACHABLE:
            raise
        print(f"full-test-lock: cannot reach {lock_folder} ({error.strerror}); running without the "
              "test lock. Restart the session to share it.", file=sys.stderr, flush=True)
        return subprocess.call(command)
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print(f"Waiting for the test lock (held by {read_holder(lock)}) …", file=sys.stderr, flush=True)
        fcntl.flock(lock, fcntl.LOCK_EX)
    os.ftruncate(lock, 0)
    os.pwrite(lock, f"{holder}\n".encode(), 0)
    # The command does not inherit the lock: a daemon a test leaves behind must not hold it.
    return subprocess.call(command)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except OSError as error:
        print(f"full-test-lock: {error}", file=sys.stderr)
        sys.exit(126)
    except KeyboardInterrupt:
        sys.exit(130)

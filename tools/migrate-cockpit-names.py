#!/usr/bin/env python3
"""Host-only, inode-preserving migration of the cockpit's pre-rename paths."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import stat
import subprocess
import sys


def path_pairs():
    home = Path.home()
    state = Path(os.environ.get("COCKPIT_STATE_HOME", os.environ.get("XDG_STATE_HOME", home / ".local/state")))
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    pairs = [(state / "claude-remote", state / "cockpit-remote", "directory"),
             (state / "claude-stuck-exits", state / "cockpit-stuck-exits", "directory")]
    timers = Path(os.environ.get("XDG_DATA_HOME", home / ".local/share")) / "systemd/timers"
    pairs.append((timers / "stamp-claude-health-check.timer", timers / "stamp-cockpit-health-check.timer", "optional-file"))
    pairs.extend((home / ".cache/claude" / name, home / ".cache/cockpit" / name, "cache-directory")
                 for name in ["runs", "designs"])
    pairs.extend((runtime / f"claude-{name}", runtime / f"cockpit-{name}", "directory")
                 for name in ["report", "team-host", "answers", "runs", "requests"])
    pairs.extend((runtime / f"claude-{name}", runtime / f"cockpit-{name}", "file")
                 for name in ["todo-updates.jsonl", "todo-editor.lock"])
    return pairs


def check_paths(pairs):
    for old, new, kind in pairs:
        # No ancestor may redirect a host-side migration to a different tree.
        for path in (old, new):
            for parent in path.parents:
                if parent.is_symlink():
                    raise ValueError(f"unexpected symlink ancestor: {parent}")
        old_exists = old.exists() or old.is_symlink()
        new_exists = new.exists() or new.is_symlink()
        if old.is_symlink():
            if os.readlink(old) != str(new) or not new_exists:
                raise ValueError(f"unexpected legacy symlink: {old}")
        elif old_exists and new_exists and not (kind == "cache-directory" and new.is_symlink() and os.readlink(new) == str(old)):
            raise ValueError(f"migration conflict: both {old} and {new} exist")
        if new.is_symlink() and not (kind == "cache-directory" and os.readlink(new) == str(old) and old.is_dir() and not old.is_symlink()):
            raise ValueError(f"unexpected canonical symlink: {new}")
        for path in ([] if new.is_symlink() else [new]) + ([] if old.is_symlink() else [old]):
            if not path.exists() and not path.is_symlink():
                continue
            metadata = path.lstat()
            matches_kind = stat.S_ISDIR(metadata.st_mode) if kind in ["directory", "cache-directory"] else stat.S_ISREG(metadata.st_mode)
            if not matches_kind or metadata.st_uid != os.getuid():
                raise ValueError(f"unexpected type or owner: {path}")


def check_pending(pairs):
    for old, new, _ in pairs:
        if new.name != "cockpit-runs":
            continue
        for folder in [old, new]:
            if any(folder.glob("*.asking")):
                raise ValueError("in-flight approval: answer or decline it before migrating cockpit names")


def migrate(pairs):
    for old, new, kind in pairs:
        if old.is_symlink() or (kind == "cache-directory" and new.is_symlink()):
            continue
        if kind == "optional-file" and not old.exists() and not new.exists():
            continue
        new.parent.mkdir(parents=True, exist_ok=True)
        old.parent.mkdir(parents=True, exist_ok=True)
        if kind == "cache-directory" and old.exists():
            # Old sandboxes mount the cache's parent, not each child: moving a child would hide it.
            new.symlink_to(old, target_is_directory=True)
            continue
        if old.exists():
            old.rename(new)
        elif not new.exists():
            if kind in ["directory", "cache-directory"]:
                new.mkdir(mode=0o700)
            else:
                new.touch(mode=0o600)
        # Keep the moved directory inode: already-mounted sandbox folders see the same contents.
        old.symlink_to(new, target_is_directory=kind in ["directory", "cache-directory"])


UNITS = ["remote.service", "health-check.timer", "health-check.service"]


def systemctl(action, unit, query=False):
    output = subprocess.run(["systemctl", "--user", action, unit], capture_output=True, text=True)
    if query and output.returncode in [0, 1, 3, 4]:
        if action == "is-enabled":
            return output.returncode == 0 and output.stdout.strip() in ["enabled", "enabled-runtime"]
        return output.returncode == 0
    if output.returncode:
        raise ValueError(f"systemctl --user {action} {unit} failed: {output.stderr.strip()}")
    return True


def prepare_services(snapshot):
    if snapshot.is_symlink():
        raise ValueError(f"unexpected service snapshot symlink: {snapshot}")
    if snapshot.exists():
        return  # A failed apply keeps the original enabled/active intent for its retry.
    units = {}
    for suffix in UNITS:
        old, new = f"claude-{suffix}", f"cockpit-{suffix}"
        units[suffix] = {"active": systemctl("is-active", old, query=True) or systemctl("is-active", new, query=True),
                         "enabled": systemctl("is-enabled", old, query=True) or systemctl("is-enabled", new, query=True)}
    descriptor = os.open(snapshot, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(units, output)


def migrate_services(snapshot):
    metadata = snapshot.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise ValueError(f"unsafe service snapshot: {snapshot}")
    units = json.loads(snapshot.read_text())
    if set(units) != set(UNITS) or any(set(value) != {"active", "enabled"} or
            any(type(flag) is not bool for flag in value.values()) for value in units.values()):
        raise ValueError("invalid service migration snapshot")
    for suffix in UNITS:
        old, new = f"claude-{suffix}", f"cockpit-{suffix}"
        if systemctl("is-active", old, query=True):
            systemctl("stop", old)
        if systemctl("is-enabled", old, query=True):
            systemctl("disable", old)
        if units[suffix]["enabled"] and not systemctl("is-enabled", new, query=True):
            systemctl("enable", new)
        if units[suffix]["active"] and not systemctl("is-active", new, query=True):
            systemctl("start", new)
    snapshot.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["--check", "--apply", "--prepare", "--services"])
    # Treat the mode as a value, not an argparse option.
    args = parser.parse_args(["--", *sys.argv[1:]])
    pairs = path_pairs()
    # Serialize host migrations; this does not grant any sandbox access to private state.
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    with (runtime / "cockpit-name-migration.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        check_paths(pairs)
        if any(old.exists() and not old.is_symlink() and not new.is_symlink() for old, new, _ in pairs):
            check_pending(pairs)
        snapshot = runtime / "cockpit-name-services.json"
        if args.mode == "--prepare":
            prepare_services(snapshot)
        if args.mode == "--apply":
            migrate(pairs)
        if args.mode == "--services":
            migrate_services(snapshot)
    print("Cockpit path migration " + ("checked." if args.mode == "--check" else "applied."))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(f"cockpit migration: {error}", file=sys.stderr)
        sys.exit(1)

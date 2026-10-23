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
    pairs = [(state / "claude-remote", state / "cockpit-remote", "anchored-directory"),
             (state / "claude-stuck-exits", state / "cockpit-stuck-exits", "anchored-directory")]
    timers = Path(os.environ.get("XDG_DATA_HOME", home / ".local/share")) / "systemd/timers"
    pairs.append((timers / "stamp-claude-health-check.timer", timers / "stamp-cockpit-health-check.timer", "optional-file"))
    pairs.extend((home / ".cache/claude" / name, home / ".cache/cockpit" / name, "anchored-directory")
                 for name in ["runs", "designs"])
    pairs.extend((runtime / f"claude-{name}", runtime / f"cockpit-{name}", "directory")
                 for name in ["report", "team-host", "answers", "runs", "requests"])
    pairs.extend((runtime / f"claude-{name}", runtime / f"cockpit-{name}", "file")
                 for name in ["todo-editor.lock"])
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
        elif old_exists and new_exists and not (kind == "anchored-directory" and new.is_symlink() and os.readlink(new) == str(old)):
            raise ValueError(f"migration conflict: both {old} and {new} exist")
        if new.is_symlink() and not (kind == "anchored-directory" and os.readlink(new) == str(old) and old.is_dir() and not old.is_symlink()):
            raise ValueError(f"unexpected canonical symlink: {new}")
        for path in ([] if new.is_symlink() else [new]) + ([] if old.is_symlink() else [old]):
            if not path.exists() and not path.is_symlink():
                continue
            metadata = path.lstat()
            matches_kind = stat.S_ISDIR(metadata.st_mode) if kind in ["directory", "anchored-directory"] else stat.S_ISREG(metadata.st_mode)
            if not matches_kind or metadata.st_uid != os.getuid():
                raise ValueError(f"unexpected type or owner: {path}")


def check_guides():
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    for requests in {runtime / "claude-requests", runtime / "cockpit-requests"}:
        for guide in requests.glob("*.guide"):
            request = json.loads(guide.read_text())
            if not isinstance(request, dict):
                raise ValueError(f"invalid guide request: {guide.name}")
            owner = json.dumps({field: request.get(field) for field in ["session", "project", "title"]})
            key = request.get("answers", "")
            if not isinstance(key, str) or not key or len(key) > 255 or not all(character.isascii() and (character.isalnum() or character == "-") for character in key):
                raise ValueError(f"guide {guide.name} {owner} has no valid answer folder; resolve it before the daemon restart")
            if not any((runtime / name / key / "guides" / f"{guide.stem}.result").is_file()
                       for name in ["claude-answers", "cockpit-answers"]):
                raise ValueError(f"in-flight guide {guide.stem} {owner}: daemon restart deferred until it finishes or closes")


def check_queues():
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    for name in ["claude-todo-updates.jsonl", "cockpit-todo-updates.jsonl"]:
        path = runtime / name
        if path.exists() or path.is_symlink():
            metadata = path.lstat()
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid():
                raise ValueError(f"queue must be a regular file owned by this user, not a migration alias: {path}")


def migrate(pairs):
    for old, new, kind in pairs:
        if old.is_symlink() or (kind == "anchored-directory" and new.is_symlink()):
            continue
        if kind == "optional-file" and not old.exists() and not new.exists():
            continue
        new.parent.mkdir(parents=True, exist_ok=True)
        old.parent.mkdir(parents=True, exist_ok=True)
        if kind == "anchored-directory" and old.exists():
            # Existing mounts/masks protect the old location; a new physical sibling bypasses them.
            new.symlink_to(old, target_is_directory=True)
            continue
        if old.exists():
            old.rename(new)
        elif not new.exists():
            if kind in ["directory", "anchored-directory"]:
                new.mkdir(mode=0o700)
            else:
                new.touch(mode=0o600)
        # Keep the moved directory inode: already-mounted sandbox folders see the same contents.
        old.symlink_to(new, target_is_directory=kind in ["directory", "anchored-directory"])


UNITS = ["remote.service", "health-check.timer", "health-check.service"]


def unit_property(unit, name):
    output = subprocess.run(["systemctl", "--user", "show", unit, f"--property={name}", "--value"], capture_output=True, text=True)
    value = output.stdout.strip()
    if output.returncode and not (name == "LoadState" and value == "not-found" and output.returncode in [1, 4]):
        raise ValueError(f"cannot read {name} of {unit}: {output.stderr.strip()}")
    return value


def intends_to_run(unit):
    if unit_property(unit, "LoadState") == "not-found":
        return False
    return unit_property(unit, "ActiveState") in ["active", "activating", "reloading", "refreshing"]


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
        units[suffix] = {"active": intends_to_run(old) or intends_to_run(new),
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
        if unit_property(old, "LoadState") != "not-found":
            systemctl("stop", old)
        if systemctl("is-enabled", old, query=True):
            systemctl("disable", old)
        if units[suffix]["enabled"] and not systemctl("is-enabled", new, query=True):
            systemctl("enable", new)
        if units[suffix]["active"] and not intends_to_run(new):
            systemctl("start", new)
    snapshot.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["--check", "--apply", "--prepare", "--services", "--check-guides"])
    # Treat the mode as a value, not an argparse option.
    args = parser.parse_args(["--", *sys.argv[1:]])
    if args.mode == "--check-guides":
        # Path migration preserves inodes; only a daemon restart would forget the live guide.
        check_guides()
        return
    pairs = path_pairs()
    # Serialize host migrations; this does not grant any sandbox access to private state.
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    with (runtime / "cockpit-name-migration.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        check_paths(pairs)
        check_queues()
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

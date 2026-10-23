#!/usr/bin/env python3
"""Host-only, inode-preserving migration of the cockpit's pre-rename paths."""

import argparse
import fcntl
import os
from pathlib import Path
import stat
import sys


def path_pairs():
    home = Path.home()
    state = Path(os.environ.get("COCKPIT_STATE_HOME", os.environ.get("XDG_STATE_HOME", home / ".local/state")))
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    pairs = [(state / "claude-remote", state / "cockpit-remote", "directory"),
             (state / "claude-stuck-exits", state / "cockpit-stuck-exits", "directory")]
    pairs.extend((home / ".cache/claude" / name, home / ".cache/cockpit" / name, "directory")
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
        elif old_exists and new_exists:
            raise ValueError(f"migration conflict: both {old} and {new} exist")
        for path in [new] + ([] if old.is_symlink() else [old]):
            if not path.exists() and not path.is_symlink():
                continue
            metadata = path.lstat()
            matches_kind = stat.S_ISDIR(metadata.st_mode) if kind == "directory" else stat.S_ISREG(metadata.st_mode)
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
        if old.is_symlink():
            continue
        new.parent.mkdir(parents=True, exist_ok=True)
        old.parent.mkdir(parents=True, exist_ok=True)
        if old.exists():
            old.rename(new)
        elif not new.exists():
            if kind == "directory":
                new.mkdir(mode=0o700)
            else:
                new.touch(mode=0o600)
        # Keep the moved directory inode: already-mounted sandbox folders see the same contents.
        old.symlink_to(new, target_is_directory=kind == "directory")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["--check", "--apply"])
    # Treat the mode as a value, not an argparse option.
    args = parser.parse_args(["--", *sys.argv[1:]])
    pairs = path_pairs()
    # Serialize host migrations; this does not grant any sandbox access to private state.
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    with (runtime / "cockpit-name-migration.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        check_paths(pairs)
        check_pending(pairs)
        if args.mode == "--apply":
            migrate(pairs)
    print("Cockpit path migration " + ("checked." if args.mode == "--check" else "applied."))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(f"cockpit migration: {error}", file=sys.stderr)
        sys.exit(1)

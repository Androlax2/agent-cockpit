#!/usr/bin/env python3
"""Host-only, append-only trust for fresh Cockpit clones and reviewed Codex hooks."""

import argparse
import fcntl
import hashlib
import json
import os
import stat
import sys
import tomllib
from pathlib import Path

# Codex rust-v0.161.0: hooks/src/lib.rs, engine/discovery.rs and config/src/fingerprint.rs.
REVIEWED_EVENTS = {
    "SessionStart": ("session_start", 5),
    "UserPromptSubmit": ("user_prompt_submit", 5),
    "PermissionRequest": ("permission_request", 5),
    "PostToolUse": ("post_tool_use", 5),
    "Stop": ("stop", 5),
    "Interrupt": ("interrupt", 3),
    "SessionEnd": ("session_end", 3),
}


def table(settings, *keys):
    for key in keys:
        settings = settings.get(key, {})
        if not isinstance(settings, dict):
            raise ValueError(f"expected a TOML table at {keys!r}")
    return settings


def append_entry(settings, keys, field, value):
    parent = table(settings, *keys[:-1])
    if keys[-1] in parent:
        if table(parent, keys[-1]).get(field) == value:
            return ""
        raise ValueError(f"existing trust entry needs native host review: {keys[-1]}")
    header = ".".join(json.dumps(key, ensure_ascii=False) for key in keys)
    return f"\n[{header}]\n{field} = {json.dumps(value, ensure_ascii=False)}\n"


def inherit(settings, source, clone):
    source = str(Path(source).resolve(strict=True))
    clone = str(Path(clone).resolve(strict=True))
    projects = table(settings, "projects")
    if table(projects, source).get("trust_level") != "trusted":
        return ""
    # A prior destination decision is Theo's, including an explicit distrust.
    if "trust_level" in table(projects, clone):
        return ""
    return append_entry(settings, ("projects", clone), "trust_level", "trusted")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate key in reviewed hook file: {key}")
        result[key] = value
    return result


def hook_trust(settings, codex_home):
    hooks_file = codex_home / "hooks.json"
    if not hooks_file.exists():
        return ""
    hooks = json.loads(hooks_file.read_text(), object_pairs_hook=unique_object)
    reviewed = {
        event: [
            {"hooks": [{"type": "command", "command": "cockpit-codex-hook", "timeout": timeout}]}
        ]
        for event, (_, timeout) in REVIEWED_EVENTS.items()
    }
    if (
        not isinstance(hooks, dict)
        or set(hooks) - {"description", "hooks"}
        or hooks.get("hooks") != reviewed
    ):
        raise ValueError(
            "user hooks differ from the seven reviewed Cockpit hooks; host review required"
        )
    if set(table(settings, "hooks")) - {"state"}:
        raise ValueError(
            "TOML hook definitions require host review; cannot verify hooks.json precedence"
        )
    additions = []
    for event, timeout in REVIEWED_EVENTS.values():
        key = f"{hooks_file}:{event}:0:0"
        if table(settings, "hooks", "state", key).get("enabled") is False:
            continue
        identity = {
            "event_name": event,
            "hooks": [
                {
                    "type": "command",
                    "command": "cockpit-codex-hook",
                    "timeout": timeout,
                    "async": False,
                }
            ],
        }
        canonical = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
        digest = "sha256:" + hashlib.sha256(canonical).hexdigest()
        additions.append(append_entry(settings, ("hooks", "state", key), "trusted_hash", digest))
    return "".join(additions)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    clone = commands.add_parser("inherit")
    clone.add_argument("source")
    clone.add_argument("clone")
    commands.add_parser("hooks")
    args = parser.parse_args()
    codex_home = Path.home() / ".codex"
    if os.environ.get("CODEX_HOME") and Path(os.environ["CODEX_HOME"]) != codex_home:
        raise ValueError("custom CODEX_HOME requires host review")
    config = codex_home / "config.toml"
    try:
        stream = config.open("r+b")
    except FileNotFoundError:
        # There can be no trusted source without a user config. Do not bootstrap trust.
        return
    with stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 1024 * 1024:
            raise ValueError("config must be a regular file of at most 1 MiB")
        original = stream.read()
        settings = tomllib.loads(original.decode())
        addition = (
            inherit(settings, args.source, args.clone)
            if args.command == "inherit"
            else hook_trust(settings, codex_home)
        ).encode()
        if not addition:
            return
        tomllib.loads((original + addition).decode())
        stream.seek(0)
        if not os.path.samestat(metadata, config.stat()) or stream.read() != original:
            raise ValueError("config changed during trust preparation; retry on the host")
        # Bind mounts keep this inode. Append validated tables, never replace or pre-truncate.
        stream.write(addition)
        stream.flush()
        os.fsync(stream.fileno())
        if not os.path.samestat(metadata, config.stat()):
            raise ValueError("config was replaced during trust preparation; retry on the host")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(f"cockpit: cannot prepare Codex trust: {error}", file=sys.stderr)
        sys.exit(1)

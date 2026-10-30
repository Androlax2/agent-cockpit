#!/usr/bin/env python3
"""Write a package-path policy proposal, never a live policy update."""

import argparse
import copy
import json
import os
from pathlib import Path, PurePosixPath

PACKAGE_ROOT = "/usr/lib/agent-cockpit"
RULES = ("allowRead", "allowWrite", "denyRead", "denyWrite")


def unique_fields(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate key: {key}")
        result[key] = value
    return result


def translate(policy, checkout_root):
    if not isinstance(policy, dict) or not isinstance(policy.get("filesystem"), dict):
        raise ValueError("policy must contain a filesystem object")
    root = PurePosixPath(checkout_root)
    if not root.is_absolute() or root.name != "agent-cockpit" or ".." in root.parts:
        raise ValueError("checkout root must be an absolute agent-cockpit path")
    prefixes = ("~/agent-cockpit", str(root))
    proposed = copy.deepcopy(policy)
    for rule in RULES:
        paths = proposed["filesystem"].get(rule)
        if paths is None and rule not in proposed["filesystem"]:
            continue
        if not isinstance(paths, list) or any(not isinstance(path, str) for path in paths):
            raise ValueError(f"{rule} must be a list of paths")
        for index, path in enumerate(paths):
            for prefix in prefixes:
                if path != prefix and not path.startswith(prefix + "/"):
                    continue
                suffix = path[len(prefix) :].lstrip("/")
                parts = PurePosixPath(suffix).parts
                if ".." in parts:
                    raise ValueError("parent traversal requires separate review")
                if rule.startswith("allow") and (
                    not parts or any(character in parts[0] for character in "*?[]{}\\")
                ):
                    raise ValueError("whole-root grant requires separate review")
                paths[index] = PACKAGE_ROOT + ("/" + suffix if suffix else "")
                break
    return proposed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--checkout-root", default=str(Path.home() / "agent-cockpit"))
    arguments = parser.parse_args()
    try:
        lines = arguments.policy.read_text().splitlines(keepends=True)
        header = []
        while lines and (not lines[0].strip() or lines[0].lstrip().startswith("//")):
            header.append(lines.pop(0))
        policy = json.loads("".join(lines), object_pairs_hook=unique_fields)
        proposed = translate(policy, arguments.checkout_root)
        serialized = "".join(header) + json.dumps(proposed, indent=2, allow_nan=False) + "\n"
        descriptor = os.open(arguments.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as stream:
            stream.write(serialized)
    except (OSError, ValueError) as error:
        parser.exit(2, f"cockpit policy preview: {error}\n")
    print(f"Proposal written to {arguments.output}; no live policy changed.")


if __name__ == "__main__":
    main()

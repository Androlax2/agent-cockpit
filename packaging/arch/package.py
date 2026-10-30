#!/usr/bin/env python3
"""Verify and stage a prebuilt Cockpit payload; never install or activate it."""

import argparse
import hashlib
import json
import os
import re
import shutil
import tempfile
from pathlib import Path, PurePosixPath

DIRECTORY = Path(__file__).resolve().parent
PRIVATE_ROOT = Path("usr/lib/agent-cockpit")
COMPONENTS = {
    "greywall": "7c06d51afd26cbc90a32c82375a393570c22ffc7",
    "greyproxy": "cdd5cde76dce1262c0fd8d6b1ab2bd9953df5e71",
}
BINARIES = (
    "ask-to-guide",
    "ask-to-run",
    "claude-audit",
    "cockpit-codex-hook",
    "cockpit-demo",
    "cockpit-design",
    "cockpit-fade",
    "cockpit-failsafe",
    "cockpit-guide",
    "cockpit-remote",
    "cockpit-run-approve",
    "cockpit-sidebar",
    "cockpit-subscription",
    "cockpit-team",
    "cockpit-todo",
    "cockpit-idle-prompt",
    "osd",
)
ASSETS = (
    "assets/osd/style.css",
    "assets/idle/style.css",
    *[
        f"assets/icons/{name}.svg"
        for name in (
            "volume-mute",
            "volume-low",
            "volume-mid",
            "volume-high",
            "bell-dot",
            "bell",
            "brightness",
        )
    ],
)
LICENSES = (
    "LICENSE",
    "THIRD_PARTY_NOTICES.txt",
    "components/licenses/greywall/LICENSE",
    "components/licenses/greyproxy/LICENSE",
    "components/licenses/tun2socks/LICENSE",
)
APACHE_LICENSE_SHA256 = "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"


def unique_fields(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate key: {key}")
        result[key] = value
    return result


def read_sources(name):
    paths = (DIRECTORY / name).read_text().splitlines()
    if len(paths) != len(set(paths)):
        raise ValueError(f"duplicate source in {name}")
    for path in paths:
        relative = PurePosixPath(path)
        if (
            not path.startswith("config/")
            or relative.is_absolute()
            or ".." in relative.parts
            or path.startswith("config/greywall/")
            or ".wants/" in path
        ):
            raise ValueError(f"unsafe source in {name}: {path}")
    return paths


def checked_path(payload, relative):
    path = payload / relative
    current = payload
    for part in PurePosixPath(relative).parts:
        current = current / part
        if current.is_symlink():
            raise ValueError(f"payload symlink is not allowed: {relative}")
    if not path.is_file():
        raise ValueError(f"missing required payload file: {relative}")
    return path


def stage(payload, destination, version, source):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+[a-zA-Z0-9._+]*", version):
        raise ValueError("invalid package version")
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("invalid source commit")
    if payload.is_symlink():
        raise ValueError("payload symlink is not allowed")
    payload = payload.resolve(strict=True)
    if destination.is_symlink() or (destination.exists() and any(destination.iterdir())):
        raise ValueError("destination is not empty or is a symlink")
    runtime = read_sources("runtime-files.txt")
    integrations = read_sources("integration-files.txt")
    binaries = [f"apps/target/release/{name}" for name in BINARIES]
    components = [f"components/{name}" for name in COMPONENTS]
    required = [*runtime, *integrations, *binaries, *components, *ASSETS, *LICENSES]
    if len(required) != len(set(required)):
        raise ValueError("duplicate payload destination")
    metadata_path = checked_path(payload, "release.json")
    metadata_bytes = metadata_path.read_bytes()
    metadata = json.loads(metadata_bytes, object_pairs_hook=unique_fields)
    if not isinstance(metadata, dict):
        raise ValueError("release metadata must be an object")
    if set(metadata) != {"version", "source", "target", "components", "files"}:
        raise ValueError("unexpected release metadata")
    if (
        metadata.get("version") != version
        or metadata.get("source") != source
        or metadata.get("target") != "x86_64-unknown-linux-gnu"
    ):
        raise ValueError("release identity mismatch")
    if metadata.get("components") != COMPONENTS:
        raise ValueError("component identity mismatch")
    checksums = metadata.get("files")
    if not isinstance(checksums, dict) or set(checksums) != set(required):
        raise ValueError("unexpected payload inventory")
    for relative in required:
        path = checked_path(payload, relative)
        expected = checksums[relative]
        if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected):
            raise ValueError(f"invalid checksum: {relative}")
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
        if actual != expected:
            raise ValueError(f"checksum mismatch: {relative}")
    if checksums["LICENSE"] != APACHE_LICENSE_SHA256:
        raise ValueError("project license does not match approved Apache-2.0 text")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cockpit-package-", dir=destination.parent) as folder:
        staged = Path(folder) / "root"
        staged.mkdir()
        for relative in required:
            original = payload / relative
            if relative in integrations:
                target = Path("usr/share/agent-cockpit/integrations") / relative
            elif relative in LICENSES:
                target = Path("usr/share/licenses/agent-cockpit-bin") / relative
            else:
                target = PRIVATE_ROOT / relative
            installed = staged / target
            installed.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original, installed)
            with installed.open("rb") as stream:
                copied = hashlib.file_digest(stream, "sha256").hexdigest()
            if copied != checksums[relative]:
                raise ValueError(f"payload changed during staging: {relative}")
            installed.chmod(
                0o755
                if relative in binaries + components or original.stat().st_mode & 0o111
                else 0o644
            )
            public = PurePosixPath(relative)
            if str(public.parent) == "config/bin/.local/bin":
                if public.name in {"claude", "codex", "opencode", "wl-paste"}:
                    raise ValueError(
                        f"package must not replace provider/clipboard command: {public.name}"
                    )
                link = staged / "usr/bin" / public.name
                link.parent.mkdir(parents=True, exist_ok=True)
                link.symlink_to(Path("../lib/agent-cockpit") / relative)
        (staged / PRIVATE_ROOT / "release.json").write_bytes(metadata_bytes)
        os.replace(staged, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--payload", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source", required=True)
    arguments = parser.parse_args()
    try:
        stage(arguments.payload, arguments.destination, arguments.version, arguments.source)
    except (OSError, ValueError) as error:
        parser.exit(2, f"cockpit package: {error}\n")
    print(f"Staged {arguments.version} from {arguments.source}; no activation performed.")


if __name__ == "__main__":
    main()

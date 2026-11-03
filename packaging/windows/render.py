#!/usr/bin/env python3
"""Render the torlek Scoop manifest for one published release (docs/packaging/windows.md, #565).

Usage: render.py render --tag 0.1.0-alpha.1 --sums SHA256SUMS --out DIR [--repo PATH]

Fills packaging/windows/scoop/torlek.json.in with the tag, the linux-x86_64 archive's checksum
from the release's SHA256SUMS, the checksum of that SHA256SUMS file itself, and the checksums of
the launcher scripts as the v<tag> git tag holds them, since Scoop downloads them from that tag.
Nothing here downloads, installs or publishes.
"""

import argparse
import hashlib
import importlib.util
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = Path(__file__).resolve().parent / "scoop/torlek.json.in"
RELEASE_TOOL = ROOT / "tools/release.py"
# WSL 2 is proven only for this archive (release.yml's install-wsl2 job).
PLATFORM = "linux-x86_64"
# The files Scoop downloads from the tagged source, keyed by their template token.
LAUNCHER_FILES = {
    "@SHA256_TORLEK_PS1@": "packaging/windows/torlek.ps1",
    "@SHA256_TORLEK_WSL_PS1@": "packaging/windows/torlek-wsl.ps1",
    "@SHA256_INSTALL_IN_WSL_SH@": "packaging/windows/install-in-wsl.sh",
}
SUMS_LINE = re.compile(r"([0-9a-f]{64})  (\S+)")


class Refusal(Exception):
    pass


def release_tool():
    spec = importlib.util.spec_from_file_location("release", RELEASE_TOOL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def checked_tag(tag):
    """Scoop's version is the tag itself: its Compare-Version orders alpha < beta < stable."""
    release = release_tool()
    try:
        channel = release.parse_tag(f"v{tag}")["channel"]
    except release.Refusal as error:
        raise Refusal(f"{tag!r}: not a release tag, given without its v: {error}") from error
    if channel == "stable":
        raise Refusal(f"{tag!r}: no stable release exists to package")
    return tag


def archive_checksum(tag, sums_text):
    checksums = {}
    for number, line in enumerate(sums_text.splitlines(), start=1):
        match = SUMS_LINE.fullmatch(line)
        if not match:
            raise Refusal(f"SHA256SUMS line {number}: not `<sha256>  <file>`: {line!r}")
        digest, name = match.groups()
        if name in checksums:
            raise Refusal(f"SHA256SUMS: {name} is listed twice")
        checksums[name] = digest
    name = f"torlek-v{tag}-{PLATFORM}.tar.gz"
    if name not in checksums:
        raise Refusal(f"SHA256SUMS has no {name}: is it the v{tag} release's file?")
    return checksums[name]


def tagged_file(repo, tag, path):
    result = subprocess.run(
        ["git", "-C", str(repo), "show", f"refs/tags/v{tag}:{path}"],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise Refusal(
            f"{path} at tag v{tag} is not readable in {repo}; fetch the tag first: "
            f"{result.stderr.decode(errors='replace').strip()}"
        )
    return result.stdout


def manifest(tag, sums_bytes, repo=ROOT):
    tag = checked_tag(tag)
    values = {
        "@SHA256_ARCHIVE@": archive_checksum(tag, sums_bytes.decode()),
        "@SHA256_SUMS@": hashlib.sha256(sums_bytes).hexdigest(),
    }
    for token, path in LAUNCHER_FILES.items():
        values[token] = hashlib.sha256(tagged_file(repo, tag, path)).hexdigest()
    text = TEMPLATE.read_text()
    for token, value in values.items():
        if text.count(token) != 1:
            raise Refusal(f"{TEMPLATE}: expected {token} exactly once")
        text = text.replace(token, value)
    if "@TAG@" not in text:
        raise Refusal(f"{TEMPLATE}: @TAG@ is missing")
    text = text.replace("@TAG@", tag)
    leftover = re.search(r"@[A-Z0-9_]+@", text)
    if leftover:
        raise Refusal(f"{TEMPLATE}: unknown token {leftover.group()}")
    json.loads(text)
    return text


def render(tag, sums, out, repo=ROOT):
    target = out / "torlek.json"
    if target.exists():
        raise Refusal(f"{target} exists; render into a directory without a manifest")
    text = manifest(tag, sums.read_bytes(), repo)
    out.mkdir(parents=True, exist_ok=True)
    target.write_text(text)
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    render_command = commands.add_parser("render")
    render_command.add_argument("--tag", required=True)
    render_command.add_argument("--sums", type=Path, required=True)
    render_command.add_argument("--out", type=Path, required=True)
    render_command.add_argument("--repo", type=Path, default=ROOT)
    arguments = parser.parse_args()
    try:
        target = render(arguments.tag, arguments.sums, arguments.out, arguments.repo)
        print(f"Rendered the torlek {arguments.tag} Scoop manifest into {target}")
    except (OSError, UnicodeDecodeError, Refusal) as error:
        parser.exit(1, f"render.py: {error}\n")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Render the torlek-bin AUR recipe for one published release (docs/packaging/aur.md, #562).

Usage: render.py render --tag 0.1.0-alpha.1 --sums SHA256SUMS --out DIR
       render.py check-srcinfo DIR

`render` fills packaging/aur/torlek-bin/PKGBUILD.in with the tag and the two Linux archive
checksums from the release's SHA256SUMS, then writes .SRCINFO with `makepkg --printsrcinfo`.
`check-srcinfo` refuses a directory whose .SRCINFO no longer matches its PKGBUILD.
Nothing here downloads, builds, installs or publishes.
"""

import argparse
import re
import subprocess
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parent / "torlek-bin/PKGBUILD.in"
# The Alpha1 plan's archives (tools/release.py ALPHA1_PLATFORMS), keyed by pacman's CARCH.
ARCHITECTURES = {"x86_64": "linux-x86_64", "aarch64": "linux-aarch64"}
# Only prerelease tags exist until Theo decides a stable channel (docs/RELEASING.md#tags).
TAG_PATTERN = re.compile(
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)-(?:alpha|beta)\.[1-9][0-9]*"
)
SUMS_LINE = re.compile(r"([0-9a-f]{64})  (\S+)")


class Refusal(Exception):
    pass


def package_version(tag):
    """pacman's vercmp puts `0.1.0alpha.1` before `0.1.0beta.1` before `0.1.0`; a hyphen
    is not allowed in pkgver, and `0.1.0_alpha.1` or `0.1.0.alpha.1` would sort after 0.1.0."""
    if not TAG_PATTERN.fullmatch(tag):
        raise Refusal(f"{tag!r}: not an X.Y.Z-alpha.N or X.Y.Z-beta.N release tag (without v)")
    return tag.replace("-", "", 1)


def archive_checksums(tag, sums_text):
    checksums = {}
    for number, line in enumerate(sums_text.splitlines(), start=1):
        match = SUMS_LINE.fullmatch(line)
        if not match:
            raise Refusal(f"SHA256SUMS line {number}: not `<sha256>  <file>`: {line!r}")
        digest, name = match.groups()
        if name in checksums:
            raise Refusal(f"SHA256SUMS: {name} is listed twice")
        checksums[name] = digest
    selected = {}
    for architecture, platform in ARCHITECTURES.items():
        name = f"torlek-v{tag}-{platform}.tar.gz"
        if name not in checksums:
            raise Refusal(f"SHA256SUMS has no {name}: is it the v{tag} release's file?")
        selected[architecture] = checksums[name]
    return selected


def recipe(tag, sums_text):
    checksums = archive_checksums(tag, sums_text)
    values = {
        "@TAG@": tag,
        "@PKGVER@": package_version(tag),
        "@SHA256_X86_64@": checksums["x86_64"],
        "@SHA256_AARCH64@": checksums["aarch64"],
    }
    text = TEMPLATE.read_text()
    for token, value in values.items():
        if text.count(token) != 1:
            raise Refusal(f"{TEMPLATE}: expected {token} exactly once")
        text = text.replace(token, value)
    leftover = re.search(r"@[A-Z0-9_]+@", text)
    if leftover:
        raise Refusal(f"{TEMPLATE}: unknown token {leftover.group()}")
    return text


def printed_srcinfo(directory):
    try:
        result = subprocess.run(
            ["makepkg", "--printsrcinfo"],
            cwd=directory,
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError as error:
        raise Refusal("makepkg is missing: render and check .SRCINFO on Arch Linux") from error
    if result.returncode != 0:
        raise Refusal(f"makepkg --printsrcinfo failed: {result.stderr.strip()}")
    return result.stdout


def render(tag, sums, out):
    out.mkdir(parents=True, exist_ok=True)
    for name in ("PKGBUILD", ".SRCINFO"):
        if (out / name).exists():
            raise Refusal(f"{out / name} exists; render into a directory without a recipe")
    (out / "PKGBUILD").write_text(recipe(tag, sums.read_text()))
    (out / ".SRCINFO").write_text(printed_srcinfo(out))


def check_srcinfo(directory):
    recorded = (directory / ".SRCINFO").read_text()
    if recorded != printed_srcinfo(directory):
        raise Refusal(f"{directory}/.SRCINFO does not match its PKGBUILD; render it again")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    render_command = commands.add_parser("render")
    render_command.add_argument("--tag", required=True)
    render_command.add_argument("--sums", type=Path, required=True)
    render_command.add_argument("--out", type=Path, required=True)
    check_command = commands.add_parser("check-srcinfo")
    check_command.add_argument("directory", type=Path)
    arguments = parser.parse_args()
    try:
        if arguments.command == "render":
            render(arguments.tag, arguments.sums, arguments.out)
            print(f"Rendered torlek-bin {package_version(arguments.tag)} into {arguments.out}")
        else:
            check_srcinfo(arguments.directory)
            print(f"{arguments.directory}/.SRCINFO matches its PKGBUILD")
    except (OSError, Refusal) as error:
        parser.exit(1, f"render.py: {error}\n")


if __name__ == "__main__":
    main()

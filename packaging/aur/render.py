#!/usr/bin/env python3
"""Render the torlek-bin AUR recipe for one published release (docs/packaging/aur.md, #562).

Usage: render.py render --tag 0.1.0-alpha.1 --sums SHA256SUMS --out DIR [--pkgrel N]
                        [--development]
       render.py check-srcinfo DIR

`render` fills packaging/aur/torlek-bin/PKGBUILD.in with the tag and the two Linux archive
checksums from the release's SHA256SUMS, then writes .SRCINFO with `makepkg --printsrcinfo`
and copies the recipe's install script, torlek-bin.install, beside them.
`check-srcinfo` refuses a directory whose .SRCINFO no longer matches its PKGBUILD.
`--development` takes a dry run's `X.Y.Z-dev.N` instead, for the release workflow's Arch
install check; such a recipe names no published release and is never submitted.
Nothing here downloads, builds, installs or publishes.
"""

import argparse
import importlib.util
import re
import subprocess
import tempfile
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parent / "torlek-bin/PKGBUILD.in"
# The recipe's `install=` script, published beside it unchanged.
INSTALL_SCRIPT = TEMPLATE.parent / "torlek-bin.install"
RELEASE_TOOL = Path(__file__).resolve().parents[2] / "tools/release.py"
# The Alpha1 plan's archives (tools/release.py ALPHA1_PLATFORMS), keyed by pacman's CARCH.
ARCHITECTURES = {"x86_64": "linux-x86_64", "aarch64": "linux-aarch64"}
SUMS_LINE = re.compile(r"([0-9a-f]{64})  (\S+)")


class Refusal(Exception):
    pass


def release_tool():
    spec = importlib.util.spec_from_file_location("release", RELEASE_TOOL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def package_version(tag, is_development=False):
    """pacman's vercmp puts `0.1.0alpha.1` before `0.1.0beta.1` before `0.1.0`; a hyphen
    is not allowed in pkgver, and `0.1.0_alpha.1` or `0.1.0.alpha.1` would sort after 0.1.0.
    The tag grammar is release.py's; only prereleases exist (docs/RELEASING.md#tags)."""
    release = release_tool()
    if is_development:
        # release.py dry-run-version's shape: the workspace core, then the commits since the base.
        if not re.fullmatch(rf"{release.CORE_PATTERN}-dev\.(?:0|[1-9][0-9]*)", tag):
            raise Refusal(f"{tag!r}: --development takes a dry run's X.Y.Z-dev.N")
        return tag.replace("-", "", 1)
    try:
        channel = release.parse_tag(f"v{tag}")["channel"]
    except release.Refusal as error:
        raise Refusal(f"{tag!r}: not a release tag, given without its v: {error}") from error
    if channel == "stable":
        raise Refusal(f"{tag!r}: no stable release exists to package")
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


def recipe(tag, sums_text, pkgrel=1, is_development=False):
    pkgver = package_version(tag, is_development)
    if pkgrel < 1:
        raise Refusal(f"pkgrel {pkgrel}: starts at 1")
    checksums = archive_checksums(tag, sums_text)
    values = {
        "@TAG@": tag,
        "@PKGVER@": pkgver,
        "@PKGREL@": str(pkgrel),
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


def render(tag, sums, out, pkgrel=1, is_development=False):
    """The files appear together or not at all: a failed makepkg leaves nothing to retry around."""
    for name in ("PKGBUILD", ".SRCINFO"):
        if (out / name).exists():
            raise Refusal(f"{out / name} exists; render into a directory without a recipe")
    # The committed folder holds the install script itself: rendering there keeps it.
    script = out / INSTALL_SCRIPT.name
    if script.exists() and script.read_bytes() != INSTALL_SCRIPT.read_bytes():
        raise Refusal(f"{script} exists and is not {INSTALL_SCRIPT}; render elsewhere")
    text = recipe(tag, sums.read_text(), pkgrel, is_development)
    out.mkdir(parents=True, exist_ok=True)
    pkgver = package_version(tag, is_development)
    with tempfile.TemporaryDirectory(prefix="torlek-aur-", dir=out) as folder:
        staged = Path(folder)
        (staged / "PKGBUILD").write_text(text)
        (staged / INSTALL_SCRIPT.name).write_text(INSTALL_SCRIPT.read_text())
        (staged / ".SRCINFO").write_text(printed_srcinfo(staged))
        (staged / INSTALL_SCRIPT.name).replace(out / INSTALL_SCRIPT.name)
        (staged / ".SRCINFO").replace(out / ".SRCINFO")
        (staged / "PKGBUILD").replace(out / "PKGBUILD")
    return f"{pkgver}-{pkgrel}"


def check_srcinfo(directory):
    """Also the last check before an AUR push, so a --development recipe never passes it."""
    recorded = (directory / ".SRCINFO").read_text()
    if re.search(r"^\tpkgver = \S*dev\.", recorded, re.MULTILINE):
        raise Refusal(f"{directory}: a --development recipe names no release; never submit it")
    if recorded != printed_srcinfo(directory):
        raise Refusal(f"{directory}/.SRCINFO does not match its PKGBUILD; render it again")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    render_command = commands.add_parser("render")
    render_command.add_argument("--tag", required=True)
    render_command.add_argument("--sums", type=Path, required=True)
    render_command.add_argument("--out", type=Path, required=True)
    render_command.add_argument("--pkgrel", type=int, default=1)
    render_command.add_argument("--development", action="store_true")
    check_command = commands.add_parser("check-srcinfo")
    check_command.add_argument("directory", type=Path)
    arguments = parser.parse_args()
    try:
        if arguments.command == "render":
            version = render(
                arguments.tag,
                arguments.sums,
                arguments.out,
                arguments.pkgrel,
                arguments.development,
            )
            print(f"Rendered torlek-bin {version} into {arguments.out}")
        else:
            check_srcinfo(arguments.directory)
            print(f"{arguments.directory}/.SRCINFO matches its PKGBUILD")
    except (OSError, Refusal) as error:
        parser.exit(1, f"render.py: {error}\n")


if __name__ == "__main__":
    main()

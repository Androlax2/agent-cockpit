#!/usr/bin/env python3
"""Render the torlek Scoop manifest for one published release (docs/packaging/windows.md, #565).

Usage: render.py render --tag 0.1.0-alpha.1 --sums SHA256SUMS --out DIR [--repo PATH]
       render.py rehearse --tag 0.1.0-dev.4 --sums SHA256SUMS --archive TARBALL --revision SHA
                          --url http://127.0.0.1:PORT --out DIR [--repo PATH]

`render` fills packaging/windows/scoop/torlek.json.in with the tag, the linux-x86_64 archive's
checksum from the release's SHA256SUMS, the checksum of that SHA256SUMS file itself, and the
checksums of the launcher scripts as the v<tag> git tag holds them, since Scoop downloads them
from that tag.

`rehearse` is the release workflow's dry run (install-scoop): a dry run has no tag and no
GitHub Release, so it renders the same template for a `-dev.N` version, hashes the launchers
as the given revision holds them, points every download at a loopback server, and lays out
what that server serves under DIR (release/ and source/). The manifest it writes cannot be
published: its URLs are loopback only.

Nothing here downloads, installs or publishes.
"""

import argparse
import hashlib
import importlib.util
import json
import re
import shutil
import subprocess
from collections import namedtuple
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
# The two URL prefixes of the template; a rehearsal serves them from its own loopback server.
RELEASE_DOWNLOADS = "https://github.com/Androlax2/agent-cockpit/releases/download/v@TAG@"
TAGGED_SOURCE = "https://raw.githubusercontent.com/Androlax2/agent-cockpit/v@TAG@"
LOOPBACK_URL = re.compile(r"http://(?:127\.0\.0\.1|localhost):[0-9]+")

Rehearsal = namedtuple("Rehearsal", ["revision", "url"])


class Refusal(Exception):
    pass


def release_tool():
    spec = importlib.util.spec_from_file_location("release", RELEASE_TOOL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def checked_tag(tag, is_development=False):
    """Scoop's version is the tag itself: its Compare-Version orders alpha < beta < stable."""
    release = release_tool()
    if is_development:
        # release.py dry-run-version's shape: the workspace core, then the commits since the base.
        if not re.fullmatch(rf"{release.CORE_PATTERN}-dev\.(?:0|[1-9][0-9]*)", tag):
            raise Refusal(f"{tag!r}: a rehearsal takes a dry run's X.Y.Z-dev.N")
        return tag
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


def committed_file(repo, ref, path, hint):
    result = subprocess.run(
        ["git", "-C", str(repo), "show", f"{ref}:{path}"],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise Refusal(
            f"{path} at {ref} is not readable in {repo}; {hint}: "
            f"{result.stderr.decode(errors='replace').strip()}"
        )
    return result.stdout


def launcher_file(repo, tag, path, rehearsal):
    if rehearsal:
        return committed_file(repo, rehearsal.revision, path, "fetch the revision first")
    return committed_file(repo, f"refs/tags/v{tag}", path, "fetch the tag first")


def with_rehearsal_urls(text, rehearsal):
    if not LOOPBACK_URL.fullmatch(rehearsal.url):
        raise Refusal(f"{rehearsal.url!r}: a rehearsal serves from http://127.0.0.1:PORT only")
    for prefix, folder in ((RELEASE_DOWNLOADS, "release"), (TAGGED_SOURCE, "source")):
        if prefix not in text:
            raise Refusal(f"{TEMPLATE}: expected downloads from {prefix}")
        text = text.replace(prefix, f"{rehearsal.url}/{folder}")
    return text


def manifest(tag, sums_bytes, repo=ROOT, rehearsal=None):
    tag = checked_tag(tag, is_development=rehearsal is not None)
    values = {
        "@SHA256_ARCHIVE@": archive_checksum(tag, sums_bytes.decode()),
        "@SHA256_SUMS@": hashlib.sha256(sums_bytes).hexdigest(),
    }
    for token, path in LAUNCHER_FILES.items():
        values[token] = hashlib.sha256(launcher_file(repo, tag, path, rehearsal)).hexdigest()
    text = TEMPLATE.read_text()
    if rehearsal:
        text = with_rehearsal_urls(text, rehearsal)
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


def rehearse(tag, sums, archive, rehearsal, out, repo=ROOT):
    """Write the manifest and the files its loopback URLs name, ready to be served from `out`."""
    target = out / "torlek.json"
    if target.exists():
        raise Refusal(f"{target} exists; rehearse into a directory without a manifest")
    text = manifest(tag, sums.read_bytes(), repo, rehearsal)
    archive_name = f"torlek-v{tag}-{PLATFORM}.tar.gz"
    if archive.name != archive_name:
        raise Refusal(f"{archive}: expected the file {archive_name}")
    (out / "release").mkdir(parents=True)
    shutil.copyfile(archive, out / "release" / archive_name)
    shutil.copyfile(sums, out / "release" / "SHA256SUMS")
    for path in LAUNCHER_FILES.values():
        served = out / "source" / path
        served.parent.mkdir(parents=True, exist_ok=True)
        served.write_bytes(launcher_file(repo, tag, path, rehearsal))
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
    rehearse_command = commands.add_parser("rehearse")
    rehearse_command.add_argument("--tag", required=True)
    rehearse_command.add_argument("--sums", type=Path, required=True)
    rehearse_command.add_argument("--archive", type=Path, required=True)
    rehearse_command.add_argument("--revision", required=True)
    rehearse_command.add_argument("--url", required=True)
    rehearse_command.add_argument("--out", type=Path, required=True)
    rehearse_command.add_argument("--repo", type=Path, default=ROOT)
    arguments = parser.parse_args()
    try:
        if arguments.command == "rehearse":
            rehearsal = Rehearsal(arguments.revision, arguments.url)
            target = rehearse(
                arguments.tag,
                arguments.sums,
                arguments.archive,
                rehearsal,
                arguments.out,
                arguments.repo,
            )
            print(f"Rehearsed the torlek {arguments.tag} Scoop manifest into {target}")
        else:
            target = render(arguments.tag, arguments.sums, arguments.out, arguments.repo)
            print(f"Rendered the torlek {arguments.tag} Scoop manifest into {target}")
    except (OSError, UnicodeDecodeError, Refusal) as error:
        parser.exit(1, f"render.py: {error}\n")


if __name__ == "__main__":
    main()

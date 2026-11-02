#!/usr/bin/env python3
"""The release workflows' version, packaging and gate steps (docs/RELEASING.md, issue #96).

Usage: release.py check-tag TAG --main REF --alpha-checklist open|closed
                             --beta-checklist open|closed
       release.py next-prerelease --main REF --alpha-checklist open|closed
                                  --beta-checklist open|closed
       release.py dry-run-version
       release.py stamp VERSION
       release.py package --version V --channel C --platform P --revision SHA
                          --binaries DIR --out DIR
       release.py verify-install ARCHIVE_ROOT --revision SHA
       release.py gate-gaps CHANNEL

check-tag, next-prerelease and dry-run-version print `key=value` lines for $GITHUB_OUTPUT.
Every refusal exits 1 with the rule it broke; nothing here creates a tag or a release.
"""

import argparse
import contextlib
import gzip
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Pre-releases count from the greenfield bootstrap (#18, #94): no stable tag is a base (#389).
BOOTSTRAP_COMMIT = "d2d860474e55d0e6f815ee33e50162351bbb1495"
CORE_PATTERN = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
# The release milestones' checklist issues that open each pre-release channel (#389); both
# workflows read these issues' states.
ALPHA_CHECKLIST_ISSUE = 52
BETA_CHECKLIST_ISSUE = 57
ALPHA_CHECKLIST = f"the Alpha 1 checklist (#{ALPHA_CHECKLIST_ISSUE})"
BETA_CHECKLIST = f"the Beta 1 checklist (#{BETA_CHECKLIST_ISSUE})"
TAG_PATTERN = re.compile(
    rf"v(?P<core>{CORE_PATTERN})(?:-(?P<channel>alpha|beta)\.(?P<number>[1-9][0-9]*))?"
)
PLATFORMS = ("linux-x86_64", "linux-aarch64", "macos-universal")
# Programs the daemon runs from its own directory, so they ship in bin/ beside it (#185).
HELPERS = (
    "cockpit-claude-hook",
    "cockpit-codex-hook",
    "cockpit-codex-session",
    "cockpit-opencode-bridge",
)
BINARIES = ("cockpit", "cockpit-daemon", "cockpit-tui", *HELPERS)
# No CHANGELOG.md: the committed file lags the tag (release-plz regenerates it, #101); the
# release notes are on the GitHub Release.
DOCUMENTS = ("LICENSE", "README.md")
MANIFEST_PATH = "share/cockpit/manifest.json"
MANIFEST_SCHEMA = 1
# How long verify-install waits for `cockpit version --all` and for the daemon's ready line.
COMMAND_TIMEOUT_SECONDS = 60
# The providers verify-install puts first on the daemon's PATH: each answers only
# `--version`, which is all the daemon runs of it at its start, in the words its adapter reads.
STAND_IN_VERSIONS = {
    "claude": "0.0.0 (Claude Code stand-in)",
    "codex": "codex-cli 0.0.0",
    "opencode": "0.0.0",
}
STAND_IN_PROVIDER = """#!/bin/sh
if [ "$1" = --version ]; then echo '{version}'; exit 0; fi
exit 1
"""
# Alpha notes carry the known gaps (RELEASING.md, Changelog); #97 writes the file.
KNOWN_GAPS = "docs/KNOWN-GAPS.md"

# Report 13's release gate: each check, the release.yml job that proves it (None while
# nothing does) and the channels it does not bind. A check without a job is a named gap that
# blocks publishing (RELEASING.md). An alpha may reset data, so it skips the upgrade check.
GATE_CHECKS = {
    "rust checks, clippy, dependency and license checks, crate-graph rules": ("gate", ()),
    "real CLI, terminal and provider-contract tests": ("gate", ()),
    "clean install on Linux x86_64, Linux aarch64 and macOS arm64": ("install", ()),
    "clean install of the Linux x86_64 archive inside WSL2": ("install-wsl2", ()),
    "clean install runs the macOS universal archive's x86_64 slice": (None, ()),
    "version-skew and interrupted-action fixtures": (None, ()),
    "sustained responsiveness run (report 03) against the candidate": (None, ()),
    "upgrade from the previous release of the channel": (None, ("alpha",)),
}
CHANNELS = ("alpha", "beta")


class Refusal(Exception):
    """A release rule the request breaks; the message names the rule."""


def parse_core(core):
    return tuple(int(part) for part in core.split("."))


def parse_tag(tag):
    match = TAG_PATTERN.fullmatch(tag)
    if match is None:
        raise Refusal(f"{tag}: not a release tag (vX.Y.Z, vX.Y.Z-beta.N or vX.Y.Z-alpha.N)")
    number = match["number"]
    return {
        "tag": tag,
        "core": match["core"],
        "channel": match["channel"] or "stable",
        "number": int(number) if number else None,
    }


def version_order(parsed):
    """Sort key in Semantic Versioning order for the tag shapes this policy allows."""
    channel_rank = {"alpha": 0, "beta": 1, "stable": 2}[parsed["channel"]]
    return (parse_core(parsed["core"]), channel_rank, parsed["number"] or 0)


def git(*arguments, cwd):
    result = subprocess.run(
        ["git", *arguments], cwd=cwd, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise Refusal(f"git {' '.join(arguments)} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def git_succeeds(*arguments, cwd):
    """For git's yes-or-no questions, answered by its exit status."""
    result = subprocess.run(["git", *arguments], cwd=cwd, capture_output=True, check=False)
    return result.returncode == 0


def is_ancestor(ancestor, descendant, cwd):
    return git_succeeds("merge-base", "--is-ancestor", ancestor, descendant, cwd=cwd)


def has_file(commit, path, cwd):
    return git_succeeds("cat-file", "-e", f"{commit}:{path}", cwd=cwd)


def release_tags(cwd, main_ref):
    """Every pre-release tag on main; a tag elsewhere was never a release.

    A stable tag is refused (#389), so one left on main is not a release and never the base.
    """
    tags = git("tag", "--list", "v*", "--merged", main_ref, cwd=cwd).splitlines()
    parsed_tags = [parse_tag(tag) for tag in tags if TAG_PATTERN.fullmatch(tag)]
    return [tag for tag in parsed_tags if tag["channel"] != "stable"]


def workspace_version(cwd, revision=None):
    if revision is None:
        manifest = (Path(cwd) / "Cargo.toml").read_text()
    else:
        manifest = git("show", f"{revision}:Cargo.toml", cwd=cwd)
    return tomllib.loads(manifest)["workspace"]["package"]["version"]


def base_of(cwd):
    """The bootstrap commit: pre-releases count from it while no stable tag exists (#389)."""
    return {"ref": BOOTSTRAP_COMMIT, "version": workspace_version(cwd, BOOTSTRAP_COMMIT)}


def commits_since(base_ref, commit, cwd):
    return int(git("rev-list", "--count", f"{base_ref}..{commit}", cwd=cwd))


def refuse_shallow(cwd):
    if git("rev-parse", "--is-shallow-repository", cwd=cwd) == "true":
        raise Refusal("shallow clone: N and the base need the full history (fetch-depth: 0)")


def newest_lower(tags, candidate):
    """The highest release below the candidate, or None."""
    lower = [other for other in tags if version_order(other) < version_order(candidate)]
    return max(lower, key=version_order, default=None)


def notes_start(parsed, tags, base):
    """Where a pre-release's notes start (RELEASING.md, Changelog)."""
    # refuse_released_or_behind guarantees the commit descends from it.
    previous = newest_lower(tags, parsed)
    return previous["tag"] if previous else base["ref"]


def refuse_off_main(commit, main_ref, cwd):
    if not is_ancestor(commit, main_ref, cwd):
        raise Refusal(f"{commit[:12]} is not on {main_ref}")


def check_tag(tag, main_ref, alpha_checklist, beta_checklist, cwd):
    """The release workflow's first step: every rule a tag must meet, however it was made."""
    parsed = parse_tag(tag)
    if parsed["channel"] == "stable":
        raise Refusal(f"{tag}: no stable release and no 1.0 until Theo decides (#389)")
    refuse_channel(parsed, alpha_checklist, beta_checklist)
    refuse_shallow(cwd)
    commit = git("rev-parse", f"{tag}^{{commit}}", cwd=cwd)
    refuse_off_main(commit, main_ref, cwd)
    tags = [other for other in release_tags(cwd, main_ref) if other["tag"] != tag]
    base = base_of(cwd)
    version_at_tag = workspace_version(cwd, commit)
    refuse_prerelease(parsed, commit, version_at_tag, tags, base, cwd)
    return {
        "tag": tag,
        "channel": parsed["channel"],
        "version": tag.removeprefix("v"),
        "prerelease": "true",
        "revision": commit,
        "notes_range": f"{notes_start(parsed, tags, base)}..{tag}",
    }


def refuse_channel(parsed, alpha_checklist, beta_checklist):
    """A pre-release tag's channel must be the one the checklists open."""
    tag = parsed["tag"]
    # Checked first so a beta tag names the checklist it waits for, not the alpha one.
    if parsed["channel"] == "beta" and beta_checklist == "open":
        raise Refusal(f"{tag}: no beta while {BETA_CHECKLIST} is open")
    if parsed["channel"] != prerelease_channel(alpha_checklist, beta_checklist):
        raise Refusal(f"{tag}: no alpha once {BETA_CHECKLIST} is closed; pre-releases are betas")


def refuse_released_or_behind(candidate, commit, tags, cwd):
    """One release per commit, each descending from the newest release before it."""
    # `tags` leaves out the tag being checked, so a tag never counts against itself.
    known_tags = {other["tag"] for other in tags}
    other_tags_on_commit = [
        other
        for other in git("tag", "--points-at", commit, cwd=cwd).splitlines()
        if other in known_tags
    ]
    if other_tags_on_commit:
        raise Refusal(f"{commit[:12]} is already released as {other_tags_on_commit[0]}")
    previous = newest_lower(tags, candidate)
    if previous and not is_ancestor(previous["tag"], commit, cwd):
        raise Refusal(f"{commit[:12]} does not descend from {previous['tag']}")


def refuse_prerelease(candidate, commit, version_at_commit, tags, base, cwd):
    """Every pre-release rule of RELEASING.md, Tags, for a tag made or about to be made."""
    tag = candidate["tag"]
    refuse_released_or_behind(candidate, commit, tags, cwd)
    if candidate["channel"] == "alpha" and not has_file(commit, KNOWN_GAPS, cwd):
        # release.yml needs it for the notes, and a tag is never moved or reused.
        raise Refusal(f"{tag}: an alpha needs {KNOWN_GAPS} for its notes (#97)")
    if version_at_commit != base["version"]:
        raise Refusal(
            f"{tag}: the workspace version at {commit[:12]} is {version_at_commit}; "
            f"a pre-release must still carry the base's {base['version']}"
        )
    refuse_core_against_base(candidate["core"], base)
    count = commits_since(base["ref"], commit, cwd)
    if candidate["number"] != count:
        raise Refusal(f"{tag}: N must be the commit count since {base['ref']}, which is {count}")
    if not tags:
        return
    newest = max(tags, key=version_order)
    if version_order(candidate) <= version_order(newest):
        raise Refusal(f"{tag} is not above the newest pre-release {newest['tag']}")


def refuse_core_against_base(core, base):
    if core != base["version"]:
        raise Refusal(f"{core}: a pre-release uses the bootstrap workspace's {base['version']}")


def prerelease_channel(alpha_checklist, beta_checklist):
    if beta_checklist == "closed":
        return "beta"
    if alpha_checklist == "open":
        raise Refusal(f"no alpha while {ALPHA_CHECKLIST} is open")
    return "alpha"


def next_prerelease(main_ref, alpha_checklist, beta_checklist, cwd):
    """The tag the dispatched prerelease workflow cuts on HEAD, after every refusal."""
    channel = prerelease_channel(alpha_checklist, beta_checklist)
    refuse_shallow(cwd)
    commit = git("rev-parse", "HEAD", cwd=cwd)
    refuse_off_main(commit, main_ref, cwd)
    # Every tag on main, not only HEAD's: a re-run on an older commit meets the newer tags.
    tags = release_tags(cwd, main_ref)
    base = base_of(cwd)
    version = workspace_version(cwd, commit)
    number = commits_since(base["ref"], commit, cwd)
    candidate = parse_tag(f"v{version}-{channel}.{number}")
    refuse_prerelease(candidate, commit, version, tags, base, cwd)
    return {"tag": candidate["tag"], "channel": channel, "revision": commit}


def dry_run_version(cwd):
    """A version no release tag can have, for builds that are never published."""
    refuse_shallow(cwd)
    base = base_of(cwd)
    count = commits_since(base["ref"], "HEAD", cwd)
    version = f"{workspace_version(cwd)}-dev.{count}"
    return {
        "version": version,
        "channel": "dev",
        "prerelease": "true",
        "revision": git("rev-parse", "HEAD", cwd=cwd),
        "notes_range": f"{base['ref']}..HEAD",
        "notes_tag": f"v{version}",
    }


def stamp(version, cwd):
    """Write VERSION into the workspace and every internal path-dependency pin, uncommitted.

    The pins (`cockpit-x = { path = ..., version = "0.1.0" }`, #120) would reject a
    pre-release version, so they move with it. Cargo rewrites Cargo.lock on the next build.
    """
    root = Path(cwd)
    old_version = workspace_version(cwd)
    root_manifest = root / "Cargo.toml"
    text = root_manifest.read_text()
    package_table = re.search(r"(?ms)^\[workspace\.package\]\n.*?(?=^\[|\Z)", text)
    if package_table is None:
        raise Refusal("Cargo.toml has no [workspace.package] table")
    stamped_table, replaced = re.subn(
        r'(?m)^version = "[^"]*"$', f'version = "{version}"', package_table[0], count=1
    )
    if replaced != 1:
        raise Refusal("[workspace.package] has no version line")
    root_manifest.write_text(
        text[: package_table.start()] + stamped_table + text[package_table.end() :]
    )
    path_dependency = re.compile(r"\{[^}\n]*\bpath = [^}\n]*\}")
    pin = f'version = "{old_version}"'

    def stamp_pin(table):
        return table[0].replace(pin, f'version = "{version}"')

    for member_manifest in sorted(root.glob("crates/*/Cargo.toml")):
        member_text = member_manifest.read_text()
        member_manifest.write_text(path_dependency.sub(stamp_pin, member_text))
    return {"version": version, "previous": old_version}


def sha256_of(path):
    with open(path, "rb") as contents:
        return hashlib.file_digest(contents, "sha256").hexdigest()


def archive_name(version, platform):
    return f"cockpit-v{version}-{platform}"


def bundle_files(root):
    """Every file of an unpacked bundle but its manifest, relative and sorted."""
    relative_paths = (str(path.relative_to(root)) for path in root.rglob("*") if path.is_file())
    return sorted(path for path in relative_paths if path != MANIFEST_PATH)


def helper_versions(version):
    """Helpers build from the same revision, so they carry the bundle's version."""
    return {helper: version for helper in HELPERS}


def write_manifest(staging, version, channel, platform, revision):
    """Report 13's bundle manifest: contents, source revision and component versions."""
    contents = bundle_files(staging)
    manifest = {
        "schema": MANIFEST_SCHEMA,
        "name": "cockpit",
        "version": version,
        "channel": channel,
        "platform": platform,
        "source_revision": revision,
        "helpers": helper_versions(version),
        # No instruction bundle ships yet (#97); `cockpit version --all` shows the null.
        "instruction_bundle": None,
        "contents": {path: sha256_of(staging / path) for path in contents},
    }
    manifest_file = staging / MANIFEST_PATH
    manifest_file.parent.mkdir(parents=True, exist_ok=True)
    manifest_file.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return manifest


def normalized(member):
    member.uid = member.gid = 0
    member.uname = member.gname = ""
    member.mtime = 0
    is_executable = member.isdir() or member.name.split("/")[-2:-1] == ["bin"]
    member.mode = 0o755 if is_executable else 0o644
    return member


def package(version, channel, platform, revision, binaries, out, cwd):
    """Stage bin/, the documents and the manifest, then write a reproducible .tar.gz."""
    if platform not in PLATFORMS:
        raise Refusal(f"{platform}: not one of {', '.join(PLATFORMS)}")
    name = archive_name(version, platform)
    staging = Path(out) / name
    if staging.exists():
        raise Refusal(f"{staging} exists; package into an empty directory")
    (staging / "bin").mkdir(parents=True)
    for binary in BINARIES:
        source = Path(binaries) / binary
        if not source.is_file():
            raise Refusal(f"{source}: built binary missing")
        target = staging / "bin" / binary
        shutil.copyfile(source, target)
        target.chmod(0o755)
    for document in DOCUMENTS:
        shutil.copyfile(Path(cwd) / document, staging / document)
    write_manifest(staging, version, channel, platform, revision)
    archive = Path(out) / f"{name}.tar.gz"
    # The gzip header's own timestamp is zeroed too, so a rebuild gives the same bytes.
    with (
        open(archive, "wb") as raw,
        gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as compressed,
        tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as bundle,
    ):
        for path in sorted([staging, *staging.rglob("*")]):
            bundle.add(path, arcname=str(path.relative_to(out)), recursive=False, filter=normalized)
    return {"archive": str(archive), "root": str(staging)}


def verify_install(root, revision):
    """The clean-install check on an unpacked bundle: contents, CLI/TUI versions, daemon start."""
    root = Path(root)
    manifest_file = root / MANIFEST_PATH
    if not manifest_file.is_file():
        raise Refusal(f"{manifest_file}: no bundle manifest")
    manifest_text = manifest_file.read_text()
    manifest = json.loads(manifest_text)
    if manifest.get("schema") != MANIFEST_SCHEMA:
        raise Refusal(
            f"{manifest_file}: schema {manifest.get('schema')!r} is not {MANIFEST_SCHEMA}"
        )
    refuse_changed_contents(root, manifest)
    # Short and fresh: a clean home, and a socket path under macOS's 104-byte limit.
    home = tempfile.mkdtemp(prefix="cockpit-home-", dir="/tmp")
    environment = {"HOME": home, "PATH": os.environ.get("PATH", "/usr/bin:/bin")}
    refuse_wrong_version_output(root, manifest, manifest_text, revision, environment)
    code, stdout, stderr = run_bounded([str(root / "bin/cockpit"), "tui", "--version"], environment)
    expected = f"cockpit-tui {manifest['version']}"
    if code != 0 or stdout.strip() != expected:
        raise Refusal(
            f"cockpit tui --version: exit {code}, expected {expected!r}, got {stdout!r} {stderr!r}"
        )
    # Stand-in providers, so the daemon checks each bridge it ships (#203, #411, #415).
    stand_in_dir = Path(home) / "stand-in-bin"
    stand_in_dir.mkdir()
    for provider, version in STAND_IN_VERSIONS.items():
        stand_in = stand_in_dir / provider
        stand_in.write_text(STAND_IN_PROVIDER.format(version=version))
        stand_in.chmod(0o755)
    environment = {**environment, "PATH": f"{stand_in_dir}:{environment['PATH']}"}
    refuse_daemon_not_ready(root, environment)
    return {"version": manifest["version"], "files": str(len(manifest["contents"]))}


def refuse_changed_contents(root, manifest):
    problems = []
    for relative, expected in sorted(manifest["contents"].items()):
        path = root / relative
        if not path.is_file():
            problems.append(f"{relative}: missing")
        elif sha256_of(path) != expected:
            problems.append(f"{relative}: checksum differs from the manifest")
    problems.extend(
        f"{relative}: not listed in the manifest"
        for relative in bundle_files(root)
        if relative not in manifest["contents"]
    )
    for binary in BINARIES:
        if f"bin/{binary}" not in manifest["contents"]:
            problems.append(f"bin/{binary}: not listed in the manifest")
    expected_helpers = helper_versions(manifest["version"])
    if manifest.get("helpers") != expected_helpers:
        problems.append(f"helpers: {manifest.get('helpers')!r}, expected {expected_helpers!r}")
    if problems:
        raise Refusal("; ".join(problems))


def stop_group(process):
    """Kill a process started with start_new_session, and everything it started."""
    with contextlib.suppress(ProcessLookupError):
        os.killpg(process.pid, signal.SIGKILL)
    process.wait(timeout=10)


def refuse_wrong_version_output(root, manifest, manifest_text, revision, environment):
    # Output goes to files and the group is always killed, so neither a hang nor a helper the
    # command leaves behind outlives the check.
    with (
        tempfile.TemporaryFile(mode="w+") as stdout_file,
        tempfile.TemporaryFile(mode="w+") as stderr_file,
    ):
        shown = subprocess.Popen(
            [str(root / "bin/cockpit"), "version", "--all"],
            env=environment,
            stdout=stdout_file,
            stderr=stderr_file,
            start_new_session=True,
        )
        try:
            shown.wait(timeout=COMMAND_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired as timeout:
            raise Refusal(
                f"cockpit version --all did not finish in {timeout.timeout}s"
            ) from timeout
        finally:
            stop_group(shown)
        stdout_file.seek(0)
        stdout = stdout_file.read()
        stderr_file.seek(0)
        stderr = stderr_file.read()
    expected_first_line = f"cockpit {manifest['version']} (revision {revision})"
    if shown.returncode != 0 or stdout.splitlines()[:1] != [expected_first_line]:
        raise Refusal(
            f"cockpit version --all: exit {shown.returncode}, expected {expected_first_line!r}, "
            f"got {stdout!r} {stderr!r}"
        )
    if manifest_text not in stdout:
        raise Refusal("cockpit version --all does not show the installed manifest")


def refuse_daemon_not_ready(root, environment):
    """cockpit-daemon prints `ready <socket>` once it accepts requests on a clean home, and
    `cockpit provider list` then shows that every stand-in provider reports events: the
    daemon found and ran each bridge beside it (#203, #411, #415)."""
    # Its own process group, so a helper it started dies with it; stderr goes to a file, so
    # a chatty daemon never blocks on a full pipe.
    with tempfile.TemporaryFile(mode="w+") as stderr_file:
        daemon = subprocess.Popen(
            [str(root / "bin/cockpit-daemon")],
            env=environment,
            stdout=subprocess.PIPE,
            stderr=stderr_file,
            text=True,
            start_new_session=True,
        )
        deadline = threading.Timer(COMMAND_TIMEOUT_SECONDS, os.killpg, (daemon.pid, signal.SIGKILL))
        deadline.start()
        try:
            first_line = daemon.stdout.readline()
            deadline.cancel()
            listed = None
            if first_line.startswith("ready "):
                try:
                    listed = run_bounded(
                        [str(root / "bin/cockpit"), "provider", "list"], environment
                    )
                except Refusal as timed_out:
                    # Raised once the daemon's log is read, which says why it hung.
                    listed = timed_out
        finally:
            deadline.cancel()
            stop_group(daemon)
            daemon.stdout.close()
        stderr_file.seek(0)
        stderr = stderr_file.read()
    if not first_line.startswith("ready "):
        raise Refusal(f"cockpit-daemon did not print `ready <socket>`: {first_line!r} {stderr!r}")
    if isinstance(listed, Refusal):
        raise Refusal(f"{listed}; daemon log {stderr!r}")
    code, stdout, listed_stderr = listed
    unbridged = [
        provider
        for provider in STAND_IN_VERSIONS
        if f"{provider}\tresume\tevents" not in stdout.splitlines()
    ]
    if code != 0 or unbridged:
        raise Refusal(
            f"the daemon did not find a runnable bridge for {', '.join(unbridged) or 'any'}: "
            f"cockpit provider list exit {code}, printed {stdout!r} {listed_stderr!r}; "
            f"daemon log {stderr!r}"
        )


def run_bounded(command, environment):
    """Exit code, stdout and stderr of `command`, in its own group killed after the timeout."""
    with (
        tempfile.TemporaryFile(mode="w+") as stdout_file,
        tempfile.TemporaryFile(mode="w+") as stderr_file,
    ):
        process = subprocess.Popen(
            command,
            env=environment,
            stdout=stdout_file,
            stderr=stderr_file,
            start_new_session=True,
        )
        try:
            process.wait(timeout=COMMAND_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired as timeout:
            raise Refusal(
                f"{' '.join(command[1:])} did not finish in {timeout.timeout}s"
            ) from timeout
        finally:
            stop_group(process)
        stdout_file.seek(0)
        stderr_file.seek(0)
        return process.returncode, stdout_file.read(), stderr_file.read()


def gate_gaps(channel):
    if channel not in CHANNELS:
        raise Refusal(f"{channel}: not a release channel")
    return [
        check
        for check, (job, exempt_channels) in GATE_CHECKS.items()
        if job is None and channel not in exempt_channels
    ]


def print_outputs(outputs):
    for key, value in outputs.items():
        print(f"{key}={value}")


def parse_arguments(arguments):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    check_command = commands.add_parser("check-tag")
    check_command.add_argument("tag")
    next_command = commands.add_parser("next-prerelease")
    for gated_command in (check_command, next_command):
        for checklist in ("--alpha-checklist", "--beta-checklist"):
            gated_command.add_argument(checklist, choices=("open", "closed"), required=True)
    commands.add_parser("dry-run-version")
    for command in (check_command, next_command):
        command.add_argument("--main", required=True)
    commands.add_parser("stamp").add_argument("version")
    package_command = commands.add_parser("package")
    for option in ("--version", "--channel", "--platform", "--revision", "--binaries", "--out"):
        package_command.add_argument(option, required=True)
    verify_command = commands.add_parser("verify-install")
    verify_command.add_argument("root")
    verify_command.add_argument("--revision", required=True)
    commands.add_parser("gate-gaps").add_argument("channel")
    return parser.parse_args(arguments)


def main(arguments, cwd=ROOT):
    options = parse_arguments(arguments)
    try:
        if options.command == "check-tag":
            print_outputs(
                check_tag(
                    options.tag,
                    options.main,
                    options.alpha_checklist,
                    options.beta_checklist,
                    cwd,
                )
            )
        elif options.command == "next-prerelease":
            print_outputs(
                next_prerelease(options.main, options.alpha_checklist, options.beta_checklist, cwd)
            )
        elif options.command == "dry-run-version":
            print_outputs(dry_run_version(cwd))
        elif options.command == "stamp":
            print_outputs(stamp(options.version, cwd))
        elif options.command == "package":
            print_outputs(
                package(
                    options.version,
                    options.channel,
                    options.platform,
                    options.revision,
                    options.binaries,
                    options.out,
                    cwd,
                )
            )
        elif options.command == "verify-install":
            print_outputs(verify_install(options.root, options.revision))
        elif options.command == "gate-gaps":
            gaps = gate_gaps(options.channel)
            for gap in gaps:
                print(f"gap: {gap}")
            if gaps:
                raise Refusal(f"{len(gaps)} release gate check(s) have no job; publishing waits")
    except Refusal as refusal:
        print(f"release: {refusal}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

#!/usr/bin/env python3
"""The release workflows' version, packaging and gate steps (docs/RELEASING.md, issue #96).

Usage: release.py check-tag TAG
       release.py next-prerelease --m3-gate open|closed --m5-gate open|closed
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
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import threading
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Before the first stable tag, pre-releases count from the greenfield bootstrap (#18, #94).
BOOTSTRAP_COMMIT = "d2d8604"
TAG_PATTERN = re.compile(
    r"v(?P<core>(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))"
    r"(?:-(?P<channel>alpha|beta)\.(?P<number>[1-9][0-9]*))?"
)
PLATFORMS = ("linux-x86_64", "linux-aarch64", "macos-universal")
BINARIES = ("cockpit", "cockpit-daemon")
DOCUMENTS = ("LICENSE", "README.md", "CHANGELOG.md")
MANIFEST_PATH = "share/cockpit/manifest.json"
MANIFEST_SCHEMA = 1

# Report 13's release gate: each check and the release.yml job that proves it, or None while
# nothing does. A check without a job is a named gap that blocks publishing (RELEASING.md).
GATE_CHECKS = {
    "rust checks, clippy, dependency and license checks, crate-graph rules": "gate",
    "real CLI, terminal and provider-contract tests": "gate",
    "clean install on Linux x86_64, Linux aarch64 and macOS arm64": "install",
    "clean install of the Linux x86_64 archive inside WSL2": "install-wsl2",
    "clean install runs the macOS universal archive's x86_64 slice": None,
    "version-skew and interrupted-action fixtures": None,
    "sustained responsiveness run (report 03) against the candidate": None,
    "upgrade from the previous release of the channel": None,
}
# An alpha may reset data, so only clean installs block it (RELEASING.md, Channels).
NOT_REQUIRED_BY_CHANNEL = {
    "alpha": {"upgrade from the previous release of the channel"},
    "beta": set(),
    "stable": set(),
}


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


def is_ancestor(ancestor, descendant, cwd):
    result = subprocess.run(
        ["git", "merge-base", "--is-ancestor", ancestor, descendant], cwd=cwd, check=False
    )
    return result.returncode == 0


def release_tags(cwd):
    """Every tag of a release shape; other tags are not releases and are ignored."""
    tags = git("tag", "--list", "v*", cwd=cwd).splitlines()
    return [parse_tag(tag) for tag in tags if TAG_PATTERN.fullmatch(tag)]


def workspace_version(cwd, revision=None):
    if revision is None:
        manifest = (Path(cwd) / "Cargo.toml").read_text()
    else:
        manifest = git("show", f"{revision}:Cargo.toml", cwd=cwd)
    return tomllib.loads(manifest)["workspace"]["package"]["version"]


def base_of(tags, cwd):
    """The newest stable tag, published or not, else the bootstrap commit."""
    stable_tags = [tag for tag in tags if tag["channel"] == "stable"]
    if not stable_tags:
        return {
            "ref": BOOTSTRAP_COMMIT,
            "version": workspace_version(cwd, BOOTSTRAP_COMMIT),
            "is_stable_tag": False,
        }
    newest = max(stable_tags, key=version_order)
    return {"ref": newest["tag"], "version": newest["core"], "is_stable_tag": True}


def commits_since(base_ref, commit, cwd):
    return int(git("rev-list", "--count", f"{base_ref}..{commit}", cwd=cwd))


def refuse_shallow(cwd):
    if git("rev-parse", "--is-shallow-repository", cwd=cwd) == "true":
        raise Refusal("shallow clone: N and the base need the full history (fetch-depth: 0)")


def notes_start(parsed, tags, commit, cwd):
    """Where the release notes start (RELEASING.md, Changelog)."""
    if parsed["channel"] == "stable":
        return base_of(tags, cwd)["ref"]
    lower = [
        tag
        for tag in tags
        if version_order(tag) < version_order(parsed) and is_ancestor(tag["tag"], commit, cwd)
    ]
    if lower:
        return max(lower, key=version_order)["tag"]
    return base_of(tags, cwd)["ref"]


def check_tag(tag, cwd):
    """The release workflow's first step: the tag matches the commit's version and count."""
    parsed = parse_tag(tag)
    refuse_shallow(cwd)
    commit = git("rev-parse", f"{tag}^{{commit}}", cwd=cwd)
    tags = [other for other in release_tags(cwd) if other["tag"] != tag]
    version_at_tag = workspace_version(cwd, commit)
    if parsed["channel"] == "stable":
        if version_at_tag != parsed["core"]:
            raise Refusal(
                f"{tag}: the workspace version at {commit[:12]} is {version_at_tag}; "
                "a stable tag must equal it"
            )
        version = parsed["core"]
    else:
        base = base_of(tags, cwd)
        if version_at_tag != base["version"]:
            raise Refusal(
                f"{tag}: the workspace version at {commit[:12]} is {version_at_tag}; "
                f"a pre-release must still carry the base's {base['version']}"
            )
        refuse_core_against_base(parsed["core"], base)
        count = commits_since(base["ref"], commit, cwd)
        if parsed["number"] != count:
            raise Refusal(
                f"{tag}: N must be the commit count since {base['ref']}, which is {count}"
            )
        version = f"{parsed['core']}-{parsed['channel']}.{parsed['number']}"
    return {
        "tag": tag,
        "channel": parsed["channel"],
        "version": version,
        "prerelease": "false" if parsed["channel"] == "stable" else "true",
        "revision": commit,
        "notes_start": notes_start(parsed, tags, commit, cwd),
    }


def refuse_core_against_base(core, base):
    if base["is_stable_tag"]:
        if parse_core(core) <= parse_core(base["version"]):
            raise Refusal(f"{core} is not above the previous stable {base['version']}")
    elif core != base["version"]:
        raise Refusal(
            f"{core}: before the first stable tag a pre-release uses the workspace's "
            f"{base['version']}"
        )


def prerelease_channel(m3_gate, m5_gate):
    if m5_gate == "closed":
        return "beta"
    if m3_gate == "open":
        raise Refusal("no alpha while the M3 gate issue (#52) is open")
    return "alpha"


def next_prerelease(m3_gate, m5_gate, cwd):
    """The tag the dispatched prerelease workflow cuts on HEAD, after every refusal."""
    channel = prerelease_channel(m3_gate, m5_gate)
    refuse_shallow(cwd)
    commit = git("rev-parse", "HEAD", cwd=cwd)
    tags = release_tags(cwd)
    tags_on_commit = git("tag", "--points-at", commit, cwd=cwd).splitlines()
    release_tags_on_commit = [tag for tag in tags_on_commit if TAG_PATTERN.fullmatch(tag)]
    if release_tags_on_commit:
        raise Refusal(f"{commit[:12]} is already released as {release_tags_on_commit[0]}")
    base = base_of(tags, cwd)
    if base["is_stable_tag"]:
        # The version comes from the open release PR, which nothing reads yet (#101).
        raise Refusal("a pre-release after a stable tag needs the release PR's version (#101)")
    core = workspace_version(cwd)
    refuse_core_against_base(core, base)
    if any(tag["channel"] == "stable" and tag["core"] == core for tag in tags):
        raise Refusal(f"{core} already has a stable tag")
    candidate = {
        "tag": None,
        "core": core,
        "channel": channel,
        "number": commits_since(base["ref"], commit, cwd),
    }
    candidate["tag"] = f"v{core}-{channel}.{candidate['number']}"
    prereleases = [tag for tag in tags if tag["channel"] != "stable"]
    if prereleases:
        newest = max(prereleases, key=version_order)
        if version_order(candidate) <= version_order(newest):
            raise Refusal(f"{candidate['tag']} is not above the newest pre-release {newest['tag']}")
        if not is_ancestor(newest["tag"], commit, cwd):
            raise Refusal(f"{commit[:12]} does not descend from {newest['tag']}")
    return {"tag": candidate["tag"], "channel": channel, "revision": commit}


def dry_run_version(cwd):
    """A version no release tag can have, for builds that are never published."""
    refuse_shallow(cwd)
    tags = release_tags(cwd)
    base = base_of(tags, cwd)
    count = commits_since(base["ref"], "HEAD", cwd)
    return {
        "version": f"{workspace_version(cwd)}-dev.{count}",
        "channel": "dev",
        "prerelease": "true",
        "revision": git("rev-parse", "HEAD", cwd=cwd),
        "notes_start": base["ref"],
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
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def archive_name(version, platform):
    return f"cockpit-v{version}-{platform}"


def write_manifest(staging, version, channel, platform, revision):
    """Report 13's bundle manifest: contents, source revision and component versions."""
    contents = sorted(
        str(path.relative_to(staging))
        for path in staging.rglob("*")
        if path.is_file() and str(path.relative_to(staging)) != MANIFEST_PATH
    )
    manifest = {
        "schema": MANIFEST_SCHEMA,
        "name": "cockpit",
        "version": version,
        "channel": channel,
        "platform": platform,
        "source_revision": revision,
        # No helper and no instruction bundle ships yet (#64, #97); an empty record is the
        # truth, and `cockpit version --all` shows it.
        "helpers": {},
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
        target.write_bytes(source.read_bytes())
        target.chmod(0o755)
    for document in DOCUMENTS:
        (staging / document).write_bytes((Path(cwd) / document).read_bytes())
    write_manifest(staging, version, channel, platform, revision)
    archive = Path(out) / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz", format=tarfile.PAX_FORMAT) as bundle:
        for path in sorted([staging, *staging.rglob("*")]):
            bundle.add(path, arcname=str(path.relative_to(out)), recursive=False, filter=normalized)
    return {"archive": str(archive), "root": str(staging)}


def verify_install(root, revision):
    """The clean-install check on an unpacked bundle: contents, version --all, daemon start."""
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
    for binary in BINARIES:
        if f"bin/{binary}" not in manifest["contents"]:
            problems.append(f"bin/{binary}: not listed in the manifest")
    if problems:
        raise Refusal("; ".join(problems))


def refuse_wrong_version_output(root, manifest, manifest_text, revision, environment):
    shown = subprocess.run(
        [str(root / "bin/cockpit"), "version", "--all"],
        env=environment,
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    expected_first_line = f"cockpit {manifest['version']} (revision {revision})"
    if shown.returncode != 0 or shown.stdout.splitlines()[:1] != [expected_first_line]:
        raise Refusal(
            f"cockpit version --all: exit {shown.returncode}, expected {expected_first_line!r}, "
            f"got {shown.stdout!r} {shown.stderr!r}"
        )
    if manifest_text not in shown.stdout:
        raise Refusal("cockpit version --all does not show the installed manifest")


def refuse_daemon_not_ready(root, environment, timeout_seconds=60):
    """cockpit-daemon prints `ready <socket>` once it accepts requests on a clean home."""
    daemon = subprocess.Popen(
        [str(root / "bin/cockpit-daemon")],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    deadline = threading.Timer(timeout_seconds, daemon.kill)
    deadline.start()
    try:
        first_line = daemon.stdout.readline()
    finally:
        deadline.cancel()
        daemon.kill()
        _stdout, stderr = daemon.communicate()
    if not first_line.startswith("ready "):
        raise Refusal(f"cockpit-daemon did not print `ready <socket>`: {first_line!r} {stderr!r}")


def gate_gaps(channel):
    if channel not in NOT_REQUIRED_BY_CHANNEL:
        raise Refusal(f"{channel}: not a release channel")
    return [
        check
        for check, job in GATE_CHECKS.items()
        if job is None and check not in NOT_REQUIRED_BY_CHANNEL[channel]
    ]


def print_outputs(outputs):
    for key, value in outputs.items():
        print(f"{key}={value}")


def parse_arguments(arguments):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check-tag").add_argument("tag")
    next_command = commands.add_parser("next-prerelease")
    next_command.add_argument("--m3-gate", choices=("open", "closed"), required=True)
    next_command.add_argument("--m5-gate", choices=("open", "closed"), required=True)
    commands.add_parser("dry-run-version")
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
            print_outputs(check_tag(options.tag, cwd))
        elif options.command == "next-prerelease":
            print_outputs(next_prerelease(options.m3_gate, options.m5_gate, cwd))
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
    sys.exit(main(sys.argv[1:], cwd=Path.cwd()))

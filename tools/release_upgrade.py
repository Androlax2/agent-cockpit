#!/usr/bin/env python3
"""Check upgrades between published channel archives in an isolated temporary home."""

import argparse
import contextlib
import json
import os
import queue
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from pathlib import Path, PurePosixPath

import release

REPOSITORY = "Androlax2/agent-cockpit"


def gh(*arguments):
    result = subprocess.run(
        ["gh", *arguments], capture_output=True, text=True, timeout=60, check=False
    )
    if result.returncode:
        raise release.Refusal(f"GitHub metadata/download command failed (exit {result.returncode})")
    return result.stdout


def previous_release(version):
    candidate = release.parse_tag("v" + version)
    if candidate["channel"] not in release.CHANNELS:
        raise release.Refusal("stable release policy remains disabled")
    pages = json.loads(gh("api", "--paginate", "--slurp", f"repos/{REPOSITORY}/releases"))
    if not isinstance(pages, list) or any(not isinstance(page, list) for page in pages):
        raise release.Refusal("invalid releases metadata")
    earlier = []
    for page in pages:
        for record in page:
            if not isinstance(record, dict):
                raise release.Refusal("invalid release record")
            tag = record.get("tag_name", "")
            if (
                record.get("draft") is not False
                or not isinstance(tag, str)
                or not release.TAG_PATTERN.fullmatch(tag)
            ):
                continue
            parsed = release.parse_tag(tag)
            if parsed["channel"] == candidate["channel"] and release.version_order(
                parsed
            ) < release.version_order(candidate):
                earlier.append(parsed)
    if not earlier:
        raise release.Refusal(
            f"no published earlier {candidate['channel']} release; upgrade was not run"
        )
    return max(earlier, key=release.version_order)["tag"]


def checked_manifest(root, revision):
    text = (root / release.MANIFEST_PATH).read_text()
    manifest = json.loads(text)
    if not isinstance(manifest, dict) or manifest.get("schema") != release.MANIFEST_SCHEMA:
        raise release.Refusal("unsupported bundle manifest")
    if not re.fullmatch(r"[0-9a-f]{40}", revision) or manifest.get("source_revision") != revision:
        raise release.Refusal("bundle source revision differs from the expected commit")
    if manifest.get("platform") not in release.PLATFORMS:
        raise release.Refusal("unsupported bundle platform")
    contents = manifest.get("contents")
    if not isinstance(contents, dict):
        raise release.Refusal("invalid bundle contents")
    for name, digest in contents.items():
        relative = PurePosixPath(name)
        if relative.is_absolute() or ".." in relative.parts:
            raise release.Refusal("unsafe manifest path")
        path = root / name
        if (
            path.is_symlink()
            or not path.resolve().is_relative_to(root.resolve())
            or not path.is_file()
            or release.sha256_of(path) != digest
        ):
            raise release.Refusal(f"{name}: bundle checksum differs")
    if set(release.bundle_files(root)) != set(contents):
        raise release.Refusal("bundle contents differ from the manifest")
    if not {"bin/torlek", "bin/torlek-daemon"} <= contents.keys():
        raise release.Refusal("bundle lacks its CLI or daemon")
    return manifest, text


def fetch_previous(version, channel, platform, directory):
    directory.mkdir(parents=True, exist_ok=True)
    if channel == "dev" and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+-dev\.[0-9]+", version):
        (directory / "not-run.txt").write_text("dev dry run: no release channel\n")
        return "upgrade=not-run (dry run, no release channel)"
    if release.parse_tag("v" + version)["channel"] != channel:
        raise release.Refusal("version and channel disagree")
    if channel == "alpha":
        (directory / "not-run.txt").write_text("explicit Alpha upgrade exemption\n")
        return "upgrade=not-run (explicit Alpha exemption)"
    if channel != "beta" or platform not in release.PLATFORMS:
        raise release.Refusal("unsupported upgrade channel/platform")
    tag = previous_release(version)
    revision = json.loads(gh("api", f"repos/{REPOSITORY}/commits/{tag}"))["sha"]
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise release.Refusal("invalid previous source revision")
    name = f"torlek-{tag}-{platform}"
    archive = directory / (name + ".tar.gz")
    gh(
        "release",
        "download",
        tag,
        "--repo",
        REPOSITORY,
        "--dir",
        str(directory),
        "--pattern",
        archive.name,
        "--pattern",
        "SHA256SUMS",
    )
    (directory / "baseline.json").write_text(json.dumps({"tag": tag, "revision": revision}))
    return f"baseline={tag} ({revision})"


def unpack_previous(version, platform, directory):
    metadata = json.loads((directory / "baseline.json").read_text())
    tag, revision = metadata["tag"], metadata["revision"]
    previous, candidate = release.parse_tag(tag), release.parse_tag("v" + version)
    if previous["channel"] != candidate["channel"] or release.version_order(
        previous
    ) >= release.version_order(candidate):
        raise release.Refusal("baseline must be an earlier release of the same channel")
    name = f"torlek-{tag}-{platform}"
    archive = directory / (name + ".tar.gz")
    checksums = [line.split() for line in (directory / "SHA256SUMS").read_text().splitlines()]
    matching = [parts[0] for parts in checksums if len(parts) == 2 and parts[1] == archive.name]
    if (
        len(matching) != 1
        or not re.fullmatch(r"[0-9a-f]{64}", matching[0])
        or release.sha256_of(archive) != matching[0]
    ):
        raise release.Refusal("previous archive checksum differs or is missing/ambiguous")
    with tarfile.open(archive) as bundle:
        members = bundle.getmembers()
        seen = set()
        for member in members:
            path = PurePosixPath(member.name)
            if (
                not path.parts
                or path.parts[0] != name
                or ".." in path.parts
                or not (member.isdir() or member.isfile())
                or path in seen
            ):
                raise release.Refusal("unsafe or duplicate previous archive member")
            seen.add(path)
        bundle.extractall(directory, members=members, filter="data")
    root = directory / name
    manifest, text = checked_manifest(root, revision)
    if (manifest.get("version"), manifest.get("channel"), manifest["platform"]) != (
        tag[1:],
        release.parse_tag(tag)["channel"],
        platform,
    ):
        raise release.Refusal("previous bundle does not match the selected release/platform")
    return root, manifest, text


def checked_command(command, environment):
    code, stdout, stderr = release.run_bounded([str(part) for part in command], environment)
    if code:
        raise release.Refusal(f"{Path(command[0]).name} failed (exit {code}): {stderr}")
    return stdout


@contextlib.contextmanager
def running_daemon(root, environment):
    with tempfile.TemporaryFile(mode="w+") as stderr:
        process = subprocess.Popen(
            [str(root / "bin/torlek-daemon"), "--profile", "release-gate"],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=stderr,
            text=True,
            start_new_session=True,
        )
        ready = queue.Queue()

        def read_output():
            ready.put(process.stdout.readline())
            for _line in process.stdout:
                pass

        reader = threading.Thread(target=read_output, daemon=True)
        reader.start()
        try:
            try:
                line = ready.get(timeout=release.COMMAND_TIMEOUT_SECONDS)
            except queue.Empty:
                line = "startup timeout"
            expected = str(Path(environment["XDG_RUNTIME_DIR"]) / "torlek-release-gate/daemon.sock")
            if line.strip() == "ready " + expected:
                yield
                stop_daemon(process)
                return
        finally:
            release.stop_group(process)
            reader.join(timeout=10)
            process.stdout.close()
        stderr.seek(0)
        raise release.Refusal(f"daemon did not become ready: {line!r}; {stderr.read()}")


def stop_daemon(process):
    """SIGTERM, as `systemctl --user restart` stops it during an upgrade, so the daemon's own
    shutdown runs; one that outlives the command timeout fails the check."""
    process.terminate()
    try:
        process.wait(timeout=release.COMMAND_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as error:
        raise release.Refusal(
            f"daemon did not stop within {release.COMMAND_TIMEOUT_SECONDS}s of SIGTERM"
        ) from error


def verify(candidate, revision, downloaded, fixture):
    """Runs `fixture` from the previous channel release to the candidate; alpha and dev run it
    from the candidate to itself and say the cross-version upgrade was not run."""
    check, name, proves = fixture
    candidate = candidate.resolve()
    manifest, text = checked_manifest(candidate, revision)
    if not {f"bin/{binary}" for binary in release.BINARIES} <= manifest["contents"].keys():
        raise release.Refusal("candidate lacks a required binary")
    # A previous release may carry older helpers; only the candidate requires the current set.
    expected_helpers = release.helper_versions(manifest["version"])
    if manifest.get("helpers") != expected_helpers:
        raise release.Refusal(
            f"helpers: {manifest.get('helpers')!r}, expected {expected_helpers!r}"
        )
    channel = manifest["channel"]
    if channel == "dev" and re.fullmatch(
        r"[0-9]+\.[0-9]+\.[0-9]+-dev\.[0-9]+", manifest["version"]
    ):
        check(candidate, manifest, text, candidate, manifest, text)
        return f"upgrade=not-run (dev: no release channel); same-candidate {name} fixture passed"
    if release.parse_tag("v" + manifest["version"])["channel"] != channel:
        raise release.Refusal("version and channel disagree")
    if channel == "alpha":
        check(candidate, manifest, text, candidate, manifest, text)
        return f"upgrade=not-run (alpha: explicit exemption); same-candidate {name} fixture passed"
    if channel != "beta" or release.parse_tag("v" + manifest["version"])["channel"] != channel:
        raise release.Refusal("candidate is not an enabled release channel")
    with tempfile.TemporaryDirectory(prefix="torlek-upgrade-") as downloads:
        directory = Path(downloads)
        if downloaded is None:
            fetch_previous(manifest["version"], channel, manifest["platform"], directory)
        else:
            # Only copy files; extraction always happens in a new private directory.
            for file in downloaded.iterdir():
                if file.is_file() and not file.is_symlink():
                    shutil.copyfile(file, directory / file.name)
        old, old_manifest, old_text = unpack_previous(
            manifest["version"], manifest["platform"], directory
        )
        check(old, old_manifest, old_text, candidate, manifest, text)
    return f"upgrade=passed ({old_manifest['version']} -> {manifest['version']}; {proves})"


@contextlib.contextmanager
def private_home(old, old_manifest, old_text, candidate, manifest, text):
    """A temporary home with stand-in providers first on `PATH`, after both bundles answered
    their own version in it, and a git project to add; yields the home and its environment."""
    # A short private home keeps Unix socket paths within macOS's limit.
    with tempfile.TemporaryDirectory(prefix="cu-", dir="/tmp") as home_name:
        home = Path(home_name)
        environment = {"HOME": home_name, "PATH": "/usr/bin:/bin", "GIT_CONFIG_NOSYSTEM": "1"}
        for key, suffix in (
            ("XDG_RUNTIME_DIR", "run"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CONFIG_HOME", "config"),
            # The daemons' tmux server lives in the home, never on the host's default socket.
            ("TMUX_TMPDIR", "tmux"),
        ):
            path = home / suffix
            path.mkdir(mode=0o700)
            environment[key] = str(path)
        stand_ins = home / "bin"
        stand_ins.mkdir()
        for provider, version in release.STAND_IN_VERSIONS.items():
            program = stand_ins / provider
            program.write_text(release.STAND_IN_PROVIDER.format(version=version))
            program.chmod(0o755)
        environment["PATH"] = str(stand_ins) + ":/usr/bin:/bin"
        for root, info, shown in ((old, old_manifest, old_text), (candidate, manifest, text)):
            release.refuse_wrong_version_output(
                root, info, shown, info["source_revision"], environment
            )
        project = home / "project"
        project.mkdir()
        git = [
            "git",
            "-C",
            project,
            "-c",
            "user.name=Upgrade fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ]
        checked_command([*git, "init", "--quiet"], environment)
        checked_command([*git, "commit", "--allow-empty", "--quiet", "-m", "fixture"], environment)
        try:
            yield home, environment
        finally:
            stop_tmux_servers(Path(environment["TMUX_TMPDIR"]))


def stop_tmux_servers(tmux_tmpdir):
    """Stops every tmux server the home's daemons started; their agents end with them. It runs
    while a failure may be on its way out, so a server that does not stop is reported, not
    raised over that failure."""
    for socket in sorted(tmux_tmpdir.glob("tmux-*/*")):
        try:
            stopped = subprocess.run(
                ["tmux", "-S", str(socket), "kill-server"],
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
        except subprocess.TimeoutExpired:
            print(f"release upgrade: tmux server {socket} did not stop in 10s", file=sys.stderr)
            continue
        if stopped.returncode:
            print(
                f"release upgrade: stopping tmux server {socket} failed "
                f"(exit {stopped.returncode}): {stopped.stderr.strip()}",
                file=sys.stderr,
            )


def verify_state(old, old_manifest, old_text, candidate, manifest, text):
    with private_home(old, old_manifest, old_text, candidate, manifest, text) as (
        home,
        environment,
    ):

        def cli(root, *arguments):
            return checked_command(
                [root / "bin/torlek", "--profile", "release-gate", *arguments], environment
            )

        project = home / "project"
        title = "retained task before upgrade"
        with running_daemon(old, environment):
            cli(old, "project", "add", str(project), "--name", "upgrade")
            cli(old, "todo", "add", title, "--project", "upgrade")
            before = cli(old, "todo", "list", "--project", "upgrade").splitlines()
            if len(before) != 1 or not before[0].endswith("\t" + title):
                raise release.Refusal("baseline did not persist the retained task")
        with running_daemon(candidate, environment):
            after = cli(candidate, "todo", "list", "--project", "upgrade").splitlines()
            if after != before:
                raise release.Refusal("candidate lost or changed the retained task")
            cli(candidate, "todo", "add", "written after upgrade", "--project", "upgrade")
            after_write = cli(candidate, "todo", "list", "--project", "upgrade").splitlines()
            if before[0] not in after_write or not any(
                row.endswith("\twritten after upgrade") for row in after_write
            ):
                raise release.Refusal("candidate cannot persist new state after upgrade")


# The provider of the session that lives through the upgrade: it logs its launch, then reports
# each line its terminal receives through the installed `torlek`, as an agent's hook does.
SESSION_AGENT = """#!/bin/sh
if [ "$1" = --version ]; then echo '{version}'; exit 0; fi
# The daemon's other probes, such as `auth status`, run outside any session.
if [ -z "${{TORLEK_SESSION-}}" ]; then exit 1; fi
printf '%s %s\\n' "$TORLEK_SESSION" "$$" >> '{home}/launches.log'
'{home}/installed/bin/torlek' agent report --state waiting > '{home}/waiting.partial' 2>&1
printf 'exit=%s\\n' "$?" >> '{home}/waiting.partial'
mv '{home}/waiting.partial' '{home}/waiting.log'
while IFS= read -r line; do
  '{home}/installed/bin/torlek' agent report --state waiting --label "$line" \\
    > '{home}/report.partial' 2>&1
  printf 'exit=%s\\n%s\\n' "$?" "$line" >> '{home}/report.partial'
  mv '{home}/report.partial' '{home}/report.log'
done
"""
REACHED = "reached after the upgrade"


def wait_for(what, probe):
    """`probe()`'s first truthy answer, polled until the command timeout names `what`."""
    deadline = time.monotonic() + release.COMMAND_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        answer = probe()
        if answer:
            return answer
        time.sleep(0.1)
    raise release.Refusal(f"timed out waiting for {what}")


def read_if_written(path):
    """What the agent wrote whole to `path`, or None before it renamed the file into place."""
    return path.read_text() if path.exists() else None


def launches(home):
    log = home / "launches.log"
    return log.read_text().splitlines() if log.exists() else []


def is_alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def verify_sessions(old, old_manifest, old_text, candidate, manifest, text):
    """A session started under the old daemon keeps its one agent through stop, swap of the
    installed bundle and start, and both the owner and the agent reach it through the candidate."""
    with private_home(old, old_manifest, old_text, candidate, manifest, text) as (
        home,
        environment,
    ):
        agent = home / "bin/claude"
        agent.write_text(
            SESSION_AGENT.format(version=release.STAND_IN_VERSIONS["claude"], home=home)
        )
        installed = home / "installed"

        def cli(*arguments):
            return checked_command(
                [installed / "bin/torlek", "--profile", "release-gate", *arguments], environment
            )

        def shown(session):
            # `id  state  agent  launch ...`: what a daemon lists of the session's run.
            rows = [row.split("\t") for row in cli("session", "list").splitlines()]
            matching = [row[:4] for row in rows if row[0] == session]
            if len(matching) > 1:
                raise release.Refusal(f"session {session} is listed {len(matching)} times")
            return matching[0] if matching else None

        def running(row):
            return row if row and row[1] == "running" else None

        installed.symlink_to(old)
        with running_daemon(installed, environment):
            cli("project", "add", str(home / "project"), "--name", "upgrade")
            session = cli("session", "new", str(home / "project"), "--agent", "claude")
            session = session.split("\t", 1)[0]
            before = wait_for("the session to run", lambda: running(shown(session)))
            waiting = wait_for(
                "the agent's first report", lambda: read_if_written(home / "waiting.log")
            )
            if not waiting.endswith("exit=0\n"):
                raise release.Refusal(
                    f"the agent could not report to the previous release: {waiting!r}"
                )
            launched = launches(home)
        if len(launched) != 1 or not launched[0].startswith(session + " "):
            raise release.Refusal(f"previous release launched {launched!r}")
        pid = int(launched[0].split()[1])
        if not is_alive(pid):
            raise release.Refusal("the session's agent ended with the previous daemon")
        installed.unlink()
        installed.symlink_to(candidate)
        with running_daemon(installed, environment):
            after = wait_for(
                "the candidate to list the session running", lambda: running(shown(session))
            )
            if after != before:
                raise release.Refusal(f"candidate shows {after!r}, not the running {before!r}")
            cli("session", "send", session, REACHED)
            report = wait_for(
                "the agent's report through the candidate",
                lambda: read_if_written(home / "report.log"),
            )
            if report != f"reported for session {session}\nexit=0\n{REACHED}\n":
                raise release.Refusal(
                    f"the session's agent did not reach the candidate: {report!r}"
                )
            if launches(home) != launched or not is_alive(pid):
                raise release.Refusal(f"candidate did not keep the one agent: {launches(home)!r}")


FIXTURES = {
    "verify": (verify_state, "state", "project/task persistence"),
    "sessions": (verify_sessions, "running-session", "a running session kept and reachable"),
}


def main(arguments):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("select").add_argument("--version", required=True)
    for name in FIXTURES:
        check = commands.add_parser(name)
        check.add_argument("--candidate", required=True, type=Path)
        check.add_argument("--revision", required=True)
        check.add_argument("--downloads", type=Path)
    fetch = commands.add_parser("fetch")
    fetch.add_argument("--version", required=True)
    fetch.add_argument("--channel", required=True)
    fetch.add_argument("--platform", required=True)
    fetch.add_argument("--out", type=Path, required=True)
    options = parser.parse_args(arguments)
    try:
        if options.command == "select":
            result = previous_release(options.version)
        elif options.command == "fetch":
            result = fetch_previous(options.version, options.channel, options.platform, options.out)
        else:
            result = verify(
                options.candidate, options.revision, options.downloads, FIXTURES[options.command]
            )
        print(result)
    except (
        release.Refusal,
        subprocess.TimeoutExpired,
        OSError,
        ValueError,
        TypeError,
        KeyError,
        tarfile.TarError,
    ) as error:
        print(f"release upgrade: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

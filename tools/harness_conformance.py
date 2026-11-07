#!/usr/bin/env python3
"""Nightly conformance of each adapter's latest harness (#732).

`check` installs the latest release of one harness, runs that adapter's live conformance tests
against it and records the outcome. `report` turns outcomes into GitHub issues (the workflow's
GITHUB_TOKEN, `issues: write`). `propose` turns passes into pull requests that append the version
to the adapter's tested-versions file (the release proposal App token and deploy key).
Nothing here needs a provider account: the live tests only run the harness's own `--help` and
`--version`, and read its program file.
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple

import release_pr

BRANCH_PREFIX = "harness-conformance/"
ISSUE_BOT = "github-actions[bot]"
VERSION = re.compile(r"[0-9]+(?:\.[0-9]+){1,3}(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?")
INSTALL_TIMEOUT_SECONDS = 600
TEST_TIMEOUT_SECONDS = 1800
NEXTEST_TEST_FAILURE = 100
STAGES = ("install", "version", "conformance", "cargo")


class Adapter(NamedTuple):
    package: str
    executable: str
    program_variable: str


ADAPTERS = {
    "claude": Adapter("@anthropic-ai/claude-code", "claude", "COCKPIT_CLAUDE_PROGRAM"),
    "codex": Adapter("@openai/codex", "codex", "COCKPIT_CODEX_PROGRAM"),
    "opencode": Adapter("opencode-ai", "opencode", "COCKPIT_OPENCODE_PROGRAM"),
}

Refusal = release_pr.Refusal


def tested_versions_path(adapter):
    return Path("crates") / f"cockpit-{adapter}" / "tested-versions.txt"


def listed_versions(text):
    return [
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]


def run_url():
    server = os.environ.get("GITHUB_SERVER_URL")
    run_id = os.environ.get("GITHUB_RUN_ID")
    if not server or not run_id:
        return "(not run by GitHub Actions)"
    return f"{server}/{release_pr.REPOSITORY}/actions/runs/{run_id}"


def read_results(path):
    results = json.loads(Path(path).read_text())
    if not isinstance(results, list):
        raise Refusal("results must be a list")
    for result in results:
        if result.get("adapter") not in ADAPTERS:
            raise Refusal(f"unknown adapter {result.get('adapter')!r}")
        version = result.get("version")
        if version is not None and not VERSION.fullmatch(version):
            raise Refusal(f"{version!r} is not a version")
        if not isinstance(result.get("passed"), bool):
            raise Refusal("each result needs passed: true or false")
        if result.get("stage", "conformance") not in STAGES:
            raise Refusal(f"{result['stage']!r} is not a stage")
        if result["passed"] and version is None:
            raise Refusal("a passing result needs its version")
    return results


def command(arguments, *, env=None, timeout, capture=False):
    """Run a command in its own process group; (exit code, its captured output or a reason).

    Output that is not captured goes straight to the log, so a hang leaves what it printed. A
    command over its timeout is killed with everything it started.
    """
    try:
        process = subprocess.Popen(
            arguments,
            env=env,
            stdout=subprocess.PIPE if capture else None,
            text=True,
            start_new_session=True,
        )
    except OSError as cause:
        return 127, f"cannot run {arguments[0]}: {cause}"
    try:
        output, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate()
        return 124, f"{arguments[0]} did not end within {timeout} seconds"
    return process.returncode, (output or "").strip()


def check(adapter_name, prefix, results_path):
    """Install the latest harness, run the live tests and append the outcome to the results."""
    adapter = ADAPTERS[adapter_name]
    version, stage = None, "install"
    code, output = command(["npm", "view", adapter.package, "version"], timeout=60, capture=True)
    if code == 0 and not VERSION.fullmatch(output):
        code, output = 1, f"npm answered {output!r}, not a version"
    if code == 0:
        version = output
        code, output = command(
            ["npm", "install", "--prefix", str(prefix), "--no-audit", "--no-fund",
             f"{adapter.package}@{version}"],
            timeout=INSTALL_TIMEOUT_SECONDS,
        )  # fmt: skip
    if code == 0:
        program = Path(prefix) / "node_modules" / ".bin" / adapter.executable
        stage = "version"
        code, output = command([str(program), "--version"], timeout=60, capture=True)
        if code == 0 and not prints_version(output, version):
            code, output = 1, f"`--version` printed {output!r}, not {version}"
    if code == 0:
        stage = "conformance"
        code, output = command(
            ["cargo", "nextest", "run", "--locked", "-p", f"cockpit-{adapter_name}",
             "--test", "conformance", "--run-ignored", "only"],
            env={**os.environ, adapter.program_variable: str(program)},
            timeout=TEST_TIMEOUT_SECONDS,
        )  # fmt: skip
        if code not in (0, NEXTEST_TEST_FAILURE):
            # A build or toolchain failure says nothing about the harness.
            stage = "cargo"
    result = {"adapter": adapter_name, "version": version, "passed": code == 0}
    if code != 0:
        result["stage"] = stage
        print(f"{adapter_name} {version} failed at {stage}: {output}")
    path = Path(results_path)
    results = json.loads(path.read_text()) if path.exists() else []
    path.write_text(json.dumps([*results, result]))
    print(json.dumps(result))
    return result


def prints_version(output, version):
    """Whether `output` holds `version` as a whole token, not inside a longer version."""
    return (
        re.search(rf"(?<![0-9A-Za-z.-])v?{re.escape(version)}(?![0-9A-Za-z-])", output) is not None
    )


def issue_marker(adapter_name):
    return f"<!-- harness-conformance adapter={adapter_name} -->"


def open_issues():
    pages = release_pr.api(
        "GET", release_pr.endpoint("issues?state=open&per_page=100"), paginate=True
    )
    return [
        issue
        for page in pages
        for issue in page
        if "pull_request" not in issue and issue["user"]["login"] == ISSUE_BOT
    ]


def report(results, dry_run):
    """Open or update an issue per failing adapter; close it when the adapter passes again."""
    issues = open_issues()
    for result in results:
        name, version = result["adapter"], result["version"] or "latest"
        marker = issue_marker(name)
        existing = next((issue for issue in issues if marker in (issue["body"] or "")), None)
        if result["passed"]:
            if existing:
                act(
                    dry_run,
                    f"close #{existing['number']}: {name} {version} passes again",
                    close_issue,
                    existing,
                    name,
                    version,
                )
            continue
        stage = result.get("stage", "conformance")
        body = failure_body(name, version, stage)
        if existing is None:
            act(
                dry_run,
                f"open issue: {name} {version} fails at {stage}",
                open_issue,
                name,
                version,
                stage,
                body,
            )
        elif existing["body"].replace("\r\n", "\n").split("\n\n")[0] == body.split("\n\n")[0]:
            print(f"{name} {version} is already reported in #{existing['number']}")
        else:
            act(
                dry_run,
                f"update #{existing['number']}: {name} {version} fails at {stage}",
                update_issue,
                existing,
                body,
            )


def failure_body(name, version, stage):
    """The issue text; its first paragraph says what failed, so a repeat night changes nothing."""
    return (
        f"{issue_marker(name)}\n{name} `{version}` fails at the {stage} stage.\n\n"
        f"Found by the nightly conformance run: {run_url()}\n\n"
        f"{name} stays on its tested-versions list, and the daemon still probes any other "
        "version at start. The run's log says which live check failed."
    )


def open_issue(name, version, stage, body):
    title = f"Harness conformance: {name} {version} fails at {stage}"
    release_pr.api("POST", release_pr.endpoint("issues"), {"title": title, "body": body})


def update_issue(issue, body):
    release_pr.api("PATCH", release_pr.endpoint(f"issues/{issue['number']}"), {"body": body})
    comment(issue["number"], f"The failure changed; see the description. {run_url()}")


def close_issue(issue, name, version):
    comment(issue["number"], f"{name} `{version}` passes the live checks again: {run_url()}")
    release_pr.api(
        "PATCH",
        release_pr.endpoint(f"issues/{issue['number']}"),
        {"state": "closed", "state_reason": "completed"},
    )


def comment(number, body):
    release_pr.api("POST", release_pr.endpoint(f"issues/{number}/comments"), {"body": body})


def act(dry_run, description, action, *arguments):
    if dry_run:
        print(f"dry run: would {description}")
        return
    print(description)
    action(*arguments)


def has_untested_pass(results, cwd):
    """Whether any passing version is missing from its adapter's tested-versions file."""
    return any(
        result["passed"]
        and result["version"]
        not in listed_versions((Path(cwd) / tested_versions_path(result["adapter"])).read_text())
        for result in results
    )


def propose(results, base, cwd, dry_run):
    """Open a pull request per adapter whose passing version is not yet on its tested list."""
    if not re.fullmatch(release_pr.SHA, base):
        raise Refusal("propose needs the full commit SHA of the checkout")
    if release_pr.git("rev-parse", "HEAD", cwd=cwd) != base or release_pr.git(
        "status", "--porcelain", cwd=cwd
    ):
        raise Refusal("propose needs a clean checkout at the base commit")
    open_pulls = None
    for result in results:
        if not result["passed"]:
            continue
        name, version = result["adapter"], result["version"]
        path = tested_versions_path(name)
        text = (Path(cwd) / path).read_text()
        if version in listed_versions(text):
            print(f"{name} {version} is already tested")
            continue
        branch = f"{BRANCH_PREFIX}{name}-{version}"
        if dry_run:
            print(f"dry run: would open pull request {branch} adding {version} to {path}")
            continue
        if open_pulls is None:
            open_pulls = open_conformance_pulls()
        if any(pull["head"]["ref"].startswith(f"{BRANCH_PREFIX}{name}-") for pull in open_pulls):
            print(f"{name} already has an open conformance pull request; not adding {version}")
            continue
        if release_pr.remote_head(branch, cwd):
            print(f"{branch} already exists; never overwriting it")
            continue
        head = commit_version(name, version, text, base, cwd)
        release_pr.push_branch(branch, head, cwd)
        pull = release_pr.api(
            "POST",
            release_pr.endpoint("pulls"),
            {
                "title": f"chore(providers): {name} {version} passes the live conformance checks",
                "head": branch,
                "base": "main",
                "body": pull_body(name, version),
            },
        )
        print(f"opened #{pull['number']} for {name} {version}")
        release_pr.git("reset", "--hard", base, cwd=cwd)


def open_conformance_pulls():
    pages = release_pr.api(
        "GET", release_pr.endpoint("pulls?state=open&base=main&per_page=100"), paginate=True
    )
    return [
        pull
        for page in pages
        for pull in page
        if pull["user"]["login"] == release_pr.BOT and pull["user"]["id"] == release_pr.BOT_ID
    ]


def commit_version(name, version, text, base, cwd):
    """Commit one added line to the adapter's tested-versions file, on top of `base`."""
    path = tested_versions_path(name)
    separator = "" if text.endswith("\n") else "\n"
    (Path(cwd) / path).write_text(f"{text}{separator}{version}\n")
    release_pr.git("add", "--", str(path), cwd=cwd)
    environment = {
        **release_pr.generation_environment(),
        "GIT_AUTHOR_NAME": release_pr.BOT,
        "GIT_COMMITTER_NAME": release_pr.BOT,
        "GIT_AUTHOR_EMAIL": f"{release_pr.BOT_ID}+{release_pr.BOT}@users.noreply.github.com",
        "GIT_COMMITTER_EMAIL": f"{release_pr.BOT_ID}+{release_pr.BOT}@users.noreply.github.com",
    }
    release_pr.git(
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-m",
        f"chore(providers): {name} {version} passes the live conformance checks",
        cwd=cwd,
        env=environment,
    )
    head = release_pr.git("rev-parse", "HEAD", cwd=cwd)
    changed = release_pr.git("diff", "--numstat", "--no-renames", base, head, cwd=cwd)
    if changed.split() != ["1", "0", str(path)]:
        raise Refusal(f"the commit must add one line to {path} and nothing else")
    return head


def pull_body(name, version):
    return (
        f"The nightly conformance run installed {name} `{version}`, the latest release, and its "
        "live conformance checks passed: the version prints as the adapter reads it, every flag "
        "and variable the adapter passes is still there, and the runtime probe finds nothing "
        f"missing. Run: {run_url()}\n\n"
        f"This adds `{version}` to `{tested_versions_path(name)}`, so the adapter reports it as "
        "`tested` and no longer probes it at daemon start. Merge it when the checks are green; "
        "nothing merges it for you.\n\n"
        "Opened by `tools/harness_conformance.py` with the release proposal App; "
        "see docs/PROVIDER_CONFORMANCE.md."
    )


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check_parser = commands.add_parser("check", help="test one adapter's latest harness")
    check_parser.add_argument("adapter", choices=sorted(ADAPTERS))
    check_parser.add_argument("--prefix", required=True, type=Path)
    check_parser.add_argument("--results", required=True, type=Path)
    report_parser = commands.add_parser("report", help="open, update or close failure issues")
    propose_parser = commands.add_parser("propose", help="open pull requests for passes")
    propose_parser.add_argument("--base", required=True)
    untested_parser = commands.add_parser(
        "untested", help="print true when a passing version is not yet on its tested list"
    )
    untested_parser.add_argument("--results", required=True, type=Path)
    for sub in (report_parser, propose_parser):
        sub.add_argument("--results", required=True, type=Path)
        sub.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(arguments)
    try:
        if args.command == "check":
            check(args.adapter, args.prefix, args.results)
        elif args.command == "untested":
            print(str(has_untested_pass(read_results(args.results), release_pr.ROOT)).lower())
        elif args.command == "report":
            report(read_results(args.results), args.dry_run)
        else:
            propose(read_results(args.results), args.base, release_pr.ROOT, args.dry_run)
    except Refusal as refusal:
        print(f"refused: {refusal}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

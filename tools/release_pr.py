#!/usr/bin/env python3
"""Prepare a local release proposal, then publish an immutable draft without rewriting history.

Run prepare only in a disposable, clean checkout of --base. Publish requires the
repository-scoped Release App token through GH_TOKEN. Neither command creates tags/releases.
"""

import argparse
import copy
import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

REPOSITORY = "Androlax2/agent-cockpit"
ORIGIN = f"https://github.com/{REPOSITORY}.git"
BOT = "agent-cockpit-release[bot]"
ALLOWED = {"Cargo.toml", "Cargo.lock", "CHANGELOG.md"}
SHA = r"[0-9a-f]{40}"
MARKER = re.compile(rf"<!-- cockpit-release-proposal v1 base=({SHA}) head=({SHA}) -->")
ROOT = Path(__file__).resolve().parents[1]


class Refusal(Exception):
    pass


def run(args, cwd, *, env=None, input_text=None, ok=(0,)):
    result = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        input=input_text,
        text=True,
        capture_output=True,
        timeout=300,
        check=False,
    )
    if result.returncode not in ok:
        # API payloads, credential-helper diagnostics and debug logs need not reach logs.
        raise Refusal(
            f"{args[0]} failed (exit {result.returncode}); inspect the operation before retrying"
        )
    return result.stdout.strip()


def git(*args, cwd, env=None, ok=(0,)):
    return run(["git", "-c", "core.hooksPath=/dev/null", *args], cwd, env=env, ok=ok)


def clean_environment():
    env = dict(os.environ)
    env.pop("GH_DEBUG", None)
    env.update(GH_HOST="github.com", GH_PROMPT_DISABLED="1")
    return env


def generation_environment():
    env = clean_environment()
    for key in (
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GIT_TOKEN",
        "GITLAB_TOKEN",
        "GITEA_TOKEN",
        "BITBUCKET_TOKEN",
        "AZURE_DEVOPS_TOKEN",
    ):
        env.pop(key, None)
    env.update(CARGO_NET_OFFLINE="true", GIT_CLIFF_OFFLINE="true")
    return env


def api(method, path, data=None, paginate=False):
    args = ["gh", "api", "--method", method, path]
    if paginate:
        args += ["--paginate", "--slurp"]
    if data is not None:
        args += ["--input", "-"]
    output = run(
        args, ROOT, env=clean_environment(), input_text=None if data is None else json.dumps(data)
    )
    return json.loads(output) if output else None


def endpoint(path):
    return f"repos/{REPOSITORY}/{path}"


def workspace_version(text):
    version = tomllib.loads(text)["workspace"]["package"]["version"]
    if not re.fullmatch(r"0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise Refusal("proposal must have a stable-shaped 0.x workspace version; no 1.0")
    return version


def validate_delta(base, head, cwd):
    """Only root version, local workspace lock versions and generated changelog may differ."""
    paths = set(git("diff", "--name-only", "--no-renames", base, head, cwd=cwd).splitlines())
    if not paths <= ALLOWED or not paths:
        raise Refusal("proposal changes files outside root version/lock/changelog, or is empty")
    before = tomllib.loads(git("show", f"{base}:Cargo.toml", cwd=cwd))
    after_text = git("show", f"{head}:Cargo.toml", cwd=cwd)
    version = workspace_version(after_text)
    after = tomllib.loads(after_text)
    after["workspace"]["package"]["version"] = before["workspace"]["package"]["version"]
    if before != after:
        raise Refusal("proposal changes manifest fields other than the workspace version")
    before_lock = tomllib.loads(git("show", f"{base}:Cargo.lock", cwd=cwd))
    after_lock = tomllib.loads(git("show", f"{head}:Cargo.lock", cwd=cwd))
    old_packages = {(p["name"], p.get("source", "")): p for p in before_lock["package"]}
    # Use complete package records for externals: duplicate versions must not collapse in a dict.
    if sorted(
        json.dumps(p, sort_keys=True) for p in before_lock["package"] if "source" in p
    ) != sorted(json.dumps(p, sort_keys=True) for p in after_lock["package"] if "source" in p):
        raise Refusal("proposal changes external dependencies")
    normalized = copy.deepcopy(after_lock)
    for package in normalized["package"]:
        if "source" not in package:
            old = old_packages.get((package["name"], ""))
            if not old or package["version"] != version:
                raise Refusal("proposal has inconsistent local workspace versions")
            package["version"] = old["version"]
    if normalized != before_lock:
        raise Refusal("proposal changes lockfile fields beyond workspace package versions")
    if not git("show", f"{head}:CHANGELOG.md", cwd=cwd).startswith("# Changelog"):
        raise Refusal("proposal needs a generated CHANGELOG.md")
    return version


def prepare(base, cwd):
    if git("rev-parse", "HEAD", cwd=cwd) != base or git("status", "--porcelain", cwd=cwd):
        raise Refusal("prepare needs a clean disposable checkout at the exact base")
    if git("symbolic-ref", "--quiet", "HEAD", cwd=cwd, ok=(0, 1)):
        raise Refusal("prepare needs a detached disposable checkout, not a working branch")
    if git("rev-parse", "--is-shallow-repository", cwd=cwd) != "false":
        raise Refusal("prepare needs full Git history")
    env = generation_environment()
    run(["release-plz", "update", "--config", "release-plz.toml"], cwd, env=env)
    changelog = run(["git-cliff", "--offline", "--config", "cliff.toml"], cwd, env=env)
    (cwd / "CHANGELOG.md").write_text(changelog + "\n")
    paths = set(git("diff", "--name-only", "--no-renames", cwd=cwd).splitlines())
    paths.update(git("ls-files", "--others", "--exclude-standard", cwd=cwd).splitlines())
    if not paths <= ALLOWED:
        raise Refusal("generator changed a member manifest or another unexpected file")
    git("add", "--", *sorted(ALLOWED), cwd=cwd)
    tree = git("write-tree", cwd=cwd)
    version = validate_delta(base, tree, cwd)
    stamp = git("show", "-s", "--format=%ct", base, cwd=cwd)
    env.update(
        GIT_AUTHOR_NAME=BOT,
        GIT_COMMITTER_NAME=BOT,
        GIT_AUTHOR_EMAIL=f"{BOT}@users.noreply.github.com",
        GIT_COMMITTER_EMAIL=f"{BOT}@users.noreply.github.com",
        GIT_AUTHOR_DATE=f"@{stamp} +0000",
        GIT_COMMITTER_DATE=f"@{stamp} +0000",
    )
    git(
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-m",
        f"chore(release): prepare {version}",
        cwd=cwd,
        env=env,
    )
    print(f"prepared={git('rev-parse', 'HEAD', cwd=cwd)}")
    print(f"version={version}")


def remote_git(*args, cwd):
    # gh reads the short-lived App token from the environment; it never enters argv/remote URLs.
    return git(
        "-c",
        "credential.helper=",
        "-c",
        "credential.helper=!gh auth git-credential",
        *args,
        cwd=cwd,
        env=clean_environment(),
    )


def remote_head(branch, cwd):
    ref = "refs/heads/" + branch
    output = remote_git("ls-remote", "--heads", "origin", ref, cwd=cwd)
    if not output:
        return None
    lines = output.splitlines()
    if len(lines) != 1 or lines[0].split()[1:] != [ref]:
        raise Refusal("remote branch lookup was ambiguous")
    return lines[0].split()[0]


def current_main(base):
    if api("GET", endpoint("git/ref/heads/main"))["object"]["sha"] != base:
        raise Refusal("main advanced; leave every existing proposal intact and run on current main")


def owned_draft(pr):
    marker = MARKER.findall(pr.get("body") or "")
    return (
        len(marker) == 1
        and pr["state"] == "open"
        and pr["draft"]
        and pr["user"]["login"] == BOT
        and pr["user"]["type"] == "Bot"
        and pr["head"]["repo"] is not None
        and pr["head"]["repo"]["full_name"] == REPOSITORY
        and pr["base"]["repo"]["full_name"] == REPOSITORY
        and pr["base"]["ref"] == "main"
        and pr["head"]["ref"] == "release-plz/" + marker[0][0]
        and pr["head"]["sha"] == marker[0][1]
    )


def verify_replacement(number, branch, head, base, cwd):
    current_main(base)
    pr = api("GET", endpoint(f"pulls/{number}"))
    if (
        not owned_draft(pr)
        or pr["head"]["ref"] != branch
        or pr["head"]["sha"] != head
        or remote_head(branch, cwd) != head
    ):
        raise Refusal("replacement draft no longer matches the verified proposal")
    return pr


def comment_once(number, body):
    pages = api("GET", endpoint(f"issues/{number}/comments?per_page=100"), paginate=True)
    if not any(
        comment["body"] == body and comment["user"]["login"] == BOT
        for page in pages
        for comment in page
    ):
        api("POST", endpoint(f"issues/{number}/comments"), {"body": body})


def publish(base, cwd):
    origin = git("remote", "get-url", "origin", cwd=cwd)
    if origin not in (ORIGIN, ORIGIN.removesuffix(".git")):
        raise Refusal("origin is not the authorized repository")
    if git("status", "--porcelain", cwd=cwd):
        raise Refusal("publish needs the clean prepared proposal commit")
    head = git("rev-parse", "HEAD", cwd=cwd)
    if git("rev-list", "--parents", "-n", "1", head, cwd=cwd).split() != [head, base]:
        raise Refusal("proposal must be exactly one new commit on the requested base")
    version = validate_delta(base, head, cwd)
    branch = "release-plz/" + base
    current_main(base)
    pages = api("GET", endpoint("pulls?state=all&base=main&per_page=100"), paginate=True)
    pulls = [pr for page in pages for pr in page]
    targets = [pr for pr in pulls if pr["head"]["ref"] == branch]
    if len(targets) > 1 or (
        targets and (not owned_draft(targets[0]) or targets[0]["head"]["sha"] != head)
    ):
        raise Refusal("existing target PR is closed, edited, non-draft or not automation-owned")
    remote = remote_head(branch, cwd)
    if remote and remote != head:
        raise Refusal("existing branch has an unexpected head; never overwrite it")
    if targets and not remote:
        raise Refusal("existing draft branch is missing; do not recreate it silently")
    if not remote:
        current_main(base)
        remote_git("push", "origin", f"{head}:refs/heads/{branch}", cwd=cwd)
        if remote_head(branch, cwd) != head:
            raise Refusal("normal push did not establish the expected proposal branch")
    current_main(base)
    if targets:
        number = targets[0]["number"]
    else:
        body = (
            f"Candidate workspace version: `{version}`. Generated locally with release-plz update and git-cliff.\n\n"
            "Draft for review only. Do not merge until Theo decides the release policy. "
            "This workflow cannot tag or publish. The branch and commit are retained when superseded.\n\n"
            f"<!-- cockpit-release-proposal v1 base={base} head={head} -->"
        )
        pr = api(
            "POST",
            endpoint("pulls"),
            {
                "title": f"chore(release): prepare {version}",
                "head": branch,
                "base": "main",
                "draft": True,
                "body": body,
            },
        )
        number = pr["number"]
    replacement = verify_replacement(number, branch, head, base, cwd)
    for old in pulls:
        if old["number"] == number or not owned_draft(old):
            continue
        old_base = MARKER.findall(old["body"])[0][0]
        if git("merge-base", old_base, base, cwd=cwd, ok=(0, 1, 128)) != old_base:
            continue  # Only supersede proposals whose source revision is in this main history.
        fresh = api("GET", endpoint(f"pulls/{old['number']}"))
        if not owned_draft(fresh) or fresh["head"]["sha"] != old["head"]["sha"]:
            continue  # Preserve a draft that a human edited/promoted while this run worked.
        verify_replacement(number, branch, head, base, cwd)
        comment_once(number, f"Supersedes #{old['number']}; its branch and commits are retained.")
        comment_once(
            old["number"], f"Superseded by #{number}; this branch and its commits are retained."
        )
        verify_replacement(number, branch, head, base, cwd)
        latest = api("GET", endpoint(f"pulls/{old['number']}"))
        if owned_draft(latest) and latest["head"]["sha"] == old["head"]["sha"]:
            api("PATCH", endpoint(f"pulls/{old['number']}"), {"state": "closed"})
    print(f"proposal={replacement['html_url']}")


def main(argv=None, cwd=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "publish"))
    parser.add_argument("--base", required=True)
    args = parser.parse_args(argv)
    try:
        if not re.fullmatch(SHA, args.base):
            raise Refusal("base must be a full commit SHA")
        directory = Path.cwd() if cwd is None else Path(cwd)
        {"prepare": prepare, "publish": publish}[args.command](args.base, directory)
    except (Refusal, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        message = (
            str(error) if isinstance(error, Refusal) else "invalid metadata or unavailable command"
        )
        print(f"release proposal refused: {message}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

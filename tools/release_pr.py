#!/usr/bin/env python3
"""Prepare a local release proposal, then publish an immutable draft without rewriting history.

Run prepare only in a disposable, clean checkout of --base. Publish uses a
read/PR-write proposal App token through GH_TOKEN and a Git-only PROPOSAL_GIT_SSH_KEY.
Neither command creates tags/releases.
"""

import argparse
import copy
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

import release

REPOSITORY = "Androlax2/agent-cockpit"
ORIGIN = f"https://github.com/{REPOSITORY}.git"
BOT = "agent-cockpit-proposal[bot]"
BOT_ID = 351595931
# Pinned from https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints
GITHUB_HOST_KEY = (
    "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl\n"
)
ALLOWED = {"Cargo.toml", "Cargo.lock", "CHANGELOG.md"}
SHA = r"[0-9a-f]{40}"
MARKER = re.compile(rf"<!-- cockpit-release-proposal v1 base=({SHA}) head=({SHA}) -->")
VERSION = r"0\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:alpha|beta)\.[1-9][0-9]*)?"
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
    return run(
        ["git", "-c", "core.hooksPath=/dev/null", *args],
        cwd,
        env=generation_environment() if env is None else env,
        ok=ok,
    )


def clean_environment():
    env = dict(os.environ)
    env.pop("GH_DEBUG", None)
    env.pop("PROPOSAL_GIT_SSH_KEY", None)
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
    if not re.fullmatch(VERSION, version):
        raise Refusal("proposal must have a 0.x alpha/beta or legacy core version; no 1.0")
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
    changelog = git("show", f"{head}:CHANGELOG.md", cwd=cwd)
    if not changelog.startswith("# Changelog"):
        raise Refusal("proposal needs a generated CHANGELOG.md")
    if "-" in version:
        headings = re.findall(r"(?m)^## (.+)$", changelog)
        expected = rf"{re.escape(version)}(?: \([^\n]+\))?"
        if (
            not headings
            or not re.fullmatch(expected, headings[0])
            or sum(bool(re.fullmatch(expected, heading)) for heading in headings) != 1
        ):
            raise Refusal("proposal changelog must start with its exact prerelease version once")
    return version


def prepare(base, cwd, channel):
    if git("rev-parse", "HEAD", cwd=cwd) != base or git("status", "--porcelain", cwd=cwd):
        raise Refusal("prepare needs a clean disposable checkout at the exact base")
    if git("symbolic-ref", "--quiet", "HEAD", cwd=cwd, ok=(0, 1)):
        raise Refusal("prepare needs a detached disposable checkout, not a working branch")
    if git("rev-parse", "--is-shallow-repository", cwd=cwd) != "false":
        raise Refusal("prepare needs full Git history")
    version = release.next_version(
        release.workspace_version(cwd, base), channel, release.release_tags(cwd, base)
    )
    if release.git_succeeds("show-ref", "--verify", "--quiet", f"refs/tags/v{version}", cwd=cwd):
        raise Refusal(f"v{version} already exists; never reuse or move a tag")
    env = generation_environment()
    run(["release-plz", "update", "--config", "release-plz.toml"], cwd, env=env)
    release.stamp(version, cwd)
    run(["cargo", "update", "--workspace", "--offline"], cwd, env=env)
    stamp = git("show", "-s", "--format=%ct", base, cwd=cwd)
    cliff = ["git-cliff", "--offline", "--config", "cliff.toml"]
    context = json.loads(run([*cliff, "--tag", f"v{version}", "--context"], cwd, env=env))
    if not context or context[0]["version"] != f"v{version}":
        raise Refusal("changelog generator did not produce the proposed version")
    # A synthetic tag otherwise uses wall-clock time and changes the immutable retry commit.
    context[0]["timestamp"] = int(stamp)
    with tempfile.TemporaryDirectory(prefix="release-changelog-") as directory:
        context_file = Path(directory) / "context.json"
        context_file.write_text(json.dumps(context))
        changelog = run([*cliff, "--from-context", str(context_file)], cwd, env=env)
    (cwd / "CHANGELOG.md").write_text(changelog + "\n")
    paths = set(git("diff", "--name-only", "--no-renames", cwd=cwd).splitlines())
    paths.update(git("ls-files", "--others", "--exclude-standard", cwd=cwd).splitlines())
    if not paths <= ALLOWED:
        raise Refusal("generator changed a member manifest or another unexpected file")
    git("add", "--", *sorted(ALLOWED), cwd=cwd)
    tree = git("write-tree", cwd=cwd)
    version = validate_delta(base, tree, cwd)
    env.update(
        GIT_AUTHOR_NAME=BOT,
        GIT_COMMITTER_NAME=BOT,
        GIT_AUTHOR_EMAIL=f"{BOT_ID}+{BOT}@users.noreply.github.com",
        GIT_COMMITTER_EMAIL=f"{BOT_ID}+{BOT}@users.noreply.github.com",
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


def push_proposal(base, head, cwd):
    if not re.fullmatch(SHA, base) or not re.fullmatch(SHA, head):
        raise Refusal("proposal push needs full base and head commit SHAs")
    private_key = os.environ.get("PROPOSAL_GIT_SSH_KEY")
    if not private_key:
        raise Refusal("proposal push requires PROPOSAL_GIT_SSH_KEY")
    environment = clean_environment()
    for name in ("GH_TOKEN", "GITHUB_TOKEN", "SSH_AUTH_SOCK"):
        environment.pop(name, None)
    if git(
        "config",
        "--name-only",
        "--get-regexp",
        r"^url\..*\.(insteadof|pushinsteadof)$",
        cwd=cwd,
        env=environment,
        ok=(0, 1),
    ):
        raise Refusal("proposal push refuses Git URL rewrites")
    with tempfile.TemporaryDirectory(prefix="proposal-git-") as directory:
        key_path = Path(directory) / "key"
        key_path.touch(mode=0o600)
        key_path.write_text(private_key.rstrip("\n") + "\n")
        known_hosts = Path(directory) / "known_hosts"
        known_hosts.write_text(GITHUB_HOST_KEY)
        environment.update(
            GIT_SSH_VARIANT="ssh",
            GIT_SSH_COMMAND=shlex.join(
                [
                    "ssh",
                    "-F",
                    "/dev/null",
                    "-i",
                    str(key_path),
                    "-o",
                    "IdentitiesOnly=yes",
                    "-o",
                    "IdentityAgent=none",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    "-o",
                    f"UserKnownHostsFile={known_hosts}",
                    "-o",
                    "GlobalKnownHostsFile=/dev/null",
                    "-o",
                    "HostKeyAlgorithms=ssh-ed25519",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ForwardAgent=no",
                    "-o",
                    "ConnectTimeout=30",
                ]
            ),
        )
        git(
            "-c",
            "credential.helper=",
            "push",
            "--no-follow-tags",
            f"git@github.com:{REPOSITORY}.git",
            f"{head}:refs/heads/release-plz/{base}",
            cwd=cwd,
            env=environment,
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


def release_steps(version, base):
    """The copy-paste dispatch and approval steps, and what the dispatch refuses first (#669)."""
    dispatch = f"gh workflow run prerelease.yml -R {REPOSITORY} --ref main -f dry_run="
    actions = f"https://github.com/{REPOSITORY}/actions/workflows"
    return (
        "## How to release\n\n"
        f"1. Dry run: `{dispatch}true`, then approve the `release` environment on the run "
        f"({actions}/prerelease.yml). It prints `dry run: would tag {base} as v{version}` "
        "and creates nothing.\n"
        f"2. Release: `{dispatch}false`, then approve the `release` environment on the run. "
        f"It tags `{base}` as `v{version}`.\n"
        "3. The tag starts the Release workflow: approve its `release` jobs as its gates pass "
        f"({actions}/release.yml). It drafts, then publishes the release.\n\n"
        "Among its checks, the dispatch refuses before tagging unless:\n\n"
        f"- {release.ALPHA_CHECKLIST} is closed; once {release.BETA_CHECKLIST} closes, "
        "only a beta is cut;\n"
        f"- an alpha's source carries `{release.KNOWN_GAPS}`;\n"
        f"- `main` is still `{base}` and this is the only open release proposal, unedited;\n"
        f"- `v{version}` is the next prerelease number and no tag has it yet.\n\n"
    )


def proposal_body(version, base, head, *, with_release_steps=True):
    if "-" in version:
        return (
            f"Next prerelease: `v{version}`. Generated locally with release-plz update and git-cliff.\n\n"
            "Draft for review only; prerelease dispatch verifies this version and tags its main source. "
            "Do not merge this draft. Separate release approval and all release gates still apply. "
            "The branch and commit are retained when superseded.\n\n"
            + (release_steps(version, base) if with_release_steps else "")
            + f"<!-- cockpit-release-proposal v1 base={base} head={head} -->"
        )
    return (
        f"Candidate workspace version: `{version}`. Generated locally with release-plz update and git-cliff.\n\n"
        "Draft for review only. Do not merge until Theo decides the release policy. "
        "This workflow cannot tag or publish. The branch and commit are retained when superseded.\n\n"
        f"<!-- cockpit-release-proposal v1 base={base} head={head} -->"
    )


def owned_draft(pr, *, supersedable=False):
    marker = MARKER.findall(pr.get("body") or "")
    title = re.fullmatch(rf"chore\(release\): prepare ({VERSION})", pr.get("title", ""))
    if len(marker) != 1 or title is None:
        return False
    accepted_bodies = [proposal_body(title[1], *marker[0])]
    if supersedable:
        # Drafts published before #669, such as #689, lack the release steps. Only retiring
        # them accepts that body; dispatch still consumes nothing but the current one.
        # Remove this, and with_release_steps, once no pre-#669 draft is open.
        accepted_bodies.append(proposal_body(title[1], *marker[0], with_release_steps=False))
    return (
        pr["body"] in accepted_bodies
        and pr["state"] == "open"
        and pr["draft"]
        and pr["user"]["login"] == BOT
        and pr["user"]["id"] == BOT_ID
        and pr["user"]["type"] == "Bot"
        and pr["head"]["repo"] is not None
        and pr["head"]["repo"]["full_name"] == REPOSITORY
        and pr["base"]["repo"]["full_name"] == REPOSITORY
        and pr["base"]["ref"] == "main"
        and pr["head"]["ref"] == "release-plz/" + marker[0][0]
        and pr["head"]["sha"] == marker[0][1]
    )


def unchanged_old_commit(pr, cwd):
    base, head = MARKER.findall(pr["body"])[0]
    if remote_head(pr["head"]["ref"], cwd) != head:
        return False
    remote_git("fetch", "--no-tags", "origin", head, cwd=cwd)
    if git("rev-list", "--parents", "-n", "1", head, cwd=cwd).split() != [head, base]:
        return False
    try:
        version = validate_delta(base, head, cwd)
    except Refusal:
        return False
    return pr["title"] == f"chore(release): prepare {version}"


def verified_open_proposal(base, cwd):
    """Capture the verified draft identity; later changes do not revoke this snapshot."""
    if git("remote", "get-url", "origin", cwd=cwd) not in (ORIGIN, ORIGIN.removesuffix(".git")):
        raise Refusal("origin is not the authorized repository")
    current_main(base)
    pages = api("GET", endpoint("pulls?state=open&base=main&per_page=100"), paginate=True)
    drafts = [
        pr
        for page in pages
        for pr in page
        if pr["state"] == "open" and pr["head"]["ref"].startswith("release-plz/")
    ]
    if len(drafts) != 1:
        raise Refusal("prerelease selection needs exactly one open release proposal")
    pr = drafts[0]
    if not owned_draft(pr):
        raise Refusal("open release proposal is edited, non-draft or not App-owned")
    if MARKER.findall(pr["body"])[0][0] != base:
        raise Refusal("open release proposal is not based on current main")
    if not unchanged_old_commit(pr, cwd):
        raise Refusal("open release proposal commit, title or version files are inconsistent")
    version = workspace_version(git("show", f"{pr['head']['sha']}:Cargo.toml", cwd=cwd))
    if release.parse_tag(f"v{version}")["channel"] == "stable":
        raise Refusal("open release proposal must carry an exact alpha or beta version")
    fresh = verify_replacement(pr["number"], pr["head"]["ref"], pr["head"]["sha"], base, cwd)
    if fresh["title"] != pr["title"] or fresh["body"] != pr["body"]:
        raise Refusal("open release proposal changed during selection")
    pages = api("GET", endpoint("pulls?state=open&base=main&per_page=100"), paginate=True)
    remaining = [
        candidate
        for page in pages
        for candidate in page
        if candidate["state"] == "open" and candidate["head"]["ref"].startswith("release-plz/")
    ]
    if [candidate["number"] for candidate in remaining] != [pr["number"]]:
        raise Refusal("open release proposal set changed during selection")
    latest = remaining[0]
    if (
        not owned_draft(latest)
        or latest["title"] != pr["title"]
        or latest["body"] != pr["body"]
        or latest["head"]["sha"] != pr["head"]["sha"]
        or remote_head(pr["head"]["ref"], cwd) != pr["head"]["sha"]
    ):
        raise Refusal("open release proposal changed during final verification")
    current_main(base)
    return {"version": version, "number": pr["number"], "head": pr["head"]["sha"]}


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
        comment["body"] == body
        and comment["user"]["login"] == BOT
        and comment["user"]["id"] == BOT_ID
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
    if release.parse_tag(f"v{version}")["channel"] == "stable":
        raise Refusal("new proposals must carry an exact alpha or beta version")
    branch = "release-plz/" + base
    current_main(base)
    pages = api("GET", endpoint("pulls?state=all&base=main&per_page=100"), paginate=True)
    pulls = [pr for page in pages for pr in page]
    targets = [pr for pr in pulls if pr["head"]["ref"] == branch]
    if len(targets) > 1 or (targets and not owned_draft(targets[0])):
        raise Refusal("existing target PR is closed, edited, non-draft or not automation-owned")
    if targets and targets[0]["head"]["sha"] != head:
        raise Refusal(
            "existing immutable proposal has a different head; "
            "channel transitions require a deliberate new main commit"
        )
    remote = remote_head(branch, cwd)
    if remote and remote != head:
        raise Refusal("existing branch has an unexpected head; never overwrite it")
    if targets and not remote:
        raise Refusal("existing draft branch is missing; do not recreate it silently")
    if not remote:
        current_main(base)
        push_proposal(base, head, cwd)
        if remote_head(branch, cwd) != head:
            raise Refusal("normal push did not establish the expected proposal branch")
    current_main(base)
    if targets:
        number = targets[0]["number"]
    else:
        body = proposal_body(version, base, head)
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
        if old["number"] == number or not owned_draft(old, supersedable=True):
            continue
        old_base = MARKER.findall(old["body"])[0][0]
        if git("merge-base", old_base, base, cwd=cwd, ok=(0, 1, 128)) != old_base:
            continue  # Only supersede proposals whose source revision is in this main history.
        if not unchanged_old_commit(old, cwd):
            continue
        fresh = api("GET", endpoint(f"pulls/{old['number']}"))
        if not owned_draft(fresh, supersedable=True) or fresh["head"]["sha"] != old["head"]["sha"]:
            continue  # Preserve a draft that a human edited/promoted while this run worked.
        verify_replacement(number, branch, head, base, cwd)
        comment_once(number, f"Supersedes #{old['number']}; its branch and commits are retained.")
        comment_once(
            old["number"], f"Superseded by #{number}; this branch and its commits are retained."
        )
        verify_replacement(number, branch, head, base, cwd)
        latest = api("GET", endpoint(f"pulls/{old['number']}"))
        if (
            owned_draft(latest, supersedable=True)
            and latest["head"]["sha"] == old["head"]["sha"]
            and remote_head(latest["head"]["ref"], cwd) == old["head"]["sha"]
        ):
            api("PATCH", endpoint(f"pulls/{old['number']}"), {"state": "closed"})
    print(f"proposal={replacement['html_url']}")


def main(argv=None, cwd=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "publish"))
    parser.add_argument("--base", required=True)
    parser.add_argument("--channel", choices=release.CHANNELS)
    args = parser.parse_args(argv)
    try:
        if not re.fullmatch(SHA, args.base):
            raise Refusal("base must be a full commit SHA")
        directory = Path.cwd() if cwd is None else Path(cwd)
        if args.command == "prepare":
            if args.channel is None:
                raise Refusal("prepare requires an explicit --channel")
            prepare(args.base, directory, args.channel)
        else:
            publish(args.base, directory)
    except (
        Refusal,
        release.Refusal,
        OSError,
        ValueError,
        KeyError,
        TypeError,
        subprocess.SubprocessError,
    ) as error:
        message = (
            str(error)
            if isinstance(error, (Refusal, release.Refusal))
            else "invalid metadata or unavailable command"
        )
        print(f"release proposal refused: {message}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

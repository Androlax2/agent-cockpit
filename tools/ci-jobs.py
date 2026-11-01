#!/usr/bin/env python3
"""Print which CI jobs a change set needs, as `job=true|false` lines for $GITHUB_OUTPUT.

Usage: git diff --name-only BASE...HEAD | ci-jobs.py
       ci-jobs.py --all    (a push to main: every job runs)

Patterns follow GitHub's `paths` filters: `*` stays inside one folder, `**` crosses folders.
A path no job claims (rust-toolchain.toml, .cargo/deny.toml, assets/ ...) and an empty change
set run every job: a gap in these lists must show up as extra work, never as skipped checks.
The legacy apps/ suite has no CI job since 2026-10-31 (decided by Theo): it slowed every
greenfield change and leaves at the cutover; `make ci` stays available locally.

`platform` runs the macOS and WSL 2 jobs on a pull request only when it changes a crate whose
behaviour can differ by OS, the dependency set or ci.yml itself; every push to main runs them
on the whole tree, so a platform break that slips through shows on main (#397). A platform
crate has a `#[cfg]` on the OS, is tested by name in a platform job, or wraps an OS-provided
program or file system (tmux, the database). tools/tests/test_ci_jobs.py fails when a crate
with a platform `#[cfg]` or tested by a platform job is missing from PLATFORM_CRATES.
"""

import sys
from pathlib import PurePosixPath

PLATFORM_CRATES = [
    "cockpit-agent-channel",
    "cockpit-approvals",
    "cockpit-cli",
    "cockpit-daemon",
    "cockpit-processes",
    "cockpit-resource-admission",
    "cockpit-sessions",
    "cockpit-tmux",
    "cockpit-tui",
    "cockpit-turso",
]

JOB_PATHS = {
    "greenfield": ["crates/**", "Cargo.toml", "Cargo.lock", "Makefile", ".github/**"],
    "platform": [
        *(f"crates/{crate}/**" for crate in PLATFORM_CRATES),
        "Cargo.toml",
        "Cargo.lock",
        ".github/workflows/ci.yml",
    ],
    "docs": ["docs/**", "**/*.md"],
}


def jobs_claiming(path):
    return {
        job
        for job, patterns in JOB_PATHS.items()
        if any(PurePosixPath(path).full_match(pattern) for pattern in patterns)
    }


def jobs_for(changed_paths):
    claimed_per_path = [jobs_claiming(path) for path in changed_paths]
    if not changed_paths or any(not claimed for claimed in claimed_per_path):
        return set(JOB_PATHS)
    return set().union(*claimed_per_path)


def main(arguments):
    if arguments == ["--all"]:
        selected = set(JOB_PATHS)
    elif not arguments:
        selected = jobs_for([line for line in sys.stdin.read().splitlines() if line])
    else:
        print(__doc__, file=sys.stderr)
        return 2
    for job in JOB_PATHS:
        print(f"{job}={'true' if job in selected else 'false'}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

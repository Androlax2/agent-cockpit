#!/usr/bin/env python3
"""Print which CI jobs a change set needs, as `job=true|false` lines for $GITHUB_OUTPUT.

Usage: git diff --name-only BASE...HEAD | ci-jobs.py
       ci-jobs.py --all    (a push to main: every job runs)

Patterns follow GitHub's `paths` filters: `*` stays inside one folder, `**` crosses folders.
A path no job claims (rust-toolchain.toml, .cargo/deny.toml, assets/ ...) and an empty change
set run every job: a gap in these lists must show up as extra work, never as skipped checks.
"""

import sys
from pathlib import PurePosixPath

JOB_PATHS = {
    "greenfield": ["crates/**", "Cargo.toml", "Cargo.lock", "Makefile", ".github/**"],
    "legacy": ["apps/**", "config/**", "tools/**", "packaging/**", "Makefile", ".github/**"],
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

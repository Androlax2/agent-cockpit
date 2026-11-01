#!/usr/bin/env python3
"""Print which CI jobs a change set needs, as `job=true|false` lines for $GITHUB_OUTPUT.

Usage: git diff --name-only BASE...HEAD | ci-jobs.py
       ci-jobs.py --all    (a push to main: every job runs)

Patterns follow GitHub's `paths` filters: `*` stays inside one folder, `**` crosses folders.
A path no job claims (rust-toolchain.toml, .cargo/deny.toml, assets/ ...) and an empty change
set run every job: a gap in these lists must show up as extra work, never as skipped checks.
The legacy apps/ suite has no CI job since 2026-10-31 (decided by Theo): it slowed every
greenfield change and leaves at the cutover; `make ci` stays available locally.

`platform` runs the macOS and WSL 2 jobs on a pull request only when it changes a crate those
jobs build: the crates they test or run (PLATFORM_ROOTS) and every crate those depend on,
read from the manifests, so no dependency reaches main untested on a platform (#127, #397).
The root manifests and ci.yml run them too, and every push to main does.
"""

import sys
import tomllib
from pathlib import Path, PurePosixPath

CRATES = Path(__file__).resolve().parents[1] / "crates"

# The crates ci.yml's platform jobs test with `-p`, and those whose programs their tests run:
# the daemon's tests and the TUI's end-to-end harness start `cockpit` and `cockpit-claude-hook`.
PLATFORM_ROOTS = [
    "cockpit-cli",
    "cockpit-claude",
    "cockpit-daemon",
    "cockpit-processes",
    "cockpit-resource-admission",
    "cockpit-tui",
]


def path_dependencies(crate):
    manifest = tomllib.loads((CRATES / crate / "Cargo.toml").read_text())
    tables = [manifest, *manifest.get("target", {}).values()]
    return {
        (CRATES / crate / spec["path"]).resolve().name
        for table in tables
        for section in ("dependencies", "dev-dependencies", "build-dependencies")
        for spec in table.get(section, {}).values()
        if isinstance(spec, dict) and "path" in spec
    }


def platform_crates():
    built, pending = set(), list(PLATFORM_ROOTS)
    while pending:
        crate = pending.pop()
        if crate not in built:
            built.add(crate)
            pending.extend(path_dependencies(crate))
    return built


JOB_PATHS = {
    "greenfield": ["crates/**", "Cargo.toml", "Cargo.lock", "Makefile", ".github/**"],
    "platform": [
        *(f"crates/{crate}/**" for crate in sorted(platform_crates())),
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

#!/usr/bin/env python3
"""Print the languages CodeQL scans for a change, as `languages=<JSON list>` for $GITHUB_OUTPUT.

Usage: git diff --name-only BASE...HEAD | codeql-languages.py
       codeql-languages.py --all    (a push to main or the weekly schedule: every language)

`actions` is always scanned: the `main: CodeQL merge protection` ruleset waits for CodeQL results,
and the Actions scan is what produces them for a change that touches no Rust. `rust` is scanned
only when the change touches Rust code, a Cargo or toolchain file, or the CodeQL workflow itself
(#1261, decided by Theo). An empty change set scans both: a gap must show up as extra work.
"""

import json
import sys
from pathlib import PurePosixPath

ALWAYS = ["actions"]
RUST_PATHS = [
    "**/*.rs",
    "**/Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/**",
    ".github/workflows/codeql.yml",
    "tools/codeql-languages.py",
]


def touches_rust(path):
    return any(PurePosixPath(path).full_match(pattern) for pattern in RUST_PATHS)


def languages_for(changed_paths):
    if not changed_paths or any(touches_rust(path) for path in changed_paths):
        return ["rust", *ALWAYS]
    return ALWAYS


def main(arguments):
    if arguments == ["--all"]:
        languages = ["rust", *ALWAYS]
    elif not arguments:
        languages = languages_for([line for line in sys.stdin.read().splitlines() if line])
    else:
        print(__doc__, file=sys.stderr)
        return 2
    print(f"languages={json.dumps(languages)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

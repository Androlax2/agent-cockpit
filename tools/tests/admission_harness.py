#!/usr/bin/env python3
"""Test-only: run the shipped agent-build with its account home replaced by a test folder.

usage: admission_harness.py FIXTURE_HOME ARGV0 [ARGUMENTS...]

Lives in tools/tests, which is never stowed, installed or put on a live PATH. It loads the
repository's config/bin/.local/bin/agent-build unchanged, replaces only `account_home` inside this
process (the shipped file has no override of its own), sets sys.argv to ARGV0 + ARGUMENTS so the
shipped shim-name logic decides, then calls the shipped run() and exits with its code. Whatever the
tool starts afterwards is the real tool, through the shipped execv.
"""

import importlib.machinery
import importlib.util
import sys
from pathlib import Path

RUNNER = Path(__file__).resolve().parents[2] / "config/bin/.local/bin/agent-build"


def load_runner():
    loader = importlib.machinery.SourceFileLoader("agent_build_under_test", str(RUNNER))
    module = importlib.util.module_from_spec(importlib.util.spec_from_loader(loader.name, loader))
    loader.exec_module(module)
    return module


def main():
    if len(sys.argv) < 3:
        print("usage: admission_harness.py FIXTURE_HOME ARGV0 [ARGUMENTS...]", file=sys.stderr)
        return 2
    fixture_home, argv0, arguments = Path(sys.argv[1]), sys.argv[2], sys.argv[3:]
    if not fixture_home.is_absolute():
        print(f"admission_harness: fixture home must be absolute: {fixture_home}", file=sys.stderr)
        return 2
    runner = load_runner()
    runner.account_home = lambda: fixture_home
    sys.argv = [argv0, *arguments]
    return runner.run()


if __name__ == "__main__":
    sys.exit(main())

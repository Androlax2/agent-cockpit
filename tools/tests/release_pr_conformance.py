#!/usr/bin/env python3
"""Explicit pinned-tool check; no GitHub writes, user home or live provider fixtures."""

import contextlib
import io
import os
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from unittest.mock import patch

from test_release_pr import ROOT, Fixture, git, proposal


def main():
    with tempfile.TemporaryDirectory(prefix="release-pr-conformance-") as directory:
        parent = Path(directory)
        home = parent / "home"
        home.mkdir()
        environment = dict(os.environ)
        for key in ("GH_TOKEN", "GITHUB_TOKEN", "GIT_TOKEN", "GH_DEBUG"):
            environment.pop(key, None)
        environment.update(
            HOME=str(home),
            CARGO_HOME=str(parent / "cargo"),
            CARGO_NET_OFFLINE="true",
            GIT_CONFIG_GLOBAL="/dev/null",
            GIT_CONFIG_NOSYSTEM="1",
        )
        for tool, expected in [
            ("release-plz", "release-plz 0.4.2"),
            ("git-cliff", "git-cliff 2.14.2"),
        ]:
            version = subprocess.check_output(
                [tool, "--version"], text=True, env=environment
            ).strip()
            if version != expected:
                raise RuntimeError(f"Expected {expected}, found {version}")
        fixture = Fixture(parent)

        def prepare():
            subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "tools/release_pr.py"),
                    "prepare",
                    "--base",
                    fixture.base,
                ],
                cwd=fixture.root,
                env=environment,
                check=True,
            )
            return git(fixture.root, "rev-parse", "HEAD")

        first = prepare()
        version = tomllib.loads((fixture.root / "Cargo.toml").read_text())["workspace"]["package"][
            "version"
        ]
        assert version == "0.1.0", version
        assert "initial fixture" in (fixture.root / "CHANGELOG.md").read_text()

        def publish():
            with (
                patch.object(proposal, "api", fixture.api.call),
                patch.object(proposal, "ORIGIN", str(fixture.remote)),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                assert proposal.main(["publish", "--base", fixture.base], cwd=fixture.root) == 0

        publish()
        git(fixture.root, "checkout", "-q", "--detach", fixture.base)
        assert prepare() == first, "same base must generate an identical commit"
        publish()
        assert len(fixture.api.pulls) == 1
        # This tag belongs only to a disposable fixture, never the real repository.
        git(fixture.root, "tag", "v0.1.0", fixture.base)
        fixture.next_base()
        prepare()
        version = tomllib.loads((fixture.root / "Cargo.toml").read_text())["workspace"]["package"][
            "version"
        ]
        assert version == "0.1.1", version
        assert "fixture value" in (fixture.root / "CHANGELOG.md").read_text()
        publish()
        assert fixture.api.pulls[1]["state"] == "closed"
        assert fixture.api.pulls[2]["draft"]
        assert (
            git(fixture.remote, "rev-parse", "refs/heads/" + fixture.api.pulls[1]["head"]["ref"])
            == first
        )
        print(
            "PASS: initial version, fix bump, changelog, draft replacement and deterministic retry; fake GitHub only."
        )


if __name__ == "__main__":
    main()

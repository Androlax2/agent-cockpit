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

from test_release_pr import (
    ROOT,
    Fixture,
    fixture_environment,
    git,
    proposal,
    ssh_fixture_environment,
)


def main():
    with (
        tempfile.TemporaryDirectory(prefix="release-pr-conformance-") as directory,
        patch.dict(os.environ, fixture_environment(Path(directory)), clear=True),
    ):
        parent = Path(directory)
        (parent / "home").mkdir()
        environment = fixture_environment(parent)
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
        publication_environment = {**environment, **ssh_fixture_environment(fixture, parent)}
        # A historical non-conventional subject broke the first live draft's docs gate.
        git(fixture.root, "checkout", "-q", "main")
        (fixture.root / "placeholder.txt").write_text("fixture\n")
        fixture.base = fixture.commit(
            "fixture command <repo>: keep `cockpit stack <repo>` readable"
        )
        git(fixture.root, "push", "-q", "origin", "main")
        git(fixture.root, "checkout", "-q", "--detach", fixture.base)

        def prepare():
            subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "tools/release_pr.py"),
                    "prepare",
                    "--channel",
                    "alpha",
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
        assert version == "0.1.0-alpha.1", version
        assert "initial fixture" in (fixture.root / "CHANGELOG.md").read_text()
        changelog = (fixture.root / "CHANGELOG.md").read_text()
        problems = []
        if "the title format.\n\n## 0.1.0-alpha.1 (" not in changelog:
            problems.append("generated heading needs a blank line after the header (MD022)")
        if "fixture command &lt;repo&gt;: keep `cockpit stack <repo>` readable" not in changelog:
            problems.append("escape prose placeholders (MD033), preserving literal code spans")
        assert not problems, "; ".join(problems)

        def publish():
            with (
                patch.object(proposal, "api", fixture.api.call),
                patch.object(proposal, "ORIGIN", str(fixture.remote)),
                patch.dict(os.environ, publication_environment, clear=True),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                assert proposal.main(["publish", "--base", fixture.base], cwd=fixture.root) == 0

        publish()
        git(fixture.root, "checkout", "-q", "--detach", fixture.base)
        assert prepare() == first, "same base must generate an identical commit"
        publish()
        assert len(fixture.api.pulls) == 1
        # This tag belongs only to a disposable fixture, never the real repository.
        git(fixture.root, "tag", "v0.1.0-alpha.1", fixture.base)
        fixture.next_base()
        prepare()
        version = tomllib.loads((fixture.root / "Cargo.toml").read_text())["workspace"]["package"][
            "version"
        ]
        assert version == "0.1.0-alpha.2", version
        assert "fixture value" in (fixture.root / "CHANGELOG.md").read_text()
        assert "initial fixture" in (fixture.root / "CHANGELOG.md").read_text(), (
            "keep previous release history"
        )
        publish()
        assert fixture.api.pulls[1]["state"] == "closed"
        assert fixture.api.pulls[2]["draft"]
        assert (
            git(fixture.remote, "rev-parse", "refs/heads/" + fixture.api.pulls[1]["head"]["ref"])
            == first
        )
        print(
            "PASS: alpha.1/alpha.2 versions, changelog, draft replacement and deterministic retry; fake GitHub only."
        )


if __name__ == "__main__":
    main()

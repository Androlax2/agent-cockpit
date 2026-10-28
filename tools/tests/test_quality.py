import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

RUNNER = Path(__file__).resolve().parents[1] / "quality.py"


class QualityTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)

    def source(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        subprocess.run(["git", "add", "--", name], cwd=self.root, check=True)

    def run_check(self, *arguments, environment=None):
        return subprocess.run(
            [sys.executable, str(RUNNER), *arguments],
            cwd=self.root,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_extensionless_scripts_are_discovered_by_dialect(self):
        for name, content in {
            "bin/python tool": "#!/usr/bin/env python3\nprint('ok')\n",
            "bin/bash tool": "#!/bin/bash\necho ok\n",
            "bin/fish tool": "#!/usr/bin/env fish\necho ok\n",
            "fragment.sh": "echo ok\n",
            "README.md": "# Example\n",
        }.items():
            self.source(name, content)
        for language, expected in {
            "python": ["./bin/python tool"],
            "shell": ["./bin/bash tool", "./fragment.sh"],
            "fish": ["./bin/fish tool"],
            "markdown": ["./README.md"],
        }.items():
            with self.subTest(language=language):
                result = self.run_check(language, "--list")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), expected)

    def test_untracked_files_and_symlinks_are_not_checked(self):
        self.source("owned.py", "print('ok')\n")
        (self.root / "untracked.py").write_text("not python")
        (self.root / "linked.py").symlink_to("untracked.py")
        subprocess.run(["git", "add", "linked.py"], cwd=self.root, check=True)
        for language in ("python", "typos"):
            with self.subTest(language=language):
                result = self.run_check(language, "--list")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), ["./owned.py"])

    def test_missing_tool_is_a_named_failure(self):
        self.source("entry.py", "print('ok')\n")
        tools = self.root / "tools"
        tools.mkdir()
        (tools / "git").symlink_to(shutil.which("git"))
        result = self.run_check("python", environment=os.environ | {"PATH": str(tools)})
        self.assertEqual(result.returncode, 127, result.stderr)
        self.assertIn("ruff", result.stderr)

    def test_paths_with_spaces_reach_the_checker_as_single_arguments(self):
        self.source("python tool", "#!/usr/bin/env python3\nprint('ok')\n")
        tools = self.root / "fake-tools"
        tools.mkdir()
        checker = tools / "ruff"
        checker.write_text(
            f"#!{sys.executable}\nimport sys\n"
            "assert sys.argv[1:] in "
            "[['check', './python tool'], ['format', '--check', './python tool']], sys.argv\n"
        )
        checker.chmod(0o755)
        result = self.run_check(
            "python", environment=os.environ | {"PATH": f"{tools}:{os.environ['PATH']}"}
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_lint_failure_is_returned_to_the_caller(self):
        self.source("entry.py", "print('ok')\n")
        tools = self.root / "fake-tools"
        tools.mkdir()
        checker = tools / "ruff"
        checker.write_text(f"#!{sys.executable}\nraise SystemExit(19)\n")
        checker.chmod(0o755)
        result = self.run_check(
            "python", environment=os.environ | {"PATH": f"{tools}:{os.environ['PATH']}"}
        )
        self.assertEqual(result.returncode, 19, result.stderr)

    def test_fish_syntax_checks_every_file(self):
        self.source("a.fish", "echo ok\n")
        self.source("b.fish", "if true\n")
        result = self.run_check("fish")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Missing end", result.stderr)

    def test_sourced_posix_shell_is_not_checked_as_bash(self):
        self.source("fragment.sh", "# shellcheck shell=sh\nvalues=(one two)\n")
        result = self.run_check("shell")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SC3030", result.stdout)

    def test_local_links_resolve_spaces_and_heading_fragments(self):
        self.source("guide with spaces.md", "# Guide\n\n## Getting started\n")
        self.source("README.md", "# Docs\n\n[Guide](guide%20with%20spaces.md#getting-started)\n")
        result = self.run_check("links")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_missing_local_heading_is_a_failed_check(self):
        self.source("guide.md", "# Guide\n")
        self.source("README.md", "# Docs\n\n[Guide](guide.md#missing-heading)\n")
        result = self.run_check("links")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing-heading", result.stdout + result.stderr)

    def test_spelling_checks_tracked_hidden_source(self):
        # Keep the intentionally misspelled fixture out of this repository's spelling scan.
        misspelling = "t" + "eh"
        self.source(".local/bin/tool.py", f'print("{misspelling}")\n')
        config = self.root / ".config/typos.toml"
        config.parent.mkdir()
        shutil.copyfile(RUNNER.parents[1] / ".config/typos.toml", config)
        result = self.run_check("typos")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(misspelling, result.stdout + result.stderr)


class JavaScriptPolicyTests(unittest.TestCase):
    def test_runtime_globals_are_checked_in_their_execution_environment(self):
        for path, source, succeeds in [
            (
                "apps/cockpit/assets/remote/page.js",
                '"use strict";\nconsole.log(document.title);',
                True,
            ),
            ("apps/cockpit/assets/remote/page.js", "console.log(process.env);", False),
            ("config/codex/.codex/cockpit-bridge.mjs", "console.log(process.env);", True),
            ("config/codex/.codex/cockpit-bridge.mjs", "console.log(document.title);", False),
        ]:
            with self.subTest(path=path, source=source), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                shutil.copyfile(RUNNER.parents[1] / "biome.jsonc", root / "biome.jsonc")
                fixture = root / path
                fixture.parent.mkdir(parents=True)
                fixture.write_text(source)
                result = subprocess.run(
                    ["biome", "lint", path],
                    cwd=root,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(result.returncode == 0, succeeds, result.stderr)
                if not succeeds:
                    self.assertIn("noRestrictedGlobals", result.stderr)


if __name__ == "__main__":
    unittest.main()

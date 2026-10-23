import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from urllib.parse import parse_qs, urlsplit


REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/agent-github.sh"


class AgentGithubTokenTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.home = self.root / "home"
        (self.home / "IdeaProjects/shop").mkdir(parents=True)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.url_file = self.root / "opened-url"
        self.environment = {
            **os.environ,
            "HOME": str(self.home),
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "OPENED_URL": str(self.url_file),
        }
        self.environment.pop("ALLOW_WORKFLOWS", None)
        self.shim("git", 'echo https://github.com/example/shop.git')
        self.shim("xdg-open", 'printf "%s" "$1" > "$OPENED_URL"')
        self.shim("secret-tool", '[ "$1" != lookup ] || echo fake-test-token')
        self.shim("curl", '''
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) body=$2; shift 2 ;;
        -D) headers=$2; shift 2 ;;
        *) shift ;;
    esac
done
if [ -n "${body:-}" ]; then
    printf '%s' '{"private":false,"full_name":"example/shop"}' > "$body"
    printf 'github-authentication-token-expiration: tomorrow\r\n' > "$headers"
    printf 200
else
    printf '[]'
fi
''')

    def tearDown(self):
        self.directory.cleanup()

    def shim(self, name, body):
        path = self.bin / name
        path.write_text(f"#!/bin/sh\nset -eu\n{body}\n")
        path.chmod(0o755)

    def create_token(self, allow_workflows=None, through_make=False):
        environment = dict(self.environment)
        if allow_workflows is not None:
            environment["ALLOW_WORKFLOWS"] = allow_workflows
        arguments = [str(SCRIPT), "token", "shop"]
        if through_make:
            arguments = ["make", "--no-print-directory", "agent-github-token", "PROJECT=shop"]
            if allow_workflows is not None:
                arguments.append(f"ALLOW_WORKFLOWS={allow_workflows}")
        return subprocess.run(arguments, cwd=REPO, env=environment, capture_output=True, text=True)

    def token_parameters(self):
        return parse_qs(urlsplit(self.url_file.read_text()).query)

    def test_default_token_does_not_request_workflow_access(self):
        result = self.create_token()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("workflows", self.token_parameters())
        self.assertNotIn("Pull requests, Workflows", result.stdout)

    def test_regular_repository_permissions_are_preserved(self):
        result = self.create_token()
        self.assertEqual(result.returncode, 0, result.stderr)
        permissions = self.token_parameters()
        for permission, expected in {
            "contents": ["write"], "issues": ["write"], "pull_requests": ["write"],
            "actions": ["read"], "metadata": ["read"],
        }.items():
            with self.subTest(permission=permission):
                self.assertEqual(permissions[permission], expected)

    def test_explicit_opt_in_requests_workflow_write_access(self):
        result = self.create_token("1", through_make=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.token_parameters().get("workflows"), ["write"])
        self.assertIn("Pull requests, Workflows", result.stdout)

    def test_make_defaults_to_no_workflow_access(self):
        result = self.create_token(through_make=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("workflows", self.token_parameters())

    def test_zero_does_not_opt_in(self):
        result = self.create_token("0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("workflows", self.token_parameters())

    def test_invalid_opt_in_opens_no_token_page(self):
        result = self.create_token("yes")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ALLOW_WORKFLOWS must be 0 or 1", result.stderr)
        self.assertFalse(self.url_file.exists())


if __name__ == "__main__":
    unittest.main()

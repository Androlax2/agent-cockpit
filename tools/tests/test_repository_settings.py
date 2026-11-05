import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/repository_settings.py"
REPOSITORY = "repos/Androlax2/agent-cockpit"
PERMISSIONS = f"{REPOSITORY}/actions/permissions"
REFS = f"{REPOSITORY}/git/refs"
RULESET = f"{REPOSITORY}/rulesets/26382178"
PROBE_COMMIT = "3abf9222c4e3b4bc819959c9d2f380875f4c2c50"
PROBE_WORKFLOWS = f"{REPOSITORY}/contents/.github/workflows?ref={PROBE_COMMIT}"
CREATION_RULESET = {
    "enforcement": "active",
    "target": "tag",
    "conditions": {"ref_name": {"include": ["refs/tags/v*"], "exclude": []}},
    "rules": [{"type": "creation"}],
    "bypass_actors": [{"actor_type": "Integration", "actor_id": 5573576, "bypass_mode": "always"}],
}
RULE_VIOLATION = {
    "message": "Repository rule violations found\n\nCannot create ref due to creations being restricted.\n\n",
    "status": "422",
}

# A stateful GitHub stand-in: GET answers from state, PUT stores the request body, and every
# other request answers from its configured response. Each call is logged with its body.
FAKE_GH = """\
import json, os, sys
from pathlib import Path
arguments = sys.argv[1:]
endpoint = arguments[1]
method = arguments[arguments.index("--method") + 1]
body = json.loads(sys.stdin.read() or "null") if "--input" in arguments else None
with open(os.environ["FAKE_LOG"], "a") as log:
    log.write(json.dumps({"arguments": arguments, "body": body}) + "\\n")
state_path = Path(os.environ["FAKE_STATE"])
state = json.loads(state_path.read_text())
if method == "PUT":
    state[endpoint] = {"status": 204, "body": None, "stored": body}
    state[endpoint + ":GET"] = {"status": 200, "body": {**state[endpoint + ":GET"]["body"], **body}}
    state_path.write_text(json.dumps(state))
    response = {"status": 204, "body": None}
else:
    response = state.get(endpoint + ":" + method, {"status": 404, "body": {"message": "Not Found"}})
print(f"HTTP/2.0 {response['status']} Status")
print()
if "raw" in response:
    print(response["raw"])
elif response["body"] is not None:
    print(json.dumps(response["body"]))
sys.exit(1 if response["status"] >= 400 else 0)
"""


class RepositorySettingsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="repository settings ")
        self.addCleanup(self.temporary.cleanup)
        home = Path(self.temporary.name)
        self.bin = home / "bin"
        self.bin.mkdir()
        self.log = home / "calls.jsonl"
        self.state = home / "state.json"
        self.state_values = {
            f"{PERMISSIONS}:GET": {
                "status": 200,
                "body": {"enabled": True, "allowed_actions": "all", "sha_pinning_required": False},
            },
            f"{RULESET}:GET": {"status": 200, "body": CREATION_RULESET},
            f"{REFS}:POST": {"status": 422, "body": RULE_VIOLATION},
        }
        fake = self.bin / "gh"
        fake.write_text(f"#!{sys.executable}\n{FAKE_GH}")
        fake.chmod(0o755)
        self.environment = {
            "PATH": str(self.bin),
            "HOME": str(home),
            "FAKE_LOG": str(self.log),
            "FAKE_STATE": str(self.state),
        }

    def run_script(self, *arguments):
        self.state.write_text(json.dumps(self.state_values))
        return subprocess.run(
            [sys.executable, str(SCRIPT), *arguments],
            env=self.environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def calls(self, method):
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        return [
            call
            for call in calls
            if call["arguments"][call["arguments"].index("--method") + 1] == method
        ]

    def test_sha_pinning_is_turned_on_keeping_the_allowed_actions_policy(self):
        self.state_values[f"{PERMISSIONS}:GET"]["body"]["allowed_actions"] = "selected"
        result = self.run_script("require-action-sha-pinning")
        self.assertEqual(result.returncode, 0, result.stderr)
        [put] = self.calls("PUT")
        self.assertEqual(put["arguments"][1], PERMISSIONS)
        self.assertEqual(
            put["body"],
            {"enabled": True, "allowed_actions": "selected", "sha_pinning_required": True},
        )
        self.assertIn("now required", result.stdout)

    def test_sha_pinning_already_required_changes_nothing(self):
        self.state_values[f"{PERMISSIONS}:GET"]["body"]["sha_pinning_required"] = True
        result = self.run_script("require-action-sha-pinning")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls("PUT"), [])
        self.assertIn("already required", result.stdout)

    def test_sha_pinning_refuses_to_enable_disabled_actions(self):
        self.state_values[f"{PERMISSIONS}:GET"]["body"]["enabled"] = False
        result = self.run_script("require-action-sha-pinning")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.calls("PUT"), [])
        self.assertIn("Actions is disabled", result.stderr)

    def test_sha_pinning_fails_when_the_readback_does_not_show_it(self):
        # GitHub answers 204 but keeps the old value: the PUT is answered, never stored.
        self.state_values[f"{PERMISSIONS}:PUT"] = {"status": 204, "body": None}
        fake = self.bin / "gh"
        fake.write_text(fake.read_text().replace('if method == "PUT":', "if False:"))
        result = self.run_script("require-action-sha-pinning")
        self.assertEqual(result.returncode, 1)
        self.assertIn("readback", result.stderr)

    def test_tag_probe_passes_when_the_ruleset_refuses_the_tag(self):
        result = self.run_script("probe-tag-refusal")
        self.assertEqual(result.returncode, 0, result.stderr)
        [post] = self.calls("POST")
        self.assertEqual(post["arguments"][1], REFS)
        self.assertEqual(
            post["body"], {"ref": "refs/tags/v0.0.0-ruleset-probe", "sha": PROBE_COMMIT}
        )
        self.assertIn("refused", result.stdout)

    def test_tag_probe_fails_loudly_when_the_tag_is_created(self):
        self.state_values[f"{REFS}:POST"] = {"status": 201, "body": {"ref": "refs/tags/x"}}
        result = self.run_script("probe-tag-refusal")
        self.assertEqual(result.returncode, 1)
        self.assertIn("NOT protected", result.stderr)

    def test_tag_probe_fails_when_the_tag_already_exists(self):
        self.state_values[f"{REFS}:POST"] = {
            "status": 422,
            "body": {"message": "Reference already exists", "status": "422"},
        }
        result = self.run_script("probe-tag-refusal")
        self.assertEqual(result.returncode, 1)
        self.assertIn("already exists", result.stderr)

    def test_tag_probe_reports_an_unrelated_error_as_inconclusive(self):
        self.state_values[f"{REFS}:POST"] = {"status": 403, "body": {"message": "Forbidden"}}
        result = self.run_script("probe-tag-refusal")
        self.assertEqual(result.returncode, 2)
        self.assertIn("inconclusive", result.stderr)

    def test_tag_probe_fails_when_the_refusal_is_not_the_creation_ruleset(self):
        self.state_values[f"{RULESET}:GET"]["body"] = {
            **CREATION_RULESET,
            "enforcement": "evaluate",
        }
        result = self.run_script("probe-tag-refusal")
        self.assertEqual(result.returncode, 1)
        self.assertIn("enforcement", result.stderr)

    def test_tag_probe_never_tags_a_commit_that_holds_workflows(self):
        self.state_values[f"{PROBE_WORKFLOWS}:GET"] = {"status": 200, "body": [{"name": "ci.yml"}]}
        result = self.run_script("probe-tag-refusal")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.calls("POST"), [])
        self.assertIn("not workflow-free", result.stderr)

    def test_a_non_json_error_page_is_inconclusive(self):
        self.state_values[f"{REFS}:POST"] = {
            "status": 502,
            "body": None,
            "raw": "<html>Bad gateway",
        }
        result = self.run_script("probe-tag-refusal")
        self.assertEqual(result.returncode, 2)
        self.assertIn("without a JSON body", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_missing_gh_is_inconclusive_without_a_traceback(self):
        (self.bin / "gh").unlink()
        result = self.run_script("require-action-sha-pinning")
        self.assertEqual(result.returncode, 2)
        self.assertIn("gh is not installed", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_sha_pinning_is_inconclusive_without_an_allowed_actions_policy(self):
        del self.state_values[f"{PERMISSIONS}:GET"]["body"]["allowed_actions"]
        result = self.run_script("require-action-sha-pinning")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.calls("PUT"), [])

    def test_every_request_targets_the_fixed_repository_on_github_com(self):
        self.run_script("require-action-sha-pinning")
        self.run_script("probe-tag-refusal")
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertTrue(calls)
        for call in calls:
            arguments = call["arguments"]
            with self.subTest(endpoint=arguments[1]):
                self.assertEqual(arguments[0], "api")
                self.assertTrue(arguments[1].startswith(REPOSITORY + "/"))
                self.assertEqual(arguments[arguments.index("--hostname") + 1], "github.com")


if __name__ == "__main__":
    unittest.main()

import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/repository_security.py"
REPOSITORY = "repos/Androlax2/agent-cockpit"


class RepositorySecurityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="repository security ")
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name)
        self.bin = self.home / "bin"
        self.bin.mkdir()
        self.log = self.home / "calls.jsonl"
        self.fixture = self.home / "responses.json"
        self.responses = {
            REPOSITORY: [{"full_name": "Androlax2/agent-cockpit"}],
            f"{REPOSITORY}/rulesets?per_page=100": [[{"id": 7}], [{"id": 8}]],
            f"{REPOSITORY}/rulesets/7": [{"id": 7, "bypass_actors": []}],
            f"{REPOSITORY}/rulesets/8": [{"id": 8, "target": "tag"}],
            f"{REPOSITORY}/actions/secrets?per_page=100": [{"total_count": 0, "secrets": []}],
            f"{REPOSITORY}/environments/release/secrets?per_page=100": [
                {"total_count": 0, "secrets": []}
            ],
        }
        fake = self.bin / "gh"
        fake.write_text(
            f"#!{sys.executable}\n"
            "import json, os, sys\n"
            "from pathlib import Path\n"
            "from urllib.parse import urlsplit, parse_qs\n"
            "arguments = sys.argv[1:]\n"
            "with open(os.environ['FAKE_LOG'], 'a') as log:\n"
            "    log.write(json.dumps(arguments) + '\\n')\n"
            "endpoint = arguments[1]\n"
            "page_number = int(parse_qs(urlsplit(endpoint).query).get('page', ['1'])[0])\n"
            "base_endpoint = endpoint.split('&page=')[0].split('?page=')[0]\n"
            "responses = json.loads(Path(os.environ['FAKE_RESPONSES']).read_text())\n"
            "response = responses.get(endpoint, responses.get(base_endpoint, [{}]))\n"
            "if isinstance(response, dict):\n"
            "    print(response.get('stdout', ''), end='')\n"
            "    print(response.get('stderr', ''), file=sys.stderr, end='')\n"
            "    sys.exit(response.get('exit', 0))\n"
            "print('HTTP/2.0 200 OK')\n"
            "if page_number < len(response):\n"
            "    print('Link: <https://api.github.com/' + base_endpoint + '&page=' +\n"
            "          str(page_number + 1) + '>; rel=\"next\"')\n"
            "print()\n"
            "print(json.dumps(response[page_number - 1]))\n"
        )
        fake.chmod(0o755)

    def collect(self, *arguments):
        self.fixture.write_text(json.dumps(self.responses))
        environment = {
            "PATH": str(self.bin),
            "HOME": str(self.home),
            "FAKE_LOG": str(self.log),
            "FAKE_RESPONSES": str(self.fixture),
        }
        result = subprocess.run(
            [sys.executable, str(SCRIPT), *arguments],
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        return result

    def test_every_request_is_a_paginated_get_on_the_fixed_repository(self):
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertGreater(len(calls), 10)
        for call in calls:
            with self.subTest(endpoint=call[1]):
                self.assertEqual(call[0], "api")
                self.assertTrue(call[1].startswith(REPOSITORY))
                self.assertEqual(call[call.index("--method") + 1], "GET")
                self.assertEqual(call[call.index("--hostname") + 1], "github.com")
                self.assertIn("--include", call)
                self.assertNotIn("--paginate", call)
                self.assertNotIn("--input", call)
                self.assertNotIn("-f", call)
        variable_calls = [call for call in calls if "/variables" in call[1]]
        self.assertEqual(
            [call[1] for call in variable_calls],
            [f"{REPOSITORY}/actions/variables/RELEASE_APP_CLIENT_ID"],
        )

    def test_rulesets_on_later_pages_are_inspected(self):
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        observations = json.loads(result.stdout)["observations"]
        self.assertEqual(observations["ruleset:8"]["pages"], [{"id": 8, "target": "tag"}])

    def test_unrelated_metadata_fields_are_not_printed(self):
        self.responses[REPOSITORY] = [
            {
                "full_name": "Androlax2/agent-cockpit",
                "unrelated": "DO_NOT_PRINT",
            }
        ]
        self.responses[f"{REPOSITORY}/actions/secrets?per_page=100"] = [
            {
                "total_count": 1,
                "secrets": [{"name": "RELEASE_APP_PRIVATE_KEY", "unexpected": "DO_NOT_PRINT"}],
            }
        ]
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("DO_NOT_PRINT", result.stdout)
        self.assertIn("RELEASE_APP_PRIVATE_KEY", result.stdout)

    def test_later_page_failure_retains_successful_rulesets_and_inspects_them(self):
        self.responses[f"{REPOSITORY}/rulesets?per_page=100&page=2"] = {
            "stdout": '{"message":"DO_NOT_PRINT","status":"403"}',
            "stderr": "gh: DO_NOT_PRINT (HTTP 403)",
            "exit": 1,
        }
        result = self.collect()
        self.assertEqual(result.returncode, 1)
        report = json.loads(result.stdout)
        rulesets = report["observations"]["rulesets"]
        self.assertEqual(rulesets.get("pages"), [[{"id": 7}]])
        self.assertEqual(rulesets["state"], "unknown")
        self.assertEqual(report["observations"]["ruleset:7"]["state"], "observed")
        self.assertNotIn("DO_NOT_PRINT", result.stdout)

    def test_later_page_timeout_retains_prior_pages_at_the_real_main_entry(self):
        spec = importlib.util.spec_from_file_location("repository_security", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        def response(arguments, **kwargs):
            endpoint = arguments[2]
            if endpoint.endswith("rulesets?per_page=100"):
                output = 'HTTP/2.0 200 OK\nLink: <unused>; rel="next"\n\n[{"id":7}]'
            elif endpoint.endswith("rulesets?per_page=100&page=2"):
                raise subprocess.TimeoutExpired(arguments, 60, output=b"DO_NOT_PRINT")
            elif "/secrets?" in endpoint:
                output = 'HTTP/2.0 200 OK\n\n{"total_count":0,"secrets":[]}'
            else:
                output = "HTTP/2.0 200 OK\n\n{}"
            return subprocess.CompletedProcess(arguments, 0, stdout=output, stderr="")

        output = io.StringIO()
        with (
            patch.object(module.subprocess, "run", side_effect=response),
            patch.object(sys, "argv", [str(SCRIPT)]),
            redirect_stdout(output),
        ):
            self.assertEqual(module.main(), 1)
        report = json.loads(output.getvalue())
        rulesets = report["observations"]["rulesets"]
        self.assertEqual(rulesets["pages"], [[{"id": 7}]])
        self.assertIn("timed out", rulesets["reason"])
        self.assertEqual(report["observations"]["ruleset:7"]["state"], "observed")
        self.assertNotIn("DO_NOT_PRINT", output.getvalue())

    def test_object_shaped_environment_pages_are_all_retained(self):
        self.responses[f"{REPOSITORY}/environments?per_page=100"] = [
            {"total_count": 2, "environments": [{"name": "github-pages"}]},
            {"total_count": 2, "environments": [{"name": "release"}]},
        ]
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout)["observations"]["environments"]["pages"][1],
            {"total_count": 2, "environments": [{"name": "release"}]},
        )

    def test_access_errors_remain_unknown_and_preserve_other_observations(self):
        for status in (401, 403, 404, 429, 500):
            with self.subTest(status=status):
                self.responses[f"{REPOSITORY}/actions/permissions"] = {
                    "exit": 1,
                    "stdout": '{"private_error":"DO_NOT_PRINT"}',
                    "stderr": f"gh: DO_NOT_PRINT (HTTP {status})",
                }
                result = self.collect()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertNotIn("DO_NOT_PRINT", result.stdout + result.stderr)
                report = json.loads(result.stdout)
                self.assertFalse(report["collection_complete"])
                self.assertEqual(
                    report["observations"]["actions"],
                    {
                        "endpoint": f"{REPOSITORY}/actions/permissions",
                        "state": "unknown",
                        "pages": [],
                        "reason": f"HTTP {status}",
                    },
                )
                self.assertEqual(report["observations"]["repository"]["state"], "observed")

    def test_successful_empty_response_is_not_called_disabled(self):
        self.responses[f"{REPOSITORY}/vulnerability-alerts"] = {
            "stdout": "HTTP/2.0 204 No Content\n\n"
        }
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        observation = json.loads(result.stdout)["observations"]["dependabot_alerts"]
        self.assertEqual(observation["state"], "observed")
        self.assertEqual(observation["pages"], [])

    def test_invalid_json_or_unexpected_page_shape_fails_collection(self):
        for output in ("not-json", '"bad-page"', "null"):
            with self.subTest(output=output):
                self.responses[REPOSITORY] = {"stdout": "HTTP/2.0 200 OK\n\n" + output}
                result = self.collect()
                self.assertEqual(result.returncode, 1)
                self.assertEqual(
                    json.loads(result.stdout)["observations"]["repository"]["state"], "unknown"
                )

    def test_untrusted_ruleset_id_cannot_become_an_api_path(self):
        self.responses[f"{REPOSITORY}/rulesets?per_page=100"] = [[{"id": "../../elsewhere"}]]
        result = self.collect()
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("elsewhere", self.log.read_text())

    def test_missing_gh_is_reported_without_a_traceback(self):
        (self.bin / "gh").unlink()
        result = self.collect()
        self.assertEqual(result.returncode, 1)
        self.assertIn("gh unavailable", result.stdout)
        self.assertNotIn("Traceback", result.stderr)

    def test_arguments_cannot_select_another_repository_or_a_write_mode(self):
        result = self.collect("--apply")
        self.assertEqual(result.returncode, 2)
        self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main()

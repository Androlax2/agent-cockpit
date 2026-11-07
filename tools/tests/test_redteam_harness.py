import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import importlib.util

SPEC = importlib.util.spec_from_file_location(
    "redteam_harness", Path(__file__).resolve().parents[1] / "redteam-harness.py"
)
redteam_harness = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(redteam_harness)


class ClassifyOutcomeTests(unittest.TestCase):
    def test_a_torlek_refusal_message_is_refused(self):
        self.assertEqual(
            redteam_harness.classify_outcome(1, "torlek: refused: is not a plain relative folder"),
            "refused",
        )

    def test_a_not_authorized_error_code_is_refused(self):
        self.assertEqual(redteam_harness.classify_outcome(1, "code: NotAuthorized"), "refused")

    def test_a_blocked_network_host_is_blocked(self):
        self.assertEqual(
            redteam_harness.classify_outcome(
                1, "< HTTP/1.1 403\nCONNECT tunnel failed, response 403"
            ),
            "blocked",
        )

    def test_a_permission_denied_read_is_blocked(self):
        self.assertEqual(
            redteam_harness.classify_outcome(1, "cat: /root/secret: Permission denied"), "blocked"
        )

    def test_a_clean_exit_with_no_refusal_text_succeeded(self):
        self.assertEqual(redteam_harness.classify_outcome(0, "redteam\n"), "succeeded")

    def test_a_failure_matching_neither_pattern_is_unclear(self):
        self.assertEqual(
            redteam_harness.classify_outcome(127, "bash: nosuchcommand: command not found"),
            "unclear",
        )


class CatalogTests(unittest.TestCase):
    def test_the_shipped_catalog_has_one_automated_entry_per_host_only_verb_group(self):
        catalog = redteam_harness.load_catalog(redteam_harness.DEFAULT_CATALOG)
        ids = [entry["id"] for entry in catalog]
        self.assertEqual(len(ids), len(set(ids)), "duplicate catalog ids")
        for entry in catalog:
            self.assertIn(entry["status"], ("automated", "manual"))
            self.assertIn(entry["category"], (1, 2, 3, 4))
            if entry["status"] == "automated":
                self.assertIn("command", entry)


class RunCatalogTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="redteam-harness-test-"))
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", str(self.root)]))
        self.results_log = self.root / "results.jsonl"
        self.calls = []

    def fake_runner(self, outputs):
        def run_guest_command(command):
            self.calls.append(command)
            if command.startswith("~/.local/bin/torlek version"):
                return subprocess.CompletedProcess(
                    args=command, returncode=0, stdout="torlek 0.1.0-test\n", stderr=""
                )
            return outputs.get(
                command,
                subprocess.CompletedProcess(args=command, returncode=0, stdout="", stderr=""),
            )

        return run_guest_command

    def test_runs_only_automated_entries_and_logs_each_one(self):
        catalog = [
            {"id": "a", "category": 1, "title": "t", "status": "automated", "command": "echo a"},
            {"id": "b", "category": 1, "title": "t", "status": "manual"},
        ]
        runner = self.fake_runner(
            {
                "echo a": subprocess.CompletedProcess(
                    args="echo a", returncode=0, stdout="a\n", stderr=""
                )
            }
        )

        records = redteam_harness.run_catalog(catalog, self.results_log, runner)

        self.assertEqual([record["id"] for record in records], ["a"])
        self.assertIn("echo a", self.calls)
        logged = [json.loads(line) for line in self.results_log.read_text().splitlines()]
        self.assertEqual(len(logged), 1)
        self.assertEqual(logged[0]["outcome"], "succeeded")
        self.assertEqual(logged[0]["torlek_version"], "torlek 0.1.0-test")

    def test_an_id_filter_runs_only_that_check(self):
        catalog = [
            {"id": "a", "category": 1, "title": "t", "status": "automated", "command": "echo a"},
            {"id": "b", "category": 1, "title": "t", "status": "automated", "command": "echo b"},
        ]
        runner = self.fake_runner({})

        records = redteam_harness.run_catalog(catalog, self.results_log, runner, only_ids={"b"})

        self.assertEqual([record["id"] for record in records], ["b"])

    def test_a_refused_command_is_logged_as_refused_not_succeeded(self):
        catalog = [
            {
                "id": "a",
                "category": 2,
                "title": "t",
                "status": "automated",
                "command": "torlek session grant x y",
            }
        ]
        runner = self.fake_runner(
            {
                "torlek session grant x y": subprocess.CompletedProcess(
                    args="torlek session grant x y",
                    returncode=1,
                    stdout="",
                    stderr="torlek: refused: this command is the owner's alone",
                )
            }
        )

        records = redteam_harness.run_catalog(catalog, self.results_log, runner)

        self.assertEqual(records[0]["outcome"], "refused")

    def test_results_log_is_appended_to_not_overwritten(self):
        self.results_log.parent.mkdir(parents=True, exist_ok=True)
        self.results_log.write_text(json.dumps({"id": "earlier"}) + "\n")
        catalog = [
            {"id": "a", "category": 1, "title": "t", "status": "automated", "command": "echo a"}
        ]
        runner = self.fake_runner({})

        redteam_harness.run_catalog(catalog, self.results_log, runner)

        logged = [json.loads(line) for line in self.results_log.read_text().splitlines()]
        self.assertEqual([entry.get("id") for entry in logged], ["earlier", "a"])


if __name__ == "__main__":
    unittest.main()

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "acceptance/alpha1.py"

# Only the selected candidate is faked: the runner executes subprocesses and writes real evidence.
FAKE = """#!{python}
import json
import os
import sys
from pathlib import Path

with (Path(os.environ["HOME"]).parent / "calls.jsonl").open("a") as log:
    log.write(json.dumps({{"argv": sys.argv[1:], "env": dict(os.environ), "cwd": os.getcwd()}}) + "\\n")
verb = sys.argv[3:]
if verb[0] == "doctor":
    print("available             config: absent")
    print("missing-prerequisite  daemon: not running")
    print("                        next: start `torlek-daemon`")
    print("unknown               host: assessed by the daemon")
    sys.exit(1)
print("fixture " + " ".join(verb))
"""


class AlphaAcceptanceTests(unittest.TestCase):
    def setUp(self):
        # macOS's default TMPDIR is too long for the runner's Unix socket paths.
        parent = "/tmp/opencode" if Path("/tmp/opencode").is_dir() else "/tmp"
        self.temporary = tempfile.TemporaryDirectory(dir=parent)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / "candidate with spaces"
        self.bin.mkdir()
        self.program = self.bin / "torlek"
        self.program.write_text(FAKE.format(python=sys.executable))
        self.program.chmod(0o755)
        self.parent = self.root / "runs"
        self.parent.mkdir()

    def run_rehearsal(self):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--bin-dir", str(self.bin), "--parent", str(self.parent)],
            env={
                **os.environ,
                "HOME": str(self.root / "real owner"),
                "TORLEK_PROFILE": "torlek",
                "TORLEK_AGENT_SOCKET": "/real/agent.sock",
                "TORLEK_LAUNCH_TOKEN": "fixture-secret",
                "ANTHROPIC_API_KEY": "fixture-secret",
                "OPENAI_API_KEY": "fixture-secret",
                "GH_TOKEN": "fixture-secret",
                "DBUS_SESSION_BUS_ADDRESS": "unix:path=/real/bus",
                "DISPLAY": ":123",
                "TMUX": "/real/tmux,1,0",
                "HTTP_PROXY": "http://fixture-secret@proxy.invalid",
            },
            capture_output=True,
            text=True,
            timeout=15,
        )

    def evidence(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(Path(result.stdout.strip()).read_text())

    def test_candidate_gets_only_private_paths_and_no_owner_environment(self):
        record = self.evidence(self.run_rehearsal())
        scratch = Path(record["scratch"])
        calls = [json.loads(line) for line in (scratch / "calls.jsonl").read_text().splitlines()]
        self.assertEqual(len(calls), 4)
        for call in calls:
            with self.subTest(argv=call["argv"]):
                self.assertEqual(call["argv"][:2], ["--profile", "next"])
                self.assertEqual(call["cwd"], str(scratch / "project"))
                for name, child in (
                    ("HOME", "home"),
                    ("XDG_CONFIG_HOME", "config"),
                    ("XDG_STATE_HOME", "state"),
                    ("XDG_CACHE_HOME", "cache"),
                    ("XDG_RUNTIME_DIR", "run"),
                    ("XDG_DATA_HOME", "data"),
                    ("TMUX_TMPDIR", "tmux"),
                    ("TMPDIR", "tmp"),
                ):
                    self.assertEqual(call["env"][name], str(scratch / child))
                self.assertEqual(call["env"]["TORLEK_PROFILE"], "next")
                self.assertNotIn("fixture-secret", json.dumps(call))
                for name in ("DBUS_SESSION_BUS_ADDRESS", "DISPLAY", "TMUX", "TORLEK_AGENT_SOCKET"):
                    self.assertNotIn(name, call["env"])
        self.assertEqual(scratch.stat().st_mode & 0o777, 0o700)

    def test_records_candidate_identity_and_expected_offline_refusal_not_acceptance(self):
        record = self.evidence(self.run_rehearsal())
        self.assertEqual(record["candidate"], str(self.program.resolve()))
        self.assertEqual(record["sha256"], hashlib.sha256(self.program.read_bytes()).hexdigest())
        self.assertEqual(record["scope"], "offline-rehearsal")
        self.assertFalse(record["live_acceptance"])
        self.assertEqual([result["exit_code"] for result in record["commands"]], [0, 0, 0, 1])
        self.assertIn("missing-prerequisite", Path(record["commands"][-1]["stdout"]).read_text())

    def test_each_run_preserves_previous_evidence_in_a_different_home(self):
        first = self.evidence(self.run_rehearsal())
        second = self.evidence(self.run_rehearsal())
        self.assertNotEqual(first["scratch"], second["scratch"])
        self.assertTrue(Path(first["scratch"]).is_dir())

    def test_missing_candidate_refuses_before_creating_scratch(self):
        self.program.unlink()
        result = self.run_rehearsal()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("executable", result.stderr)
        self.assertEqual(list(self.parent.iterdir()), [])

    def test_unexpected_candidate_exit_is_preserved_and_fails_the_rehearsal(self):
        self.program.write_text(f"#!{sys.executable}\nimport sys\nsys.exit(7)\n")
        result = self.run_rehearsal()
        self.assertEqual(result.returncode, 1, result.stderr)
        record = json.loads(Path(result.stdout.strip()).read_text())
        self.assertEqual(record["commands"][0]["exit_code"], 7)
        self.assertFalse(record["offline_checks_passed"])

    def test_doctor_exit_one_without_missing_daemon_is_not_a_pass(self):
        self.program.write_text(
            FAKE.format(python=sys.executable).replace(
                'print("missing-prerequisite  daemon: not running")',
                'print("configuration broken")',
            )
        )
        result = self.run_rehearsal()
        self.assertEqual(result.returncode, 1, result.stderr)
        record = json.loads(Path(result.stdout.strip()).read_text())
        self.assertFalse(record["offline_checks_passed"])

    def test_fixture_works_with_a_long_system_temporary_path(self):
        system_temp = self.root / ("system-temporary-" * 6)
        system_temp.mkdir()
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "unittest",
                "test_alpha_acceptance.AlphaAcceptanceTests."
                "test_records_candidate_identity_and_expected_offline_refusal_not_acceptance",
            ],
            cwd=SCRIPT.parents[1] / "tests",
            env={**os.environ, "TMPDIR": str(system_temp)},
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()

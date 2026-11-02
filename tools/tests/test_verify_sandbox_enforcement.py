"""Bounded enforcement evidence and isolated-runner refusal, without live services/network."""

import copy
import importlib.util
import json
import os
import select
import socket
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock

TOOLS = Path(__file__).resolve().parents[1]
RUNNER = TOOLS / "verify-sandbox-enforcement.py"


def load(path):
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def measured(nonce="fixture-1", session="s1"):
    return {
        "nonce": nonce,
        "session": session,
        "allowed_path": True,
        "denied_paths": {
            name: "EACCES" for name in ("ssh-key", "daemon-db", "proxy-key", "proxy-token")
        },
        "allowed_network": {"exit": 0, "http": 200, "connect": 200},
        "denied_network": {"exit": 56, "http": 0, "connect": 403},
    }


class LiveGuardTests(unittest.TestCase):
    def test_live_entry_point_refuses_before_touching_a_non_ci_home(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            result = subprocess.run(
                [sys.executable, str(RUNNER), "--ci"],
                env={"HOME": str(home), "PATH": "/usr/bin:/bin"},
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("requires a disposable GitHub-hosted CI runner", result.stderr)
            self.assertEqual(list(home.iterdir()), [])

    @unittest.skipUnless(sys.platform == "linux", "systemctl routing is Linux-only")
    def test_private_runtime_routes_the_manager_to_the_fixture_session_bus(self):
        module = load(RUNNER)
        with tempfile.TemporaryDirectory(prefix="433-bus-", dir="/tmp") as directory:
            root = Path(directory)
            runtime = root / "runtime"
            runtime.mkdir()
            bus_path = root / "session-bus"
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as bus:
                bus.bind(str(bus_path))
                bus.listen(1)
                env = module.environment(
                    root,
                    runtime,
                    root / "bin",
                    {"DBUS_SESSION_BUS_ADDRESS": f"unix:path={bus_path}"},
                )
                # This socket deliberately does not implement D-Bus. Only endpoint
                # selection is measured, never a live user's manager or service.
                child = subprocess.Popen(
                    ["/usr/bin/systemctl", "--user", "show", "--property=Version"],
                    env=env,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                try:
                    ready, _, _ = select.select([bus], [], [], 1)
                    if ready:
                        connection, _ = bus.accept()
                        connection.close()
                finally:
                    try:
                        child.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=3)
                self.assertTrue(ready, "systemctl ignored the explicit fixture session bus")
                self.assertNotEqual(child.returncode, 0, "the fixture must not act as a manager")


def proxy_logs(client="cockpit-s1-000000000001"):
    return {
        "items": [
            {
                "id": 1,
                "container_name": client,
                "destination_host": "api.github.com",
                "destination_port": 443,
                "resolved_hostname": None,
                "result": "allowed",
                "rule_id": 7,
            },
            {
                "id": 2,
                "container_name": client,
                "destination_host": "example.com",
                "destination_port": 443,
                "resolved_hostname": None,
                "result": "blocked",
                "rule_id": None,
            },
        ],
        "total": 2,
    }


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.runner = load(RUNNER)

    def test_explicit_denial_and_both_positive_controls_are_required(self):
        observations = self.runner.verify_probe(
            measured(), "fixture-1", "s1", proxy_logs(), "cockpit-s1-000000000001"
        )
        self.assertIn("allowed destination https://api.github.com/: HTTP 200", observations)
        self.assertIn(
            "denied destination https://example.com/: Greyproxy recorded blocked for this launch",
            observations,
        )
        for field, replacement in [
            ("allowed_path", False),
            ("allowed_network", {"exit": 60, "http": 0, "connect": 200}),
            ("denied_network", {"exit": 0, "http": 200, "connect": 200}),
            ("denied_paths", {"ssh-key": "EACCES"}),
            ("denied_paths", {name: "READABLE" for name in measured()["denied_paths"]}),
            ("nonce", "stale"),
            ("session", "other"),
        ]:
            result = measured()
            result[field] = replacement
            with self.subTest(field=field, replacement=replacement), self.assertRaises(ValueError):
                self.runner.verify_probe(
                    result, "fixture-1", "s1", proxy_logs(), "cockpit-s1-000000000001"
                )

    def test_a_transport_failure_needs_exact_launch_and_destination_proxy_decisions(self):
        result = measured()
        result["denied_network"] = {"exit": 28, "http": 0, "connect": 0}
        self.runner.verify_probe(result, "fixture-1", "s1", proxy_logs(), "cockpit-s1-000000000001")
        wrong_host = proxy_logs()
        wrong_host["items"][1]["destination_host"] = "another.example.com"
        wrong_port = proxy_logs()
        wrong_port["items"][1]["destination_port"] = 80
        silent_allow = proxy_logs()
        silent_allow["items"][0]["rule_id"] = None
        missing = {"items": proxy_logs()["items"][:1], "total": 1}
        for logs in [proxy_logs("other-client"), wrong_host, wrong_port, silent_allow, missing]:
            with self.subTest(logs=logs), self.assertRaises(ValueError):
                self.runner.verify_probe(result, "fixture-1", "s1", logs, "cockpit-s1-000000000001")

    def test_saved_allowed_decision_is_the_row_with_the_matching_rule(self):
        logs = proxy_logs()
        no_rule = dict(logs["items"][0], id=3, rule_id=None)
        logs["items"].insert(0, no_rule)
        selected = self.runner.proxy_decisions(logs, "cockpit-s1-000000000001")
        self.assertEqual(selected[0]["id"], 1)
        self.assertEqual(selected[0]["rule_id"], 7)

    def test_malformed_proxy_fields_are_reported_as_invalid_evidence(self):
        for field, value in [
            ("destination_host", []),
            ("resolved_hostname", {}),
            ("id", True),
            ("rule_id", True),
        ]:
            logs = proxy_logs()
            logs["items"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.runner.proxy_decisions(logs, "cockpit-s1-000000000001")

    def test_results_must_be_bounded_regular_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "result.json"
            path.write_text(json.dumps(measured()))
            self.assertEqual(self.runner.read_result(path), measured())
            link = root / "link"
            link.symlink_to(path)
            with self.assertRaises((ValueError, OSError)):
                self.runner.read_result(link)
            path.write_text(" " * 8193)
            with self.assertRaises(ValueError):
                self.runner.read_result(path)

    def test_four_distinct_measured_launches_are_required(self):
        launches = [
            {"client": f"cockpit-s1-{index:012x}", "observed": ["measured"], "session": "s1"}
            for index in range(4)
        ]
        self.runner.verify_four_launches(launches)
        bad = copy.deepcopy(launches)
        bad[-1]["client"] = bad[0]["client"]
        mixed = copy.deepcopy(launches)
        mixed[-1]["session"] = "s2"
        unmeasured = copy.deepcopy(launches)
        unmeasured[-1]["observed"] = []
        for records in [launches[:3], launches + [launches[0]], bad, mixed, unmeasured]:
            with self.subTest(records=records), self.assertRaises(ValueError):
                self.runner.verify_four_launches(records)

    def test_clean_environment_masks_providers_and_excludes_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = self.runner.environment(
                root,
                root / "run",
                root / "bin",
                {
                    "ANTHROPIC_API_KEY": "must-not-travel",
                    "GH_TOKEN": "must-not-travel",
                    "HOME": "/live-home",
                    "COCKPIT_SESSION": "live-session",
                    "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus",
                    "HTTPS_PROXY": "http://ci-route:1234",
                    "RUNNER_TEMP": str(root),
                },
            )
            self.assertEqual(env["HOME"], str(root))
            self.assertTrue(env["PATH"].startswith(str(root / "bin") + ":"))
            self.assertEqual(env["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/run/user/1000/bus")
            self.assertEqual(env["HTTPS_PROXY"], "http://ci-route:1234")
            self.assertFalse({"ANTHROPIC_API_KEY", "GH_TOKEN", "COCKPIT_SESSION"} & env.keys())


class AgentProbeTests(unittest.TestCase):
    def test_probe_uses_fixture_paths_and_a_fake_curl_with_bounded_arguments(self):
        probe = load(TOOLS / "fixtures/sandbox-probe.py")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            curl = bin_dir / "curl"
            curl.write_text(
                "#!/bin/sh\ncase \"$*\" in *api.github.com*) echo '200 200';; *example.com*) echo '000 403'; exit 56;; *) exit 99;; esac\n"
            )
            curl.chmod(0o755)
            (root / "allowed.txt").write_text("cockpit enforcement fixture\n")
            request = {
                "nonce": "fixture-1",
                "denied_paths": {name: str(root / name) for name in measured()["denied_paths"]},
            }
            result = probe.run_probe(request, root, "s1", {"PATH": str(bin_dir) + ":/usr/bin:/bin"})
            self.assertTrue(result["allowed_path"])
            self.assertEqual(set(result["denied_paths"].values()), {"ENOENT"})
            self.assertEqual(result["denied_network"], {"exit": 56, "http": 0, "connect": 403})
            load(RUNNER).verify_probe(
                result, "fixture-1", "s1", proxy_logs(), "cockpit-s1-000000000001"
            )


FAKE_CLI = r"""
import json, os, sys
from pathlib import Path
home = Path(os.environ["HOME"])
assert sys.argv[1:3] == ["--profile", "enforcement"]
args = sys.argv[3:]
with (home / "commands.jsonl").open("a") as out:
    out.write(json.dumps(args) + "\n")
state = home / "state/cockpit-enforcement"
if args[:2] == ["sandbox", "install"]:
    (state / "greyproxy").mkdir(parents=True)
    for name in ["owned.json", "ca-key.pem", "api-token"]:
        (state / "greyproxy" / name).write_text("fixture")
    (state / "state.db").write_text("fixture database")
    if (home / "fail-install").exists():
        sys.exit(1)
elif args[:2] == ["session", "new"] or args[:2] == ["session", "restart"]:
    count_file = home / "count"
    count = int(count_file.read_text()) + 1 if count_file.exists() else 1
    count_file.write_text(str(count))
    request = json.loads((home / "checkout/.enforcement/request.json").read_text())
    result = json.loads((home / "probe-template.json").read_text())
    result["nonce"] = request["nonce"]
    (home / "checkout/.enforcement" / (request["nonce"] + ".json")).write_text(json.dumps(result))
    (home / "observations.json").write_text("[]")
    print("s1\trunning\tclaude")
elif args[:2] == ["session", "sandbox-record"]:
    (home / "observations.json").write_text(json.dumps(args[6::2]))
elif args[:2] == ["session", "sandbox"]:
    print("sandbox: enforced by greywall on fixture")
    print("proxy client: cockpit-s1-" + f"{int((home / 'count').read_text()):012x}")
    for item in json.loads((home / "observations.json").read_text()):
        print("observed: owner probe: " + item)
"""

FAKE_DAEMON = r"""
import os, socket, time, tomllib
from pathlib import Path
config = Path(os.environ["XDG_CONFIG_HOME"]) / "cockpit-enforcement/config.toml"
assert tomllib.loads(config.read_text())["agents"]["desktop_session"] == "withheld"
runtime = Path(os.environ["XDG_RUNTIME_DIR"])
folder = runtime / "cockpit-enforcement"
folder.mkdir()
(folder / "daemon.sock").touch()
tmux = runtime / ("tmux-" + str(os.getuid())) / "cockpit-enforcement"
tmux.parent.mkdir()
sock = socket.socket(socket.AF_UNIX)
sock.bind(str(tmux))
while True: time.sleep(60)
"""


class RunnerEntryTests(unittest.TestCase):
    def test_output_over_limit_refuses_before_waiting_for_child_exit(self):
        module = load(RUNNER)
        # One bounded 65 KiB write followed by sleep, never a synthetic load loop.
        script = "import os,time; os.write(1,b'x'*65537); time.sleep(3)"
        with self.assertRaisesRegex(RuntimeError, "output exceeded 64 KiB"):
            module.command([sys.executable, "-c", script], {}, timeout=1)

    def test_a_second_daemon_stop_timeout_preserves_remaining_cleanup(self):
        module = load(RUNNER)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner = module.Runner(root, root, root, root, {})
            runner.daemon = Mock()
            runner.daemon.poll.return_value = None
            runner.daemon.wait.side_effect = subprocess.TimeoutExpired("fixture-daemon", 5)
            runner.log = (root / "daemon.log").open("w")
            failures = runner.close()
            self.assertEqual(len(failures), 1)
            self.assertIn("timed out", failures[0])
            self.assertTrue(runner.log.closed)

    def test_four_launch_wiring_and_cleanup_use_only_the_fixture_profile(self):
        self.exercise(False)

    def test_partial_installer_failure_still_stops_its_owned_service(self):
        self.exercise(True)

    def exercise(self, fail_install):
        module = load(RUNNER)

        class FixtureRunner(module.Runner):
            def decisions(self, client):
                return proxy_logs(client)

        with tempfile.TemporaryDirectory(prefix="ce-test-", dir="/tmp") as directory:
            root = Path(directory)
            home, runtime, output, programs = [
                root / name for name in ["home with spaces", "run", "out", "programs"]
            ]
            for path in [home, runtime, output, programs]:
                path.mkdir()
            for name, source in [("cockpit", FAKE_CLI), ("cockpit-daemon", FAKE_DAEMON)]:
                path = programs / name
                path.write_text(f"#!{sys.executable}\n{source}")
                path.chmod(0o755)
            (home / "probe-template.json").write_text(json.dumps(measured()))
            if fail_install:
                (home / "fail-install").touch()
            runner = FixtureRunner(programs, home, runtime, output, {})
            for name, script in [
                ("curl", "#!/bin/sh\nprintf 200\n"),
                (
                    "tmux",
                    f"#!{sys.executable}\nimport os,sys\nfrom pathlib import Path\np=Path(sys.argv[2])\nassert p == Path(os.environ['XDG_RUNTIME_DIR']) / ('tmux-' + str(os.getuid())) / 'cockpit-enforcement'\np.unlink()\n",
                ),
            ]:
                path = runner.bin / name
                path.write_text(script)
                path.chmod(0o755)
            try:
                if fail_install:
                    with self.assertRaises(RuntimeError):
                        runner.prepare()
                else:
                    runner.prepare()
                    runner.exercise()
                    self.assertEqual(len(runner.launches), 4)
                    self.assertEqual(len(list(output.glob("launch-*.txt"))), 4)
            finally:
                self.assertEqual(runner.close(), [])
            commands = [
                json.loads(line) for line in (home / "commands.jsonl").read_text().splitlines()
            ]
            self.assertEqual(commands[-1], ["sandbox", "stop"])
            if not fail_install:
                self.assertEqual(commands[-2], ["session", "end", "s1"])
                self.assertEqual(
                    sum(command[:2] == ["session", "restart"] for command in commands), 3
                )
                self.assertIsNotNone(runner.daemon.poll())
                self.assertFalse(
                    (runtime / ("tmux-" + str(os.getuid())) / "cockpit-enforcement").exists()
                )


if __name__ == "__main__":
    unittest.main()

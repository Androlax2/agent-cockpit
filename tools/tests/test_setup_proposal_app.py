"""Proposal setup exercises real entry points with isolated GitHub transports."""

import contextlib
import copy
import importlib
import io
import json
import os
import queue
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
setup = importlib.import_module("setup_proposal_app")

APP_KEY = "fixture-app-private-material"
SSH_KEY = "fixture-ssh-private-material"
APP = {
    "id": 123,
    "client_id": "Iv1.fixture",
    "slug": "fixture-proposal",
    "owner": {"login": "Androlax2"},
    "permissions": {"contents": "read", "pull_requests": "write", "metadata": "read"},
    "events": [],
}


class Fixtures:
    def __init__(self):
        self.app = copy.deepcopy(APP)
        self.installations = [
            {
                "id": 456,
                "app_id": 123,
                "account": {"login": "Androlax2"},
                "repository_selection": "selected",
                "permissions": self.app["permissions"],
                "suspended_at": None,
            }
        ]
        self.repositories = {
            "total_count": 1,
            "repositories": [{"full_name": "Androlax2/agent-cockpit"}],
        }
        self.environment = {
            "name": "release-proposal",
            "protection_rules": [{"id": 10, "node_id": "fixture", "type": "branch_policy"}],
            "deployment_branch_policy": {
                "protected_branches": False,
                "custom_branch_policies": True,
            },
        }
        self.refs = [{"id": 1, "name": "main", "type": "branch"}]
        self.variables = {}
        self.secrets = {}
        self.keys = []
        self.calls = []
        self.fail_write = None

    def api(self, method, path, token=None, data=None):
        self.calls.append((method, path, data))
        if path.startswith("/app-manifests/"):
            return {**self.app, "pem": APP_KEY}
        if path == "/app":
            return self.app
        if path.startswith("/app/installations?"):
            return self.installations
        if path.endswith("/access_tokens"):
            assert data == {"permissions": {"metadata": "read"}}
            return {"token": "fixture-temporary-token"}
        if path.startswith("/installation/repositories?"):
            return self.repositories
        if path == "/installation/token":
            return None
        raise AssertionError(path)

    def gh(self, arguments, data=None):
        self.calls.append(("gh", arguments, data))
        if arguments == ["api", "user"]:
            return {"login": "Androlax2"}
        if arguments[0:2] == ["secret", "set"]:
            assert arguments[3:] == [
                "--repo",
                "Androlax2/agent-cockpit",
                "--env",
                "release-proposal",
            ]
            name = arguments[2]
            assert (
                data
                == {"PROPOSAL_APP_PRIVATE_KEY": APP_KEY, "PROPOSAL_GIT_SSH_KEY": SSH_KEY}[
                    name
                ].encode()
            )
            self.secrets[name] = {"name": name, "updated_at": "fixture-timestamp"}
            if self.fail_write == name:
                raise setup.SetupError("Uncertain write; reconcile.")
            return None
        path = next(value for value in arguments if value.startswith(("repos/", "users/")))
        if path.startswith("users/"):
            return {"login": "fixture-proposal[bot]", "id": 987, "type": "Bot"}
        if path.endswith("/environments?per_page=100"):
            return [{"environments": [self.environment]}]
        if path.endswith("/environments/release-proposal"):
            return self.environment
        if "deployment-branch-policies?" in path:
            return [{"branch_policies": self.refs}]
        if "/actions/" in path:
            return [{"secrets": [], "variables": []}]
        if path.endswith("/keys?per_page=100"):
            return [self.keys]
        if path.endswith("/keys"):
            payload = json.loads(data)
            self.keys.append({"id": 789, **payload})
            if self.fail_write == "deploy-key":
                raise setup.SetupError("Uncertain key registration; reconcile.")
            return self.keys[-1]
        if "variables?" in path:
            return [{"variables": list(self.variables.values())}]
        if "secrets?" in path:
            return [{"secrets": list(self.secrets.values())}]
        if path.endswith("/variables"):
            payload = json.loads(data)
            self.variables[payload["name"]] = payload
            if self.fail_write == "variable":
                raise setup.SetupError("Uncertain variable write; reconcile.")
            return None
        raise AssertionError(arguments)


class ProposalSetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="proposal setup fixtures ")
        self.addCleanup(self.temp.cleanup)
        self.state = Path(self.temp.name) / "private state"
        self.fake = Fixtures()
        for name, replacement in (
            ("api", self.fake.api),
            ("gh", self.fake.gh),
            ("sign_jwt", lambda *_: "fixture-jwt"),
        ):
            mocked = patch.object(setup, name, replacement)
            mocked.start()
            self.addCleanup(mocked.stop)

    def registered(self, **extra):
        with setup.Store(self.state) as store:
            store.save(
                {
                    "purpose": "release-proposal",
                    "phase": "registered",
                    "app": APP,
                    "pem": APP_KEY,
                    **extra,
                }
            )

    def key_ready(self):
        self.registered(
            ssh_private=SSH_KEY, ssh_public="ssh-ed25519 AAAAfixture", deploy_key_id=789
        )
        self.fake.keys = [
            {
                "id": 789,
                "title": "release-proposal Git-only push",
                "key": "ssh-ed25519 AAAAfixture",
                "read_only": False,
            }
        ]

    def run_main(self, command):
        output, errors = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
            result = setup.main([command, "--state-dir", str(self.state)])
        text = output.getvalue() + errors.getvalue()
        for secret in (APP_KEY, SSH_KEY, "fixture-jwt", "fixture-temporary-token"):
            self.assertNotIn(secret, text)
        return result, text

    def test_configure_stores_only_environment_credentials_and_repeat_is_read_only(self):
        self.key_ready()
        self.assertEqual(self.run_main("configure")[0], 0)
        self.assertEqual(
            set(self.fake.secrets), {"PROPOSAL_APP_PRIVATE_KEY", "PROPOSAL_GIT_SSH_KEY"}
        )
        self.assertEqual(self.fake.variables["PROPOSAL_APP_CLIENT_ID"]["value"], "Iv1.fixture")
        self.fake.calls.clear()
        self.assertEqual(self.run_main("configure")[0], 0)
        self.assertFalse(any(call[0] == "gh" and call[2] for call in self.fake.calls))

    def test_wrong_scope_or_environment_ref_never_writes_credentials(self):
        for change in (
            lambda: self.fake.app["permissions"].update(contents="write"),
            lambda: self.fake.app["permissions"].update(actions="write"),
            lambda: self.fake.app.update(owner={"login": "other"}),
            lambda: self.fake.installations[0].update(repository_selection="all"),
            lambda: self.fake.repositories.update(total_count=2),
            lambda: self.fake.refs.append({"id": 2, "name": "main", "type": "tag"}),
            lambda: self.fake.environment.update(protection_rules=[{"type": "required_reviewers"}]),
        ):
            with self.subTest(change=change):
                self.fake.__init__()
                self.key_ready()
                change()
                self.assertEqual(self.run_main("configure")[0], 1)
                self.assertFalse(self.fake.variables or self.fake.secrets)

    def test_uncertain_writes_are_not_repeated(self):
        for failure in ("variable", "PROPOSAL_APP_PRIVATE_KEY", "PROPOSAL_GIT_SSH_KEY"):
            with self.subTest(failure=failure):
                self.fake.__init__()
                self.key_ready()
                self.fake.fail_write = failure
                self.assertEqual(self.run_main("configure")[0], 1)
                self.assertIn("PROPOSAL_APP_CLIENT_ID", self.fake.variables)
                if failure != "variable":
                    self.assertIn(failure, self.fake.secrets)
                self.fake.calls.clear()
                self.fake.fail_write = None
                self.assertEqual(self.run_main("configure")[0], 1)
                self.assertFalse(any(call[0] == "gh" and call[2] for call in self.fake.calls))

    def test_unrelated_or_release_recovery_state_is_refused(self):
        with setup.Store(self.state) as store:
            store.save({"phase": "registered", "app": APP, "pem": APP_KEY})
        self.assertEqual(self.run_main("configure")[0], 1)
        self.assertFalse(self.fake.calls)

    def test_verify_returns_learned_bot_and_revokes_metadata_token(self):
        self.registered()
        code, output = self.run_main("verify")
        self.assertEqual(code, 0)
        self.assertIn('"bot": "fixture-proposal[bot]"', output)
        self.assertIn(("DELETE", "/installation/token", None), self.fake.calls)

    def test_register_entry_point_uses_read_only_manifest_and_reuses_recorded_app(self):
        messages = queue.Queue()
        result = []
        stop = threading.Event()
        with (
            patch("builtins.print", side_effect=lambda message, **_: messages.put(message)),
            patch.object(setup, "time") as clock,
        ):
            clock.monotonic.side_effect = lambda: 3501 if stop.is_set() else 0
            worker = threading.Thread(
                target=lambda: result.append(
                    setup.main(["register", "--state-dir", str(self.state)])
                )
            )
            worker.start()
            try:
                invitation = messages.get(timeout=3)
                url = invitation.removeprefix("Open on this host: ")
                with urllib.request.urlopen(url, timeout=3) as response:
                    form = response.read().decode()
                self.assertIn("contents read", form)
                self.assertIn("&quot;contents&quot;: &quot;read&quot;", form)
                parsed = urllib.parse.urlsplit(url)
                query = urllib.parse.urlencode(
                    {"state": parsed.path.rsplit("/", 1)[1], "code": "fixture-code"}
                )
                with urllib.request.urlopen(
                    f"http://{parsed.netloc}/callback?{query}", timeout=3
                ) as response:
                    self.assertIn(b"Registration saved", response.read())
            finally:
                stop.set()
                worker.join(timeout=5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(result, [0])
        self.assertIn("fixture-proposal/installations/new", messages.get(timeout=1))
        self.fake.calls.clear()
        self.assertEqual(self.run_main("register")[0], 0)
        self.assertFalse(self.fake.calls)

    def test_proposal_callback_rejects_wrong_state_and_replay(self):
        with setup.Store(self.state) as store:
            server = setup.registration_server(
                store, make_manifest=setup.manifest, exchange=setup.convert
            )
            self.addCleanup(server.server_close)
            base = f"http://127.0.0.1:{server.server_port}"

            def request(query):
                worker = threading.Thread(target=server.handle_request)
                worker.start()
                try:
                    with urllib.request.urlopen(base + "/callback?" + query, timeout=3) as response:
                        return response.status
                finally:
                    worker.join(timeout=3)
                    self.assertFalse(worker.is_alive())

            with self.assertRaises(urllib.error.HTTPError) as wrong:
                request("state=wrong&code=fixture-code")
            wrong.exception.close()
            self.assertFalse(self.fake.calls)
            query = urllib.parse.urlencode({"state": server.state, "code": "fixture-code"})
            self.assertEqual(request(query), 200)
            with self.assertRaises(urllib.error.HTTPError) as replay:
                request(query)
            replay.exception.close()
            self.assertEqual(len(self.fake.calls), 1)
            self.assertEqual(store.load()["purpose"], "release-proposal")

    def test_configuration_drift_refuses_without_writes(self):
        for change in (
            lambda: self.fake.secrets["PROPOSAL_APP_PRIVATE_KEY"].update(updated_at="changed"),
            lambda: self.fake.secrets.pop("PROPOSAL_GIT_SSH_KEY"),
            lambda: self.fake.variables["PROPOSAL_APP_CLIENT_ID"].update(value="other"),
            lambda: self.fake.keys[0].update(read_only=True),
        ):
            with self.subTest(change=change):
                self.fake.__init__()
                self.key_ready()
                self.assertEqual(self.run_main("configure")[0], 0)
                change()
                self.fake.calls.clear()
                self.assertEqual(self.run_main("configure")[0], 1)
                self.assertFalse(any(call[0] == "gh" and call[2] for call in self.fake.calls))

    def test_deploy_key_response_loss_never_registers_a_second_key(self):
        self.registered(ssh_private=SSH_KEY, ssh_public="ssh-ed25519 AAAAfixture")
        self.fake.fail_write = "deploy-key"
        self.assertEqual(self.run_main("deploy-key")[0], 1)
        self.assertEqual(len(self.fake.keys), 1)
        self.fake.calls.clear()
        self.fake.fail_write = None
        self.assertEqual(self.run_main("deploy-key")[0], 1)
        self.assertFalse(self.fake.calls)

    def test_generated_key_is_registered_once_and_temporary_files_are_removed(self):
        self.registered()
        self.assertEqual(self.run_main("deploy-key")[0], 0)
        with setup.Store(self.state) as store:
            record = store.load()
        self.assertTrue(record["ssh_private"].startswith("-----BEGIN OPENSSH PRIVATE KEY-----"))
        self.assertEqual(record["deploy_key_id"], 789)
        self.assertEqual(len(self.fake.keys), 1)
        self.assertEqual(list(self.state.iterdir()), [self.state / "state.json"])
        self.assertEqual(self.run_main("deploy-key")[0], 0)
        self.assertEqual(len(self.fake.keys), 1)

    def test_scope_failure_revokes_temporary_token(self):
        self.registered()
        self.fake.repositories["total_count"] = 2
        self.assertEqual(self.run_main("verify")[0], 1)
        self.assertIn(("DELETE", "/installation/token", None), self.fake.calls)

    def test_real_cli_refuses_release_state_without_touching_it(self):
        with setup.Store(self.state) as store:
            store.save({"phase": "registered", "pem": APP_KEY})
        original = (self.state / "state.json").read_bytes()
        result = subprocess.run(
            [
                sys.executable,
                str(ROOT / "tools/setup_proposal_app.py"),
                "configure",
                "--state-dir",
                str(self.state),
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("Not proposal recovery state", result.stderr)
        self.assertNotIn(APP_KEY, result.stderr + result.stdout)
        self.assertEqual((self.state / "state.json").read_bytes(), original)

    def test_real_cli_help_never_accesses_credentials(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/setup_proposal_app.py"), "--help"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("deploy-key", result.stdout)


class ProposalTransportTests(unittest.TestCase):
    def test_real_gh_transport_keeps_post_json_and_paginated_environment_metadata(self):
        transport = importlib.import_module("setup_release_app").gh
        with tempfile.TemporaryDirectory(prefix="proposal transport ") as temporary:
            executable = Path(temporary) / "gh"
            executable.write_text(
                f"#!{sys.executable}\n"
                "import json, os, sys\n"
                "assert 'GH_DEBUG' not in os.environ\n"
                "arguments = sys.argv[1:]\n"
                "if '--input' in arguments:\n"
                "    assert arguments == ['api', 'repos/Androlax2/agent-cockpit/keys', '--input', '-']\n"
                "    body = json.load(sys.stdin)\n"
                "    assert body['read_only'] is False\n"
                "    print(json.dumps({'id': 789, **body}))\n"
                "else:\n"
                "    assert arguments == ['api', '--paginate', '--slurp', 'repos/Androlax2/agent-cockpit/environments/release-proposal/secrets?per_page=100']\n"
                "    print(json.dumps([{'secrets': [{'name': 'FIRST'}]}, {'secrets': [{'name': 'SECOND'}]}]))\n"
            )
            executable.chmod(0o700)
            with (
                patch.dict(
                    os.environ,
                    {"PATH": temporary + os.pathsep + os.environ["PATH"], "GH_DEBUG": "api"},
                ),
                patch.object(setup, "gh", transport),
            ):
                created = setup.gh(
                    ["api", "repos/Androlax2/agent-cockpit/keys", "--input", "-"],
                    json.dumps({"key": "ssh-ed25519 fixture", "read_only": False}).encode(),
                )
                self.assertEqual(created["id"], 789)
                self.assertEqual(
                    setup.metadata(
                        "repos/Androlax2/agent-cockpit/environments/release-proposal/secrets",
                        "secrets",
                    ),
                    [{"name": "FIRST"}, {"name": "SECOND"}],
                )


if __name__ == "__main__":
    unittest.main()

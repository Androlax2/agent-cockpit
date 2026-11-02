"""Release App setup uses isolated state, fake APIs and command transports only."""

import base64
import contextlib
import copy
import importlib.util
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
SPEC = importlib.util.spec_from_file_location(
    "setup_release_app", ROOT / "tools/setup_release_app.py"
)
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)
KEY = "-----BEGIN PRIVATE KEY-----\nfixture-private-material\n-----END PRIVATE KEY-----\n"
APP = {
    "id": 123,
    "client_id": "Iv1.fixture",
    "slug": "fixture-release",
    "owner": {"login": "Androlax2"},
    "permissions": {"contents": "write", "pull_requests": "write", "metadata": "read"},
    "events": [],
}
INSTALL = {
    "id": 456,
    "app_id": 123,
    "account": {"login": "Androlax2"},
    "repository_selection": "selected",
    "permissions": APP["permissions"],
    "suspended_at": None,
}


class Fixtures:
    def __init__(self):
        self.app = copy.deepcopy(APP)
        self.installations = [copy.deepcopy(INSTALL)]
        self.repositories = {"total_count": 1, "repositories": [{"full_name": setup.REPOSITORY}]}
        self.variables = {}
        self.secrets = {}
        self.calls = []
        self.fail_secret = False

    def api(self, method, path, token=None, data=None):
        self.calls.append((method, path, token, data))
        if path.startswith("/app-manifests/"):
            return {**self.app, "pem": KEY, "client_secret": "discard-me"}
        if path == "/app":
            return self.app
        if path.startswith("/app/installations?"):
            return self.installations
        if path.endswith("/access_tokens"):
            assert data == {"permissions": {"metadata": "read"}}
            return {"token": "fixture-installation-token"}
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
            assert arguments == ["secret", "set", setup.SECRET, "--repo", setup.REPOSITORY]
            assert data == KEY.encode()
            self.secrets[setup.SECRET] = {"name": setup.SECRET, "updated_at": "fixture-timestamp"}
            if self.fail_secret:
                raise setup.SetupError("Secret write failed; reconcile before retrying.")
            return None
        if "--method" in arguments:
            payload = json.loads(data)
            self.variables[payload["name"]] = payload
            return None
        if "variables?" in arguments[-1]:
            return [{"variables": list(self.variables.values())}]
        if "secrets?" in arguments[-1]:
            return [{"secrets": list(self.secrets.values())}]
        raise AssertionError(arguments)


class SetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="release app fixtures ")
        self.addCleanup(self.temp.cleanup)
        self.state = Path(self.temp.name) / "private state"
        self.fake = Fixtures()
        for target, replacement in (
            ("api", self.fake.api),
            ("gh", self.fake.gh),
            ("sign_jwt", lambda *_: "fixture-jwt"),
        ):
            mock = patch.object(setup, target, replacement)
            mock.start()
            self.addCleanup(mock.stop)

    def registered(self):
        with setup.Store(self.state) as store:
            store.save({"phase": "registered", "app": APP, "pem": KEY})

    def run_main(self, command):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = setup.main([command, "--state-dir", str(self.state)])
        output = out.getvalue() + err.getvalue()
        for secret in (
            KEY,
            "fixture-private-material",
            "fixture-jwt",
            "fixture-installation-token",
        ):
            self.assertNotIn(secret, output)
        return code, output

    def test_manifest_has_only_approved_permissions_and_no_webhook_delivery(self):
        manifest = setup.manifest("http://127.0.0.1:12345/callback")
        self.assertEqual(
            manifest["default_permissions"], {"contents": "write", "pull_requests": "write"}
        )
        self.assertEqual(manifest["default_events"], [])
        self.assertFalse(manifest["public"])
        self.assertFalse(manifest["hook_attributes"]["active"])
        self.assertEqual(manifest["redirect_url"], "http://127.0.0.1:12345/callback")

    def test_configure_verifies_scope_uploads_key_on_stdin_and_repeats_without_writes(self):
        self.registered()
        self.assertEqual(self.run_main("configure")[0], 0)
        self.assertEqual(self.fake.variables[setup.VARIABLE]["value"], APP["client_id"])
        self.assertIn(setup.SECRET, self.fake.secrets)
        self.assertTrue(
            any(call[:2] == ("DELETE", "/installation/token") for call in self.fake.calls)
        )
        writes = [
            call
            for call in self.fake.calls
            if call[0] == "gh" and (call[1][0] == "secret" or "--method" in call[1])
        ]
        self.assertEqual(len(writes), 2)
        self.fake.calls.clear()
        self.assertEqual(self.run_main("configure")[0], 0)
        self.assertFalse(
            any(
                call[0] == "gh" and (call[1][0] == "secret" or "--method" in call[1])
                for call in self.fake.calls
            )
        )
        self.assertEqual(self.state.stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.state / "state.json").stat().st_mode & 0o777, 0o600)

    def test_broad_scope_wrong_owner_or_permissions_never_write_configuration(self):
        self.registered()
        cases = [
            lambda: self.fake.installations[0].update(repository_selection="all"),
            lambda: self.fake.installations.append(copy.deepcopy(INSTALL)),
            lambda: self.fake.installations[0].update(account={"login": "other"}),
            lambda: self.fake.app["permissions"].update(actions="write"),
            lambda: self.fake.app.update(owner={"login": "other"}),
            lambda: self.fake.repositories.update(total_count=2),
            lambda: self.fake.repositories["repositories"][0].update(full_name="Androlax2/other"),
        ]
        for change in cases:
            with self.subTest(change=change):
                self.fake.__init__()
                change()
                self.assertEqual(self.run_main("configure")[0], 1)
                self.assertEqual(self.fake.variables, {})
                self.assertEqual(self.fake.secrets, {})

    def test_repository_check_failure_still_revokes_temporary_token(self):
        self.registered()
        self.fake.repositories["total_count"] = 2
        self.assertEqual(self.run_main("configure")[0], 1)
        self.assertIn(
            ("DELETE", "/installation/token", "fixture-installation-token", None), self.fake.calls
        )

    def test_existing_credentials_and_uncertain_secret_upload_are_not_overwritten(self):
        self.registered()
        self.fake.secrets[setup.SECRET] = {"updated_at": "someone-else"}
        self.assertEqual(self.run_main("configure")[0], 1)
        self.assertEqual(self.fake.variables, {})
        self.fake.secrets.clear()
        self.fake.variables[setup.VARIABLE] = {"name": setup.VARIABLE, "value": "other-id"}
        self.assertEqual(self.run_main("configure")[0], 1)
        self.fake.variables.clear()
        self.fake.fail_secret = True
        self.assertEqual(self.run_main("configure")[0], 1)
        self.fake.calls.clear()
        self.fake.fail_secret = False
        self.assertEqual(self.run_main("configure")[0], 1)
        self.assertFalse(
            any(call[0] == "gh" and call[1][0] == "secret" for call in self.fake.calls)
        )

    def test_registration_repeat_does_not_create_app_and_pending_state_refuses(self):
        self.registered()
        code, output = self.run_main("register")
        self.assertEqual(code, 0)
        self.assertIn("https://github.com/apps/fixture-release/installations/new", output)
        self.assertFalse(self.fake.calls)
        with setup.Store(self.state) as store:
            store.save({"phase": "waiting"})
        self.assertEqual(self.run_main("register")[0], 1)
        self.assertFalse(self.fake.calls)

    def test_symlink_or_public_state_refused_without_modifying_target(self):
        self.state.symlink_to(Path(self.temp.name), target_is_directory=True)
        self.assertEqual(self.run_main("configure")[0], 1)
        self.state.unlink()
        self.state.mkdir(mode=0o755)
        self.assertEqual(self.run_main("configure")[0], 1)
        self.state.chmod(0o700)
        target = Path(self.temp.name) / "innocent"
        target.write_text("unchanged")
        (self.state / "state.json").symlink_to(target)
        self.assertEqual(self.run_main("configure")[0], 1)
        self.assertEqual(target.read_text(), "unchanged")

    def test_loopback_callback_requires_state_and_is_consumed_once(self):
        with setup.Store(self.state) as store:
            server = setup.registration_server(store)
            self.addCleanup(server.server_close)
            base = f"http://127.0.0.1:{server.server_port}"

            def request(path):
                worker = threading.Thread(target=server.handle_request)
                worker.start()
                try:
                    return urllib.request.urlopen(base + path, timeout=3).read().decode()
                except urllib.error.HTTPError as error:
                    error.close()
                    raise
                finally:
                    worker.join(timeout=3)
                    self.assertFalse(worker.is_alive())

            with self.assertRaises(urllib.error.HTTPError):
                request("/callback?state=wrong&code=fixture-code")
            self.assertFalse(self.fake.calls)
            form = request("/start/" + server.state)
            self.assertIn("https://github.com/settings/apps/new?state=", form)
            query = urllib.parse.urlencode({"state": server.state, "code": "fixture-code"})
            self.assertIn("Registration saved", request("/callback?" + query))
            self.assertEqual(store.load()["pem"], KEY)
            self.assertNotIn("client_secret", json.dumps(store.load()))
            with self.assertRaises(urllib.error.HTTPError):
                request("/callback?" + query)
            conversions = [
                call for call in self.fake.calls if call[1].startswith("/app-manifests/")
            ]
            self.assertEqual(len(conversions), 1)

    def test_register_entry_point_runs_browser_handshake_without_exposing_credentials(self):
        messages = queue.Queue()
        result = []
        with patch("builtins.print", side_effect=lambda message, **_: messages.put(message)):
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
                    self.assertIn(b"Continue to GitHub", response.read())
                parsed = urllib.parse.urlsplit(url)
                query = urllib.parse.urlencode(
                    {"state": parsed.path.rsplit("/", 1)[1], "code": "fixture-code"}
                )
                with urllib.request.urlopen(
                    f"http://{parsed.netloc}/callback?{query}", timeout=3
                ) as response:
                    self.assertIn(b"Registration saved", response.read())
            finally:
                worker.join(timeout=5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(result, [0])
        self.assertIn(
            "https://github.com/apps/fixture-release/installations/new", messages.get(timeout=1)
        )
        with setup.Store(self.state) as store:
            self.assertEqual(store.load()["pem"], KEY)

    def test_conversion_failure_is_private_and_cannot_be_retried_implicitly(self):
        with setup.Store(self.state) as store:
            store.save({"phase": "waiting"})
            with patch.object(setup, "api", side_effect=setup.SetupError("GitHub request failed.")):
                with self.assertRaises(setup.SetupError):
                    setup.convert(store, "fixture-code")
            self.assertEqual(store.load()["phase"], "exchanging")
        self.assertEqual(self.run_main("register")[0], 1)


class TransportTests(unittest.TestCase):
    def test_api_error_never_discloses_conversion_url_or_response(self):
        failure = urllib.error.HTTPError(
            "https://api.github.com/app-manifests/private-code/conversions",
            403,
            "private-message",
            {},
            io.BytesIO(KEY.encode()),
        )
        with patch.object(urllib.request.OpenerDirector, "open", side_effect=failure):
            with self.assertRaises(setup.SetupError) as error:
                setup.api("POST", "/app-manifests/private-code/conversions")
        self.assertIn("HTTP 403", str(error.exception))
        self.assertNotIn("private", str(error.exception))

    def test_real_openssl_signs_fixture_jwt_and_removes_temporary_key(self):
        with tempfile.TemporaryDirectory() as temporary:
            key = Path(temporary) / "fixture.pem"
            subprocess.run(
                [
                    "openssl",
                    "genpkey",
                    "-algorithm",
                    "RSA",
                    "-pkeyopt",
                    "rsa_keygen_bits:2048",
                    "-out",
                    str(key),
                ],
                capture_output=True,
                check=True,
            )
            public = subprocess.run(
                ["openssl", "pkey", "-in", str(key), "-pubout"], capture_output=True, check=True
            ).stdout
            (Path(temporary) / "public.pem").write_bytes(public)
            signed = setup.sign_jwt({"app": APP, "pem": key.read_text()}, Path(temporary))
            header, payload, signature = signed.split(".")
            claims = json.loads(base64.urlsafe_b64decode(payload + "=="))
            self.assertEqual(claims["iss"], APP["client_id"])
            self.assertEqual(claims["exp"] - claims["iat"], 600)
            signature_path = Path(temporary) / "signature"
            signature_path.write_bytes(base64.urlsafe_b64decode(signature + "=="))
            verified = subprocess.run(
                [
                    "openssl",
                    "dgst",
                    "-sha256",
                    "-verify",
                    str(Path(temporary) / "public.pem"),
                    "-signature",
                    str(signature_path),
                ],
                input=f"{header}.{payload}".encode(),
                capture_output=True,
                check=False,
            )
            self.assertEqual(verified.returncode, 0)
            self.assertFalse(list(Path(temporary).glob(".signing-key-*")))

    def test_real_cli_help_does_not_access_host_credentials(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/setup_release_app.py"), "--help"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("register", result.stdout)

    def test_gh_transport_uses_stdin_and_suppresses_debug_and_error_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "gh"
            binary.write_text(
                "#!/usr/bin/env python3\nimport os, sys\n"
                "assert 'GH_DEBUG' not in os.environ\n"
                "assert os.environ['GH_HOST'] == 'github.com'\n"
                "assert 'fixture-private-material' not in ' '.join(sys.argv)\n"
                "assert 'fixture-private-material' in sys.stdin.read()\n"
                "print('fixture-private-material', file=sys.stderr)\n"
                "sys.exit(1)\n"
            )
            binary.chmod(0o700)
            with patch.dict(
                os.environ, {"PATH": temporary + os.pathsep + os.environ["PATH"], "GH_DEBUG": "api"}
            ):
                with self.assertRaises(setup.SetupError) as error:
                    setup.gh(
                        ["secret", "set", setup.SECRET, "--repo", setup.REPOSITORY], KEY.encode()
                    )
            self.assertNotIn("fixture-private-material", str(error.exception))


if __name__ == "__main__":
    unittest.main()

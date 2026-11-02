"""Safety and evidence failures, with temporary files and harmless fake processes."""

import json
import os
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from native import (
    SKILL_DESCRIPTION,
    Model,
    environment,
    evidence,
    initialize_project,
    prepare,
    refuse_existing,
    run_bounded,
    write_json,
)


class Safety(unittest.TestCase):
    def test_fixture_git_never_runs_the_fake_owners_hooks_or_signer(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "fixture home"
            fixture.mkdir()
            (fixture / "probe-processes").mkdir()
            owner = root / "fake owner"
            hooks = owner / "hooks"
            hooks.mkdir(parents=True)
            marker = root / "owner-command-ran"
            hook = hooks / "pre-commit"
            hook.write_text("#!/bin/sh\ntouch " + shlex.quote(str(marker)) + "\nexit 1\n")
            hook.chmod(0o755)
            config = owner / ".gitconfig"
            config.write_text(
                f"[core]\n hooksPath = {json.dumps(str(hooks))}\n"
                f"[commit]\n gpgsign = true\n[gpg]\n program = {json.dumps(str(hook))}\n"
            )
            with patch.dict(
                os.environ,
                {
                    "HOME": str(owner),
                    "GIT_CONFIG_GLOBAL": str(config),
                    "GIT_CONFIG_COUNT": "1",
                    "GIT_CONFIG_KEY_0": "core.hooksPath",
                    "GIT_CONFIG_VALUE_0": str(hooks),
                    "GIT_TEMPLATE_DIR": str(owner),
                },
                clear=True,
            ):
                initialize_project(fixture)
            self.assertFalse(marker.exists())
            self.assertFalse(
                (fixture / "projects/native-instructions/.git/hooks/pre-commit").exists()
            )
            self.assertTrue(
                (fixture / "projects/native-instructions/.git/refs/heads/main").is_file()
            )

    def test_version_wrapper_cleans_up_before_the_outer_discovery_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pid_file = root / "fake-native.pid"
            program = root / "fake-provider"
            program.write_text(
                "#!/bin/sh\nprintf '%s' \"$$\" > "
                + shlex.quote(str(pid_file))
                + "\nexec /bin/sleep 30\n"
            )
            program.chmod(0o755)
            config = {
                "root": directory,
                "provider": "claude",
                "program": str(program),
                "version": "2.1.316",
            }
            prepare(config, "http://127.0.0.1:9")
            (root / "ready").write_text("http://127.0.0.1:9")
            config_file = root / "fixture.json"
            config_file.write_text(json.dumps(config))
            wrapper = subprocess.Popen(
                [
                    sys.executable,
                    "-I",
                    "-B",
                    str(Path(__file__).with_name("native.py")),
                    "launch",
                    str(config_file),
                    "--version",
                ],
                cwd=root,
                env={"HOME": directory, "PATH": "/usr/bin:/bin"},
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            try:
                _, errors = wrapper.communicate(timeout=10)
                self.assertNotEqual(wrapper.returncode, 0)
                self.assertIn(b"native fixture process timed out", errors)
                self.assertEqual(list((root / "probe-processes").iterdir()), [])
                self.assertFalse(Path(f"/proc/{pid_file.read_text()}/stat").exists())
            finally:
                if wrapper.poll() is None:
                    wrapper.kill()
                    wrapper.wait()
                # A regression of the original outer-timeout race must clean its
                # own still-sleeping fake child, never leave it behind in CI.
                if pid_file.exists():
                    pid = int(pid_file.read_text())
                    try:
                        if Path(f"/proc/{pid}/cwd").resolve() == root:
                            os.killpg(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_external_symlinks_and_metadata_errors_are_refused_without_reading(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy"
            refuse_existing([path])
            path.symlink_to("nonexistent-secret")
            with self.assertRaisesRegex(AssertionError, "external configuration"):
                refuse_existing([path])
            with patch.object(Path, "lstat", side_effect=PermissionError("unreadable")):
                with self.assertRaises(PermissionError):
                    refuse_existing([path])

    def test_owner_credentials_and_helpers_do_not_enter_the_wrapper_environment(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            patch.dict(
                os.environ,
                {
                    "ANTHROPIC_AUTH_TOKEN": "owner-secret",
                    "OPENAI_API_KEY": "owner-secret",
                    "HTTP_PROXY": "https://owner-proxy.invalid",
                    "CODEX_HOME": "/owner/config",
                    "CLAUDE_CODE_API_KEY_HELPER_TTL_MS": "5",
                    "TORLEK_SESSION": "fixture-session",
                },
                clear=True,
            ),
        ):
            env = environment({"root": directory, "provider": "claude"}, "http://127.0.0.1:9")
            self.assertNotIn("owner-secret", str(env))
            self.assertNotIn("HTTP_PROXY", env)
            self.assertNotIn("CLAUDE_CODE_API_KEY_HELPER_TTL_MS", env)
            self.assertEqual(env["TORLEK_SESSION"], "fixture-session")
            self.assertEqual(env["CODEX_HOME"], str(Path(directory) / ".codex"))

    def test_a_timed_out_wrapper_stops_its_descendant_and_clears_its_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "probe-processes").mkdir()
            late = root / "descendant-ran"
            with self.assertRaisesRegex(AssertionError, "timed out"):
                run_bounded(
                    ["/bin/sh", "-c", '(sleep 0.4; touch "$1") & wait', "fixture", str(late)],
                    {"HOME": directory, "PATH": "/usr/bin:/bin"},
                    0.1,
                )
            time.sleep(0.45)
            self.assertFalse(late.exists())
            self.assertEqual(list((root / "probe-processes").iterdir()), [])

    def test_cooperative_stop_does_not_need_the_daemon_to_survive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "probe-processes").mkdir()
            (root / "stop").touch()
            status = run_bounded(["/bin/sleep", "30"], {"HOME": directory}, 1)
            self.assertLess(status, 0)
            self.assertEqual(list((root / "probe-processes").iterdir()), [])


class Evidence(unittest.TestCase):
    def test_model_process_acknowledges_drain_by_exiting_successfully(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / "fixture.json"
            config.write_text(json.dumps({"root": directory, "provider": "claude"}))
            child = subprocess.Popen(
                [
                    sys.executable,
                    "-I",
                    "-B",
                    str(Path(__file__).with_name("native.py")),
                    "serve",
                    str(config),
                ],
                cwd=root,
                env={"HOME": directory, "PATH": "/usr/bin:/bin"},
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                while not (root / "ready").exists():
                    self.assertIsNone(child.poll(), "model exited before becoming ready")
                    self.assertLess(time.monotonic(), deadline, "model did not become ready")
                    time.sleep(0.01)
                (root / "drain").touch()
                _, errors = child.communicate(timeout=2)
                self.assertEqual(child.returncode, 0, errors.decode())
            finally:
                if child.poll() is None:
                    child.kill()
                child.communicate(timeout=2)

    def test_model_close_waits_for_an_inflight_error_to_be_recorded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pending_error = threading.Event()
            release_error = threading.Event()
            error_written = threading.Event()
            closed = threading.Event()

            def delayed_error(path, value):
                pending_error.set()
                if not release_error.wait(3):
                    raise TimeoutError("test did not release the error writer")
                try:
                    write_json(path, value)
                finally:
                    error_written.set()

            model = Model({"root": directory, "provider": "opencode", "prompt": "OK"})
            serving = threading.Thread(target=model.serve_forever, daemon=True)

            def close_model():
                model.server_close()
                closed.set()

            closing = threading.Thread(target=close_model, daemon=True)
            with patch("native.write_json", side_effect=delayed_error):
                serving.start()
                try:
                    with socket.create_connection(model.server_address, timeout=2) as client:
                        client.sendall(
                            b"POST /v1/chat/completions HTTP/1.0\r\n"
                            b"Authorization: Bearer fixture-key\r\n"
                            b"Content-Length: 1\r\n\r\n{"
                        )
                        self.assertTrue(pending_error.wait(2), "handler did not reach its error")
                        model.shutdown()
                        closing.start()
                        self.assertFalse(
                            closed.wait(0.1), "model closed before recording its in-flight error"
                        )
                        release_error.set()
                        self.assertTrue(closed.wait(2), "model did not finish after the error")
                        self.assertIn("JSONDecodeError", (root / "error.json").read_text())
                finally:
                    release_error.set()
                    model.shutdown()
                    model.server_close()
                    serving.join(timeout=2)
                    if closing.ident is not None:
                        closing.join(timeout=2)
                    if pending_error.is_set():
                        self.assertTrue(error_written.wait(2), "error writer did not stop")

    def test_codex_mcp_tools_are_registered_under_the_captured_namespace(self):
        config = {"provider": "codex", "prompt": "Reply with only OK."}
        request = {
            "input": [{"role": "user", "content": config["prompt"]}],
            "tools": [
                {
                    "type": "namespace",
                    "name": "mcp__torlek",
                    "tools": [
                        {"type": "function", "name": "torlek_brief"},
                        {"type": "function", "name": "torlek_docs"},
                    ],
                }
            ],
        }
        self.assertTrue(evidence(config, request, 1)["mcp"])
        request["tools"][0]["name"] = "unrelated"
        self.assertFalse(evidence(config, request, 1)["mcp"])

    def test_tool_descriptions_do_not_count_as_registered_mcp_tools(self):
        config = {"provider": "opencode", "prompt": "Reply with only OK."}
        request = {
            "messages": [{"role": "user", "content": config["prompt"]}],
            "tools": [{"function": {"name": "read", "description": "torlek_brief torlek_docs"}}],
        }
        self.assertFalse(evidence(config, request, 1)["mcp"])

    def test_a_description_without_native_skill_registration_is_not_evidence(self):
        config = {"provider": "opencode", "prompt": "Reply with only OK."}
        request = {
            "messages": [
                {"role": "user", "content": config["prompt"]},
                {"role": "system", "content": SKILL_DESCRIPTION},
            ],
            "tools": [{"function": {"name": "read"}}],
        }
        self.assertFalse(evidence(config, request, 1)["skill"])

    def test_a_prompt_or_tool_echo_cannot_prove_codex_hook_delivery(self):
        config = {"provider": "codex", "prompt": "Reply with only OK."}
        request = {
            "input": [
                {"role": "user", "content": "Reply with only OK."},
                {"role": "user", "content": "# Torlek launch brief"},
            ],
            "tools": [{"name": "echo", "description": SKILL_DESCRIPTION}],
        }
        captured = evidence(config, request, 1)
        self.assertTrue(captured["main"])
        self.assertEqual(captured["context"], "")
        self.assertFalse(captured["skill"])
        self.assertFalse(captured["mcp"])
        request["input"][0]["content"] += " Read the brief first."
        self.assertFalse(evidence(config, request, 1)["main"])

    def test_claude_deferred_tools_require_native_system_context_and_tool_search(self):
        config = {"provider": "claude", "prompt": "Reply with only OK."}
        context = (
            "SessionStart hook additional context: # Torlek launch brief\n" + SKILL_DESCRIPTION
        )
        context += "\nmcp__torlek__torlek_brief\nmcp__torlek__torlek_docs"
        context += (
            "\nThe following skills are available for use with the Skill tool:\n- torlek: "
            + SKILL_DESCRIPTION
        )
        request = {
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": config["prompt"]}]},
                {"role": "system", "content": [{"type": "text", "text": context}]},
            ],
            "tools": [{"name": "Skill"}, {"name": "ToolSearch"}],
        }
        captured = evidence(config, request, 1)
        self.assertTrue(captured["main"] and captured["skill"] and captured["mcp"])
        self.assertEqual(captured["mcp_advertisement"], "deferred")
        request["tools"] = [{"name": "Skill"}]
        self.assertFalse(evidence(config, request, 1)["mcp"])
        request["messages"][1]["role"] = "assistant"
        self.assertEqual(evidence(config, request, 1)["context"], "")


if __name__ == "__main__":
    unittest.main()

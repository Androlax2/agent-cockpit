"""Safety and evidence failures, with temporary files and harmless fake processes."""

import os
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from native import SKILL_DESCRIPTION, environment, evidence, refuse_existing, run_bounded


class Safety(unittest.TestCase):
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
                    "COCKPIT_SESSION": "fixture-session",
                },
                clear=True,
            ),
        ):
            env = environment({"root": directory, "provider": "claude"}, "http://127.0.0.1:9")
            self.assertNotIn("owner-secret", str(env))
            self.assertNotIn("HTTP_PROXY", env)
            self.assertNotIn("CLAUDE_CODE_API_KEY_HELPER_TTL_MS", env)
            self.assertEqual(env["COCKPIT_SESSION"], "fixture-session")
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
    def test_codex_mcp_tools_are_registered_under_the_captured_namespace(self):
        config = {"provider": "codex", "prompt": "Reply with only OK."}
        request = {
            "input": [{"role": "user", "content": config["prompt"]}],
            "tools": [
                {
                    "type": "namespace",
                    "name": "mcp__cockpit",
                    "tools": [
                        {"type": "function", "name": "cockpit_brief"},
                        {"type": "function", "name": "cockpit_docs"},
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
            "tools": [{"function": {"name": "read", "description": "cockpit_brief cockpit_docs"}}],
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
                {"role": "user", "content": "# Cockpit launch brief"},
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
            "SessionStart hook additional context: # Cockpit launch brief\n" + SKILL_DESCRIPTION
        )
        context += "\nmcp__cockpit__cockpit_brief\nmcp__cockpit__cockpit_docs"
        context += (
            "\nThe following skills are available for use with the Skill tool:\n- cockpit: "
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

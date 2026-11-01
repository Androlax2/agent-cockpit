import json
import os
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
HELPER = REPO / "config/bin/.local/bin/cockpit-agents/codex-trust.py"
HOOKS = REPO / "config/codex/.codex/hooks.json"


class CodexTrustTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.home = Path(self.scratch.name)
        self.codex = self.home / ".codex"
        self.codex.mkdir()
        self.config = self.codex / "config.toml"
        self.source = self.home / "source checkout"
        self.clone = self.home / 'clone "quoted"'
        self.source.mkdir()
        self.clone.mkdir()
        self.config.write_text(
            f'# Keep this comment\nmodel = "example"\n[projects.{json.dumps(str(self.source))}]\n'
            'trust_level = "trusted"\n'
        )
        self.environment = dict(os.environ, HOME=str(self.home))
        self.environment.pop("CODEX_HOME", None)

    def run_helper(self, *arguments):
        return subprocess.run(
            [sys.executable, str(HELPER), *map(str, arguments)],
            env=self.environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def settings(self):
        return tomllib.loads(self.config.read_text())

    def install_hooks(self):
        (self.codex / "hooks.json").write_bytes(HOOKS.read_bytes())

    def test_trusted_source_clone_inherits_without_replacing_config_inode(self):
        before = self.config.read_bytes()
        inode = self.config.stat().st_ino
        with self.config.open("rb") as mounted:
            result = self.run_helper("inherit", self.source, self.clone)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                tomllib.loads(mounted.read().decode())["projects"][str(self.clone)]["trust_level"],
                "trusted",
            )
        self.assertEqual(self.config.stat().st_ino, inode)
        self.assertTrue(self.config.read_bytes().startswith(before))

    def test_untrusted_or_missing_source_never_grants_trust(self):
        for trust in ['"untrusted"', '"unknown"', None]:
            with self.subTest(trust=trust):
                self.config.write_text(
                    f"[projects.{json.dumps(str(self.source))}]\n"
                    + (f"trust_level = {trust}\n" if trust else "")
                )
                before = self.config.read_bytes()
                result = self.run_helper("inherit", self.source, self.clone)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.config.read_bytes(), before)

    def test_explicitly_untrusted_destination_is_not_overridden(self):
        with self.config.open("a") as stream:
            stream.write(f'[projects.{json.dumps(str(self.clone))}]\ntrust_level = "untrusted"\n')
        before = self.config.read_bytes()
        result = self.run_helper("inherit", self.source, self.clone)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.config.read_bytes(), before)

    def test_only_reviewed_user_hooks_receive_codex_hashes(self):
        self.install_hooks()
        project_hooks = self.clone / ".codex"
        project_hooks.mkdir()
        (project_hooks / "hooks.json").write_bytes(HOOKS.read_bytes())
        result = self.run_helper("hooks")
        self.assertEqual(result.returncode, 0, result.stderr)
        states = self.settings()["hooks"]["state"]
        # Codex 0.161.0 discovery normalization + fingerprint.rs canonical JSON SHA-256.
        expected = {
            "session_start": "5905eaaa30869b2dc58da4cb49c6e9ff04b38bce012056d042c9bf44b8e77862",
            "user_prompt_submit": "8ce3c69caac58d384919eaccd7361dac505c4d11a8c9acfc4b847956782bf881",
            "permission_request": "5809080394c41dd12a3e6edbc3a2e5961224c69c5e76ae0b39aaaaf29780c417",
            "post_tool_use": "2c28f8e968a905b5af1e3478394e15b83f7ab337faad51f6b8135a0ec9c1246c",
            "stop": "33c56575e32bc4267f574e9e267b9d7f35a25957c3646f06f2cdf4f5cbf327d1",
            "interrupt": "40f50ec17d91e1e0efbfb408253a545ccbb07dd77322248df020b71d0b6153df",
            "session_end": "d117f3ff8aa280051724f4359636f071a80ebabb69af23f3d6c591c7f7da4545",
        }
        self.assertEqual(
            states,
            {
                f"{self.codex}/hooks.json:{event}:0:0": {"trusted_hash": f"sha256:{digest}"}
                for event, digest in expected.items()
            },
        )
        self.assertNotIn(str(self.clone), self.settings()["projects"])
        first = self.config.read_bytes()
        self.assertEqual(self.run_helper("hooks").returncode, 0)
        self.assertEqual(self.config.read_bytes(), first)

    def test_changed_or_additional_user_hook_is_not_auto_trusted(self):
        for change in ["command", "timeout", "extra"]:
            with self.subTest(change=change):
                hooks = json.loads(HOOKS.read_text())
                if change == "extra":
                    hooks["hooks"]["PreToolUse"] = hooks["hooks"]["Stop"]
                else:
                    hooks["hooks"]["Stop"][0]["hooks"][0][change] = "unreviewed"
                (self.codex / "hooks.json").write_text(json.dumps(hooks))
                before = self.config.read_bytes()
                result = self.run_helper("hooks")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("reviewed", result.stderr)
                self.assertEqual(self.config.read_bytes(), before)

    def test_disabled_hook_is_preserved(self):
        self.install_hooks()
        key = f"{self.codex}/hooks.json:stop:0:0"
        with self.config.open("a") as stream:
            stream.write(f"[hooks.state.{json.dumps(key)}]\nenabled = false\n")
        result = self.run_helper("hooks")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.settings()["hooks"]["state"][key], {"enabled": False})

    def test_malformed_config_is_unchanged(self):
        self.config.write_text("[broken\n")
        result = self.run_helper("inherit", self.source, self.clone)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.config.read_text(), "[broken\n")

    def test_symlinked_config_target_keeps_its_inode(self):
        target = self.home / "host-config.toml"
        self.config.rename(target)
        self.config.symlink_to(target)
        inode = target.stat().st_ino
        result = self.run_helper("inherit", self.source, self.clone)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.config.is_symlink())
        self.assertEqual(target.stat().st_ino, inode)

    def test_unsupported_existing_inline_state_fails_without_writing(self):
        self.install_hooks()
        key = f"{self.codex}/hooks.json:stop:0:0"
        with self.config.open("a") as stream:
            stream.write(f"[hooks.state]\n{json.dumps(key)} = {{ enabled = true }}\n")
        before = self.config.read_bytes()
        result = self.run_helper("hooks")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.config.read_bytes(), before)

    def launch(self):
        return subprocess.run(
            [
                "bash",
                "-c",
                'source "$1"; require_plain_directory() { :; }; '
                'detached_size() { echo "132 40"; }; '
                'tmux() { cp "$HOME/.codex/config.toml" "$HOME/config-at-launch"; }; '
                'codex_launch "$2"',
                "bash",
                str(HELPER.with_name("codex.sh")),
                str(self.clone),
            ],
            env=self.environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_launch_records_hook_trust_before_starting_tmux(self):
        self.install_hooks()
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        launched = tomllib.loads((self.home / "config-at-launch").read_text())
        self.assertEqual(len(launched.get("hooks", {}).get("state", {})), 7)

    def test_launch_refuses_malformed_trust_config_before_tmux(self):
        self.config.write_text("[broken\n")
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.home / "config-at-launch").exists())


if __name__ == "__main__":
    unittest.main()

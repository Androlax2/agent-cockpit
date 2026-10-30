import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class CrateBoundaryTests(unittest.TestCase):
    def test_core_test_support_is_absent_from_normal_application_dependencies(self):
        for consumer in ("cockpit", "cockpit-daemon"):
            with self.subTest(consumer=consumer):
                result = subprocess.run(
                    [
                        "cargo",
                        "tree",
                        "--locked",
                        "--manifest-path",
                        str(ROOT / "apps/Cargo.toml"),
                        "--package",
                        consumer,
                        "--edges",
                        "normal",
                        "--prefix",
                        "none",
                        "--format",
                        "{p} [{f}]",
                    ],
                    capture_output=True,
                    text=True,
                    check=True,
                )
                core = [
                    line for line in result.stdout.splitlines() if line.startswith("cockpit-core ")
                ]
                self.assertTrue(core, "application must consume the real core package")
                for dependency in core:
                    self.assertNotIn("test-support", dependency)

    def test_core_runtime_dependencies_do_not_include_ui_or_daemon_packages(self):
        self.assert_runtime_boundary(
            "cockpit-core",
            ("gtk", "gdk", "gsk", "ratatui", "crossterm"),
            ("cockpit", "osd", "cockpit-daemon"),
        )

    def test_daemon_runtime_dependencies_do_not_include_gtk(self):
        self.assert_runtime_boundary("cockpit-daemon", ("gtk", "gdk", "gsk"))

    def test_daemon_runtime_dependencies_do_not_include_the_terminal_application(self):
        self.assert_runtime_boundary("cockpit-daemon", ("ratatui", "crossterm"), ("cockpit",))

    def test_protocol_runtime_dependencies_do_not_include_ui_or_sqlite(self):
        self.assert_runtime_boundary(
            "cockpit-protocol",
            ("gtk", "gdk", "gsk", "ratatui", "crossterm", "rusqlite", "libsqlite"),
        )

    def assert_runtime_boundary(self, crate_name, forbidden_prefixes, forbidden_packages=()):
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--format-version",
                "1",
                "--manifest-path",
                str(ROOT / "apps/Cargo.toml"),
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        metadata = json.loads(result.stdout)
        packages = {package["id"]: package["name"] for package in metadata["packages"]}
        roots = [identity for identity, name in packages.items() if name == crate_name]
        self.assertEqual(len(roots), 1, f"{crate_name} must be a real workspace crate")
        graph = {node["id"]: node["deps"] for node in metadata["resolve"]["nodes"]}
        pending = list(roots)
        visited = set()
        while pending:
            identity = pending.pop()
            if identity in visited:
                continue
            visited.add(identity)
            self.assertNotIn(
                packages[identity],
                forbidden_packages,
                f"{crate_name} runtime reaches {packages[identity]}",
            )
            self.assertFalse(
                packages[identity].startswith(forbidden_prefixes),
                f"{crate_name} runtime reaches {packages[identity]}",
            )
            pending.extend(
                dependency["pkg"]
                for dependency in graph[identity]
                if any(kind["kind"] is None for kind in dependency["dep_kinds"])
            )

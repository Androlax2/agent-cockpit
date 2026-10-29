import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class CrateBoundaryTests(unittest.TestCase):
    def test_daemon_runtime_dependencies_do_not_include_gtk(self):
        self.assert_runtime_boundary("cockpit-daemon", ("gtk", "gdk", "gsk"))

    def test_protocol_runtime_dependencies_do_not_include_ui_or_sqlite(self):
        self.assert_runtime_boundary(
            "cockpit-protocol",
            ("gtk", "gdk", "gsk", "ratatui", "crossterm", "rusqlite", "libsqlite"),
        )

    def assert_runtime_boundary(self, crate_name, forbidden_prefixes):
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
            self.assertFalse(
                packages[identity].startswith(forbidden_prefixes),
                f"{crate_name} runtime reaches {packages[identity]}",
            )
            pending.extend(
                dependency["pkg"]
                for dependency in graph[identity]
                if any(kind["kind"] is None for kind in dependency["dep_kinds"])
            )

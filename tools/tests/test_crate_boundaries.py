import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class CrateBoundaryTests(unittest.TestCase):
    def test_daemon_runtime_dependencies_do_not_include_gtk(self):
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--offline",
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
        daemon = [identity for identity, name in packages.items() if name == "cockpit-daemon"]
        self.assertEqual(len(daemon), 1, "the headless daemon must be a real workspace crate")
        graph = {node["id"]: node["deps"] for node in metadata["resolve"]["nodes"]}
        pending = list(daemon)
        visited = set()
        while pending:
            identity = pending.pop()
            if identity in visited:
                continue
            visited.add(identity)
            self.assertFalse(
                packages[identity].startswith(("gtk", "gdk", "gsk")),
                f"daemon runtime reaches {packages[identity]}",
            )
            pending.extend(
                dependency["pkg"]
                for dependency in graph[identity]
                if any(kind["kind"] is None for kind in dependency["dep_kinds"])
            )

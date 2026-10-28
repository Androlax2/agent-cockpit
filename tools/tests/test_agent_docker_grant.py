import os
import subprocess
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/agent-docker.sh"


@unittest.skipIf(
    os.geteuid() == 0, "the refusals are checked before sudo; as root they would touch real folders"
)
class AgentDockerGrantTests(unittest.TestCase):
    def grant(self, *arguments):
        return subprocess.run([str(SCRIPT), "grant", *arguments], capture_output=True, text=True)

    def test_refuses_folders_outside_the_project(self):
        for folder in ("/etc", "../other", "storage/../../etc", "./storage", "storage dir"):
            with self.subTest(folder=folder):
                result = self.grant("stadeo", "1000", folder)
                self.assertEqual(result.returncode, 1)
                self.assertIn("not a plain relative folder", result.stderr)

    def test_refuses_root_and_out_of_range_container_uids(self):
        for container_uid in ("0", "65536", "sail"):
            with self.subTest(container_uid=container_uid):
                result = self.grant("stadeo", container_uid, "storage")
                self.assertEqual(result.returncode, 1)
                self.assertIn("container uid must be between 1 and 65535", result.stderr)

    def test_refuses_an_invalid_project_name(self):
        self.assertIn("invalid project name", self.grant("../stadeo", "1000", "storage").stderr)

    def test_a_valid_request_still_needs_sudo(self):
        result = self.grant("stadeo", "1000", "storage", "bootstrap/cache")
        self.assertEqual(result.returncode, 1)
        self.assertIn("run it with sudo", result.stderr)

    def test_without_a_folder_it_prints_the_usage(self):
        result = self.grant("stadeo", "1000")
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage: agent-docker.sh grant", result.stderr)


if __name__ == "__main__":
    unittest.main()

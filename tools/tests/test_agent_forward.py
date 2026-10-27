import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/agent-forward.sh"


class AgentForwardTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.forward_dir = Path(self.directory.name) / "forward"
        self.environment = {**os.environ, "AGENT_FORWARD_DIR": str(self.forward_dir)}

    def tearDown(self):
        self.directory.cleanup()

    def forward(self, *arguments):
        return subprocess.run([str(SCRIPT), *arguments], env=self.environment, capture_output=True, text=True)

    def test_writes_the_expiry_and_ports_the_launcher_reads(self):
        result = self.forward("stadeo", "30", "8088", "8025")
        self.assertEqual(result.returncode, 0, result.stderr)
        expiry, *ports = (self.forward_dir / "stadeo").read_text().split()
        self.assertEqual(ports, ["8088", "8025"])
        self.assertAlmostEqual(int(expiry), time.time() + 30 * 86400, delta=60)

    def test_replacing_a_grant_shows_the_previous_one(self):
        self.forward("stadeo", "1", "3000")
        result = self.forward("stadeo", "1", "8088")
        self.assertIn("replacing:", result.stdout)
        self.assertIn("3000", result.stdout)
        self.assertEqual((self.forward_dir / "stadeo").read_text().split()[1:], ["8088"])

    def test_refuses_invalid_input_without_writing(self):
        for arguments in (("../etc", "1", "8088"), ("stadeo", "0", "8088"), ("stadeo", "91", "8088"),
                          ("stadeo", "1", "22"), ("stadeo", "1", "80a"), ("stadeo", "1")):
            with self.subTest(arguments=arguments):
                self.assertEqual(self.forward(*arguments).returncode, 2)
        self.assertFalse(self.forward_dir.exists())


if __name__ == "__main__":
    unittest.main()

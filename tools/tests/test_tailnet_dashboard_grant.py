import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/tailnet-dashboard-grant.sh"
STATUS = {
    "Self": {"HostName": "desktop", "DNSName": "desktop.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"]},
    "Peer": {
        "key1": {"HostName": "iPhone", "DNSName": "iphone.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.2", "fd7a:115c:a1e0::2"]},
        "key2": {"HostName": "laptop", "DNSName": "laptop.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.3", "fd7a:115c:a1e0::3"]},
    },
}


class TailnetDashboardGrantTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.bin = Path(self.directory.name)
        self.status_file = self.bin / "status.json"
        self.status_file.write_text(json.dumps(STATUS))
        tailscale = self.bin / "tailscale"
        tailscale.write_text(f'#!/bin/sh\n[ "$1 $2" = "status --json" ] && exec cat {self.status_file}\nexit 1\n')
        tailscale.chmod(0o755)
        self.environment = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}"}

    def tearDown(self):
        self.directory.cleanup()

    def grants_for(self, phone, port="8443"):
        return subprocess.run([str(SCRIPT), phone, port], env=self.environment, capture_output=True, text=True)

    def test_only_the_phone_reaches_the_dashboard_port_of_this_machine(self):
        result = self.grants_for("iPhone")
        self.assertEqual(result.returncode, 0, result.stderr)
        grants = json.loads(result.stdout)["grants"]
        self.assertEqual(grants[1], {"src": ["100.64.0.2", "fd7a:115c:a1e0::2"], "dst": ["100.64.0.1", "fd7a:115c:a1e0::1"], "ip": ["tcp:8443"]})

    def test_the_grant_replacing_the_default_leaves_out_only_the_dashboard_port(self):
        grants = json.loads(self.grants_for("iPhone").stdout)["grants"]
        self.assertEqual(grants[0], {"src": ["*"], "dst": ["*"], "ip": ["tcp:1-8442", "tcp:8444-65535", "udp:*", "icmp:*", "58:*"]})

    def test_the_phone_is_found_by_its_magicdns_name_too(self):
        result = self.grants_for("iphone")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["grants"][1]["src"], ["100.64.0.2", "fd7a:115c:a1e0::2"])

    def test_an_unknown_phone_prints_no_grant(self):
        result = self.grants_for("pixel")
        self.assertEqual((result.returncode, result.stdout), (1, ""))
        self.assertIn("no device named pixel", result.stderr)

    def test_two_devices_of_that_name_print_no_grant(self):
        STATUS_TWICE = json.loads(json.dumps(STATUS))
        STATUS_TWICE["Peer"]["key3"] = {"HostName": "iPhone", "DNSName": "iphone-1.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.4"]}
        self.status_file.write_text(json.dumps(STATUS_TWICE))
        result = self.grants_for("iPhone")
        self.assertEqual((result.returncode, result.stdout), (1, ""))
        self.assertIn("2 devices named iPhone", result.stderr)


if __name__ == "__main__":
    unittest.main()

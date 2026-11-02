"""The binaries-only smoke must not hide another readiness failure."""

import importlib.util
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "verify_engine_install", Path(__file__).resolve().parents[1] / "verify-engine-install.py"
)
SMOKE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SMOKE)


class EngineReadinessTests(unittest.TestCase):
    def test_only_the_expected_absent_proxy_is_accepted(self):
        missing = "missing-prerequisite  sandbox proxy: proxy-not-configured: no setup\n"
        SMOKE.verify_engines_only_report("available  sandbox: pinned engines\n" + missing)
        for report in [
            "available  sandbox proxy: healthy\n",
            missing + "unsupported  platform: unsupported system\n",
            missing.replace("proxy-not-configured", "proxy-unhealthy"),
            missing.replace("sandbox proxy", "sandbox"),
        ]:
            with self.subTest(report=report), self.assertRaises(RuntimeError):
                SMOKE.verify_engines_only_report(report)

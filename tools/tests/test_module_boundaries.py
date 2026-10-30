import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORE_SOURCE = ROOT / "apps/crates/cockpit-core/src"


class ModuleBoundaryTests(unittest.TestCase):
    def test_background_workers_do_not_depend_on_the_app_refresh_module(self):
        for name in ("status.rs", "costs.rs", "reviews.rs"):
            with self.subTest(source=name):
                source = (CORE_SOURCE / name).read_text()
                self.assertNotRegex(source, r"\bcrate::app::refresh\b")

    def test_draft_persistence_does_not_call_app_text_formatting(self):
        source = (CORE_SOURCE / "todo_editor.rs").read_text()
        self.assertNotRegex(source, r"\bapp::(?:editor_text|parse_editor_text)\b")

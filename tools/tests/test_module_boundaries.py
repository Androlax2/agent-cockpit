import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class ModuleBoundaryTests(unittest.TestCase):
    def test_background_workers_do_not_depend_on_the_app_refresh_module(self):
        for name in ("status.rs", "costs.rs", "reviews.rs"):
            with self.subTest(source=name):
                source = (ROOT / "apps/cockpit/src" / name).read_text()
                self.assertNotRegex(source, r"\bcrate::app::refresh\b")

    def test_draft_persistence_does_not_call_app_text_formatting(self):
        source = (ROOT / "apps/cockpit/src/todo_editor.rs").read_text()
        self.assertNotRegex(source, r"\bapp::(?:editor_text|parse_editor_text)\b")

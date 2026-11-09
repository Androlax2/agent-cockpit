import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/codeql-languages.py"


def scanned_languages(*arguments, changed_paths=()):
    result = subprocess.run(
        ["python3", str(SCRIPT), *arguments],
        input="".join(f"{path}\n" for path in changed_paths),
        capture_output=True,
        text=True,
        check=True,
    )
    (line,) = result.stdout.splitlines()
    return json.loads(line.removeprefix("languages="))


class CodeqlLanguageSelectionTests(unittest.TestCase):
    def test_a_change_without_rust_scans_only_the_workflows(self):
        for path in (
            "docs/DEVELOPMENT.md",
            "README.md",
            "site/index.html",
            ".github/workflows/ci.yml",
            "tools/ci-jobs.py",
            "apps/cockpit/script.sh",
            "crates/cockpit-daemon/README.md",
            "apps/cockpit/NOTES.md",
        ):
            with self.subTest(path=path):
                self.assertEqual(scanned_languages(changed_paths=[path]), ["actions"])

    def test_a_change_to_rust_or_what_builds_it_scans_rust_too(self):
        for path in (
            "crates/cockpit-daemon/src/main.rs",
            "crates/cockpit-tui/tests/e2e/harness.rs",
            "crates/cockpit-turso/tests/fixtures/schema.sql",
            "apps/cockpit/src/lib.rs",
            "Cargo.toml",
            "Cargo.lock",
            "crates/cockpit-cli/Cargo.toml",
            "rust-toolchain.toml",
            ".cargo/config.toml",
            ".github/workflows/codeql.yml",
            "tools/codeql-languages.py",
        ):
            with self.subTest(path=path):
                self.assertEqual(scanned_languages(changed_paths=[path]), ["rust", "actions"])

    def test_one_rust_path_among_docs_scans_rust(self):
        paths = ["docs/README.md", "crates/cockpit-tasks/src/lib.rs"]
        self.assertEqual(scanned_languages(changed_paths=paths), ["rust", "actions"])

    def test_an_empty_change_set_scans_everything(self):
        self.assertEqual(scanned_languages(), ["rust", "actions"])

    def test_every_language_is_scanned_for_main_and_the_schedule(self):
        self.assertEqual(scanned_languages("--all"), ["rust", "actions"])


if __name__ == "__main__":
    unittest.main()

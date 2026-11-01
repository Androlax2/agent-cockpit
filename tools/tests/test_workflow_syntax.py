import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = sorted((ROOT / ".github/workflows").glob("*.yml"))

# `key: value` on one line, the value a plain (unquoted, not block) scalar.
PLAIN_VALUE = re.compile(r"^ *(?:- )?[A-Za-z0-9_-]+: (?![\"'|>{\[&*!])(?P<value>.+)$")


class WorkflowSyntaxTests(unittest.TestCase):
    # GitHub cannot parse a workflow whose plain scalar holds `: `, and then records a failed
    # run on every push instead of running it: prerelease.yml did so 141 times (#397).
    def test_no_plain_scalar_holds_a_colon_and_space(self):
        self.assertTrue(WORKFLOWS)
        for workflow in WORKFLOWS:
            for number, line in enumerate(workflow.read_text().splitlines(), start=1):
                if line.lstrip().startswith("#"):
                    continue
                plain = PLAIN_VALUE.match(line)
                with self.subTest(workflow=workflow.name, line=number):
                    self.assertFalse(
                        plain and ": " in plain["value"].split(" #")[0],
                        f"quote it or use a block scalar: {line.strip()}",
                    )


if __name__ == "__main__":
    unittest.main()

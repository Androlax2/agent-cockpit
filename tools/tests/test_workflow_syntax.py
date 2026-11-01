import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = sorted(
    path for path in (ROOT / ".github/workflows").iterdir() if path.suffix in (".yml", ".yaml")
)

KEY = re.compile(r"^(?P<indent> *)(?:- )?[A-Za-z0-9_-]+:(?: (?P<value>.*))?$")
BLOCK_SCALAR = re.compile(r"^[|>][-+0-9]*$")


def plain_scalars(text):
    """Yield (line number, value) for each one-line plain `key: value`, outside block scalars."""
    block_indent = None
    for number, line in enumerate(text.splitlines(), start=1):
        indent = len(line) - len(line.lstrip(" "))
        if block_indent is not None:
            if not line.strip() or indent > block_indent:
                continue
            block_indent = None
        if line.lstrip().startswith("#"):
            continue
        key = KEY.match(line)
        if key is None or key["value"] is None:
            continue
        value = key["value"].split(" #")[0].strip()
        if BLOCK_SCALAR.match(value):
            block_indent = len(key["indent"])
        elif value and value[0] not in "\"'{[&*!":
            yield number, value


class WorkflowSyntaxTests(unittest.TestCase):
    # GitHub cannot parse a workflow whose plain scalar holds `: `, and then records a failed
    # run on every push instead of running it: prerelease.yml did so 141 times (#397).
    def test_no_plain_scalar_holds_a_colon_and_space(self):
        self.assertTrue(WORKFLOWS)
        for workflow in WORKFLOWS:
            for number, value in plain_scalars(workflow.read_text()):
                with self.subTest(workflow=workflow.name, line=number):
                    self.assertNotIn(": ", value, "quote it or use a block scalar")

    def test_lines_inside_a_block_scalar_are_not_keys(self):
        text = "run: |\n  note: see: #1\nname: ok\n"
        self.assertEqual(list(plain_scalars(text)), [(3, "ok")])


if __name__ == "__main__":
    unittest.main()

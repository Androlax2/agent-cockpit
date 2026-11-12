import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = sorted(
    path for path in (ROOT / ".github/workflows").iterdir() if path.suffix in (".yml", ".yaml")
)

JOB_HEADER = re.compile(r"^  (?P<job>[A-Za-z0-9_-]+):\s*(?:#.*)?$")
JOB_KEY = re.compile(r"^    (?P<key>[A-Za-z0-9_-]+):(?: (?P<value>.*))?$")


def job_keys(text):
    """Map each job id under the top-level `jobs:` to {key: value} for its own direct keys."""
    jobs = {}
    current = None
    in_jobs = False
    for line in text.splitlines():
        if re.match(r"^\S", line):
            in_jobs = line.startswith("jobs:")
            current = None
            continue
        if not in_jobs:
            continue
        header = JOB_HEADER.match(line)
        if header:
            current = jobs.setdefault(header["job"], {})
            continue
        key = JOB_KEY.match(line)
        if key and current is not None:
            current[key["key"]] = (key["value"] or "").split(" #")[0].strip()
    return jobs


class WorkflowTimeoutTests(unittest.TestCase):
    # A hung job otherwise runs until GitHub's six-hour default (#554).
    def test_every_job_sets_a_positive_timeout_in_minutes(self):
        self.assertTrue(WORKFLOWS)
        for workflow in WORKFLOWS:
            jobs = job_keys(workflow.read_text())
            with self.subTest(workflow=workflow.name):
                self.assertTrue(jobs, "no jobs parsed: the parser no longer matches this file")
            for job, keys in jobs.items():
                if "uses" in keys:
                    continue
                with self.subTest(workflow=workflow.name, job=job):
                    self.assertIn("timeout-minutes", keys, "set timeout-minutes on the job")
                    self.assertRegex(keys["timeout-minutes"], r"^[1-9][0-9]*$")

    # GitHub rejects `timeout-minutes` on a job that calls a reusable workflow: the called
    # workflow's own jobs carry the bound, and the test above checks them.
    def test_a_reusable_workflow_call_job_sets_no_timeout(self):
        for workflow in WORKFLOWS:
            for job, keys in job_keys(workflow.read_text()).items():
                if "uses" not in keys:
                    continue
                with self.subTest(workflow=workflow.name, job=job):
                    self.assertNotIn("timeout-minutes", keys)

    def test_job_keys_reads_only_a_jobs_direct_keys(self):
        text = (
            "name: x\n"
            "on: push\n"
            "jobs:\n"
            "  bounded:\n"
            "    timeout-minutes: 5 # why\n"
            "    steps:\n"
            "      - run: echo\n"
            "        timeout-minutes: 99\n"
            "  unbounded:\n"
            "    runs-on: ubuntu-24.04\n"
            "  called:\n"
            "    uses: ./.github/workflows/other.yml\n"
        )
        self.assertEqual(
            job_keys(text),
            {
                "bounded": {"timeout-minutes": "5", "steps": ""},
                "unbounded": {"runs-on": "ubuntu-24.04"},
                "called": {"uses": "./.github/workflows/other.yml"},
            },
        )


if __name__ == "__main__":
    unittest.main()

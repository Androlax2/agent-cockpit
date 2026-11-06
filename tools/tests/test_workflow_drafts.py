import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"


def job_conditions(text):
    jobs = text.split("\njobs:\n", 1)[1]
    headers = list(re.finditer(r"^  ([\w-]+):\s*$", jobs, re.MULTILINE))
    conditions = {}
    for index, header in enumerate(headers):
        end = headers[index + 1].start() if index + 1 < len(headers) else len(jobs)
        body = jobs[header.end() : end]
        condition = re.search(r"^    if: (.*)(?:\n|$)", body, re.MULTILINE)
        expression = condition[1] if condition else "true"
        if expression == ">-":
            continuation = re.match(r"(?:      [^\n]*\n)+", body[condition.end() :])
            expression = " ".join(continuation[0].split())
        conditions[header[1]] = expression
    return conditions


def allows(expression, context):
    """Evaluate the workflows' boolean conditions, not steps or an Actions scheduler."""
    expression = expression.removeprefix("${{").removesuffix("}}").strip()
    expression = re.sub(
        r"\b(?:github|needs)\.[\w.-]+",
        lambda match: repr(context.get(match[0], "")),
        expression,
    )
    expression = expression.replace("&&", " and ").replace("||", " or ")
    expression = re.sub(r"!(?!=)", " not ", expression)
    # No run is cancelled here: `!cancelled()` holds, as in a run nobody cancelled.
    names = {"true": True, "false": False, "cancelled": lambda: False}
    return bool(eval(expression.strip(), {"__builtins__": {}}, names))


def context(event, draft=False, branch="feature/example", ref="refs/heads/main", selected="true"):
    values = {
        "github.event_name": event,
        "github.event.pull_request.draft": draft,
        "github.head_ref": branch,
        "github.event.pull_request.head.ref": branch,
        "github.ref": ref,
        "github.repository": "Androlax2/agent-cockpit",
        "github.event.workflow_run.repository.full_name": "Androlax2/agent-cockpit",
        "github.event.workflow_run.conclusion": "success",
        "github.event.workflow_run.event": "push",
        "needs.changes.outputs.greenfield": selected,
        "needs.changes.outputs.platform": selected,
        "needs.changes.outputs.docs": selected,
        "needs.plan.outputs.tag": "v0.1.0-alpha.1" if event == "push" else "",
    }
    if event != "pull_request":
        del values["github.event.pull_request.draft"]
    return values


class DraftWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.workflows = {
            path.name: path.read_text()
            for path in WORKFLOWS.iterdir()
            if path.suffix in (".yml", ".yaml")
        }
        self.pr_workflows = {
            name: text
            for name, text in self.workflows.items()
            if re.search(r"^  pull_request:", text, re.MULTILINE)
        }

    def test_ready_event_and_ordinary_pr_updates_are_subscribed(self):
        self.assertEqual(
            set(self.pr_workflows),
            {"ci.yml", "docs.yml", "release.yml", "release-plz.yml", "codeql.yml"},
        )
        for name, text in self.pr_workflows.items():
            with self.subTest(workflow=name):
                trigger = re.search(r"  pull_request:\n((?:    [^\n]*\n)*)", text)[1]
                types = re.search(r"    types: \[([^\]]+)\]", trigger)
                self.assertIsNotNone(types, "ready_for_review is not a default PR action")
                self.assertEqual(
                    {action.strip() for action in types[1].split(",")},
                    {"opened", "synchronize", "reopened", "ready_for_review"},
                )

    def test_no_job_is_eligible_on_human_or_generated_draft_prs(self):
        for name, text in self.pr_workflows.items():
            for job, expression in job_conditions(text).items():
                for branch in ("feature/example", "release-plz/" + "a" * 40):
                    with self.subTest(workflow=name, job=job, branch=branch):
                        self.assertFalse(allows(expression, context("pull_request", True, branch)))

    def test_ready_prs_execute_checks_but_never_publish(self):
        expected = {
            "codeql.yml": {"analyze"},
            "scorecard.yml": set(),
            "ci.yml": {
                "changes",
                "greenfield-lint",
                "greenfield-test",
                "greenfield",
                "greenfield-wsl2",
                "docs",
            },
            "docs.yml": {"catalog", "build", "assemble"},
            "release.yml": {
                "plan",
                "notes",
                "build",
                "install",
                "install-arch",
                "install-arch-published",
                "install-wsl2",
                "install-arch",
                "install-scoop",
                "assemble",
                "compatibility",
                "responsiveness",
                "upgrade-baseline",
                "upgrade",
                "upgrade-wsl2",
                "upgrade-sessions",
            },
            "release-plz.yml": {"check"},
        }
        for name, jobs in expected.items():
            with self.subTest(workflow=name):
                self.assertEqual(self.eligible(name, context("pull_request")), jobs)

    def test_non_pr_events_keep_their_existing_job_gates(self):
        cases = [
            (
                "ci.yml",
                "push",
                {
                    "changes",
                    "greenfield-lint",
                    "greenfield-test",
                    "greenfield",
                    "greenfield-wsl2",
                    "docs",
                },
            ),
            ("docs.yml", "push", {"catalog", "build", "assemble", "deploy"}),
            ("docs.yml", "release", {"catalog", "build", "assemble", "deploy"}),
            ("docs.yml", "workflow_run", {"catalog", "build", "assemble", "deploy"}),
            ("docs.yml", "workflow_dispatch", {"catalog", "build", "assemble", "deploy"}),
            ("release-plz.yml", "push", {"check", "proposal"}),
            ("release-plz.yml", "workflow_dispatch", {"check", "proposal"}),
            ("prerelease.yml", "workflow_dispatch", {"tag"}),
            (
                "release.yml",
                "push",
                {
                    "plan",
                    "notes",
                    "build",
                    "install",
                    "install-arch",
                    "install-wsl2",
                    "install-arch",
                    "install-scoop",
                    "assemble",
                    "draft",
                    "gate",
                    "publish",
                    "compatibility",
                    "responsiveness",
                    "upgrade-baseline",
                    "upgrade",
                    "upgrade-wsl2",
                    "upgrade-sessions",
                },
            ),
            (
                "release.yml",
                "schedule",
                {
                    "plan",
                    "notes",
                    "build",
                    "install",
                    "install-arch",
                    "install-arch-published",
                    "install-wsl2",
                    "install-arch",
                    "install-scoop",
                    "assemble",
                    "compatibility",
                    "responsiveness",
                    "upgrade-baseline",
                    "upgrade",
                    "upgrade-wsl2",
                    "upgrade-sessions",
                },
            ),
            (
                "release.yml",
                "workflow_dispatch",
                {
                    "plan",
                    "notes",
                    "build",
                    "install",
                    "install-arch",
                    "install-arch-published",
                    "install-wsl2",
                    "install-arch",
                    "install-scoop",
                    "assemble",
                    "compatibility",
                    "responsiveness",
                    "upgrade-baseline",
                    "upgrade",
                    "upgrade-wsl2",
                    "upgrade-sessions",
                },
            ),
        ]
        for name, event, jobs in cases:
            with self.subTest(workflow=name, event=event):
                self.assertEqual(self.eligible(name, context(event)), jobs)

    def test_ready_prs_still_respect_path_selection(self):
        # The required greenfield check runs alone and passes when no greenfield job is needed.
        self.assertEqual(
            self.eligible("ci.yml", context("pull_request", selected="false")),
            {"changes", "greenfield"},
        )

    def test_proposal_credentials_are_available_only_to_main_publication(self):
        workflow = self.workflows["release-plz.yml"]
        check, proposal = workflow.split("\n  proposal:\n", 1)
        self.assertRegex(proposal, r"(?m)^    environment: release-proposal$")
        self.assertNotIn("secrets.", check)
        self.assertNotRegex(workflow, r"(?m)^    (?:contents|pull-requests): write$")
        for event, ref, expected in (
            ("push", "refs/heads/main", {"check", "proposal"}),
            ("workflow_dispatch", "refs/heads/main", {"check", "proposal"}),
            ("push", "refs/tags/v0.1.0-alpha.1", {"check"}),
            ("workflow_dispatch", "refs/heads/feature/example", {"check"}),
            ("pull_request", "refs/pull/1/merge", {"check"}),
        ):
            with self.subTest(event=event, ref=ref):
                self.assertEqual(
                    self.eligible("release-plz.yml", context(event, ref=ref)), expected
                )
        generation, credential_steps = proposal.split(
            "      - uses: actions/create-github-app-token@", 1
        )
        self.assertNotIn("secrets.", generation)
        token, publication = credential_steps.split("      - name:", 1)
        self.assertIn("client-id: ${{ vars.PROPOSAL_APP_CLIENT_ID }}", token)
        self.assertIn("private-key: ${{ secrets.PROPOSAL_APP_PRIVATE_KEY }}", token)
        self.assertIn("permission-contents: read", token)
        self.assertNotIn("permission-contents: write", token)
        self.assertIn("permission-pull-requests: write", token)
        self.assertIn("owner: Androlax2", token)
        self.assertIn("repositories: agent-cockpit", token)
        self.assertIn("PROPOSAL_GIT_SSH_KEY: ${{ secrets.PROPOSAL_GIT_SSH_KEY }}", publication)
        self.assertNotIn("RELEASE_APP_", workflow)
        for step in (token, publication):
            self.assertIn("if: github.event_name == 'push' || !inputs.dry_run", step)

    def test_prerelease_tagging_requires_release_approval_and_repository_scoped_token(self):
        workflow = self.workflows["prerelease.yml"]
        tag = workflow.split("\n  tag:\n", 1)[1]
        self.assertRegex(tag, r"(?m)^    environment: release$")
        token = tag.split("      - uses: actions/create-github-app-token@", 1)[1].split(
            "      - name:", 1
        )[0]
        self.assertIn("if: ${{ !inputs.dry_run }}", token)
        self.assertIn("owner: Androlax2", token)
        self.assertIn("repositories: agent-cockpit", token)
        self.assertIn("permission-contents: write", token)
        self.assertNotIn("permission-pull-requests: write", token)
        self.assertNotIn("PROPOSAL_", tag)

    def test_docs_catalog_still_rejects_ineligible_non_pr_events(self):
        expression = job_conditions(self.workflows["docs.yml"])["catalog"]
        cases = [
            context("workflow_dispatch", ref="refs/heads/feature/example"),
            context("workflow_run") | {"github.event.workflow_run.event": "pull_request"},
            context("workflow_run") | {"github.event.workflow_run.conclusion": "failure"},
            context("workflow_run")
            | {"github.event.workflow_run.repository.full_name": "other/repo"},
        ]
        for values in cases:
            with self.subTest(context=values):
                self.assertFalse(allows(expression, values))

    def eligible(self, name, values):
        return {
            job
            for job, expression in job_conditions(self.workflows[name]).items()
            if allows(expression, values)
        }


if __name__ == "__main__":
    unittest.main()

import contextlib
import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

SPEC = importlib.util.spec_from_file_location(
    "redteam_harness", Path(__file__).resolve().parents[1] / "redteam-harness.py"
)
redteam_harness = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(redteam_harness)

SANDBOXED_SESSION = redteam_harness.SANDBOXED_SESSION
AGENT_SESSION = redteam_harness.AGENT_SESSION


class FakeGuest:
    """A guest the catalog logic drives without a VM: it records which caller path each command
    went through, so a test can prove a section-1 attack ran inside the sandbox and never
    through an owner shell (there is no owner-shell path to take)."""

    def __init__(
        self,
        *,
        required=True,
        backend_present=True,
        key_present=True,
        agent_outputs=None,
        sandbox_runs=None,
        agent_present=True,
    ):
        self.provider = "claude"
        self.agent_present = agent_present
        self.required = required
        self.backend_present = backend_present
        self.key_present = key_present
        self.agent_outputs = agent_outputs or {}
        self.sandbox_runs = sandbox_runs or {}
        self.agent_socket_calls = []
        self.sandboxed_calls = []

    def torlek_version(self):
        return "torlek 0.1.0-test"

    def sandbox_status(self):
        enforced = self.required and self.backend_present
        detail = "sandbox: enforced by torlek-sandbox" if enforced else "unsandboxed"
        return redteam_harness.SandboxStatus(
            self.required, self.backend_present, "torlek-sandbox", detail
        )

    def agent_status(self):
        detail = "2.1.319 (tested)" if self.agent_present else "not usable: no `claude` on PATH"
        return redteam_harness.AgentStatus(self.agent_present, detail)

    def section1_key_present(self):
        return self.key_present

    def run_on_agent_socket(self, command):
        self.agent_socket_calls.append(command)
        return self.agent_outputs.get(
            command,
            subprocess.CompletedProcess(command, 1, "", "torlek: refused: owner only"),
        )

    def run_in_sandboxed_session(self, command):
        self.sandboxed_calls.append(command)
        return self.sandbox_runs.get(
            command,
            redteam_harness.SandboxRun(
                exit_code=1,
                stdout="",
                stderr="",
                session="s0abcdef",
                sandbox={"backend": "torlek-sandbox", "subject": "session s0abcdef"},
            ),
        )


class ClassifyOutcomeTests(unittest.TestCase):
    def test_a_torlek_refusal_message_is_refused(self):
        self.assertEqual(
            redteam_harness.classify_outcome(1, "torlek: refused: is not a plain relative folder"),
            "refused",
        )

    def test_a_torlek_refusal_with_its_code_is_refused(self):
        self.assertEqual(
            redteam_harness.classify_outcome(
                1, "torlek: refused [conflict]: a name is taken (nothing changed)"
            ),
            "refused",
        )

    def test_the_live_owner_only_refusal_is_refused(self):
        # The exact line the real daemon returns for an owner-only verb on the agent socket.
        self.assertEqual(
            redteam_harness.classify_outcome(
                1, "torlek: not-authorized: this command is the owner's alone"
            ),
            "refused",
        )

    def test_a_blocked_network_host_is_blocked(self):
        self.assertEqual(
            redteam_harness.classify_outcome(
                1, "< HTTP/1.1 403\nCONNECT tunnel failed, response 403"
            ),
            "blocked",
        )

    def test_a_permission_denied_read_is_blocked(self):
        self.assertEqual(
            redteam_harness.classify_outcome(1, "cat: /root/secret: Permission denied"), "blocked"
        )

    def test_a_clean_exit_with_no_refusal_text_succeeded(self):
        self.assertEqual(redteam_harness.classify_outcome(0, "redteam\n"), "succeeded")

    def test_a_breach_is_not_hidden_by_permission_denied_in_its_own_output(self):
        # A section-1 read that actually succeeds (exit 0) must read "succeeded" even if the
        # leaked content happens to contain "Permission denied"; a breach must be noticed.
        self.assertEqual(
            redteam_harness.classify_outcome(0, "secret.log: 'Permission denied' was logged"),
            "succeeded",
        )

    def test_the_egress_403_is_blocked_even_on_a_clean_exit(self):
        # The network check greps for the 403, so the pipeline exits 0 when the host was refused.
        self.assertEqual(
            redteam_harness.classify_outcome(0, "* CONNECT tunnel failed, response 403"),
            "blocked",
        )

    def test_a_failure_matching_neither_pattern_is_unclear(self):
        self.assertEqual(
            redteam_harness.classify_outcome(127, "bash: nosuchcommand: command not found"),
            "unclear",
        )


class CatalogTests(unittest.TestCase):
    def test_the_shipped_catalog_has_unique_ids_and_well_formed_entries(self):
        catalog = redteam_harness.load_catalog(redteam_harness.DEFAULT_CATALOG)
        ids = [entry["id"] for entry in catalog]
        self.assertEqual(len(ids), len(set(ids)), "duplicate catalog ids")
        for entry in catalog:
            self.assertIn(entry["status"], ("automated", "manual"))
            self.assertIn(entry["category"], (1, 2, 3, 4))
            if entry["status"] == "automated":
                self.assertIn("command", entry)
                self.assertIn(entry["caller"], (SANDBOXED_SESSION, AGENT_SESSION))

    def test_each_automated_caller_matches_its_section(self):
        catalog = redteam_harness.load_catalog(redteam_harness.DEFAULT_CATALOG)
        for entry in catalog:
            if entry["status"] != "automated":
                continue
            if entry["category"] == 1:
                self.assertEqual(entry["caller"], SANDBOXED_SESSION, entry["id"])
            if entry["category"] == 2:
                self.assertEqual(entry["caller"], AGENT_SESSION, entry["id"])


class RunCatalogTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="redteam-harness-test-"))
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", str(self.root)]))
        self.results_log = self.root / "results.jsonl"

    def test_runs_only_automated_entries_and_logs_each_one(self):
        catalog = [
            {
                "id": "a",
                "category": 2,
                "title": "t",
                "status": "automated",
                "caller": AGENT_SESSION,
                "command": "torlek todo approve 1",
            },
            {"id": "b", "category": 2, "title": "t", "status": "manual"},
        ]
        guest = FakeGuest()

        records = redteam_harness.run_catalog(catalog, self.results_log, guest)

        self.assertEqual([record["id"] for record in records], ["a"])
        self.assertEqual(guest.agent_socket_calls, ["torlek todo approve 1"])
        logged = [json.loads(line) for line in self.results_log.read_text().splitlines()]
        self.assertEqual(len(logged), 1)
        self.assertEqual(logged[0]["caller"], AGENT_SESSION)
        self.assertEqual(logged[0]["torlek_version"], "torlek 0.1.0-test")

    def test_section2_commands_run_on_the_agent_socket_and_are_refused(self):
        catalog = [
            {
                "id": "2.1",
                "category": 2,
                "title": "t",
                "status": "automated",
                "caller": AGENT_SESSION,
                "command": "torlek session grant x start-sessions",
            }
        ]
        guest = FakeGuest(
            agent_outputs={
                "torlek session grant x start-sessions": subprocess.CompletedProcess(
                    args="torlek session grant x start-sessions",
                    returncode=1,
                    stdout="",
                    stderr="torlek: not-authorized: this command is the owner's alone",
                )
            }
        )

        records = redteam_harness.run_catalog(catalog, self.results_log, guest)

        self.assertEqual(records[0]["outcome"], "refused")
        self.assertEqual(records[0]["sandbox"], None)
        self.assertIn("torlek session grant x start-sessions", guest.agent_socket_calls)
        self.assertEqual(guest.sandboxed_calls, [])

    def test_a_sandbox_only_blocked_command_is_blocked_and_not_run_as_owner_shell(self):
        # A disallowed host is reachable from a plain owner shell (the host has the network), so
        # the owner-shell bug (issue #1158) would see HTTP 200 and classify it "succeeded". Run
        # inside the real sandbox the egress proxy answers 403. This proves the attack went
        # through the sandboxed-session path, not a shell, and that the 403 classifies "blocked".
        command = "curl -sv https://example.com 2>&1"
        owner_shell_output = "< HTTP/1.1 200 OK\n"
        self.assertEqual(
            redteam_harness.classify_outcome(0, owner_shell_output),
            "succeeded",
            "a plain owner shell reaches the host",
        )
        catalog = [
            {
                "id": "1.4-network-disallowed-host",
                "category": 1,
                "title": "t",
                "status": "automated",
                "caller": SANDBOXED_SESSION,
                "command": command,
            }
        ]
        guest = FakeGuest(
            sandbox_runs={
                command: redteam_harness.SandboxRun(
                    exit_code=1,
                    stdout="* CONNECT tunnel failed, response 403",
                    stderr="",
                    session="s0deadbeef12",
                    sandbox={"backend": "torlek-sandbox", "subject": "session s0deadbeef12"},
                )
            }
        )

        records = redteam_harness.run_catalog(catalog, self.results_log, guest)

        self.assertEqual(records[0]["outcome"], "blocked")
        self.assertEqual(records[0]["caller"], SANDBOXED_SESSION)
        self.assertEqual(records[0]["sandbox"]["backend"], "torlek-sandbox")
        # Ran inside the sandbox, never on the agent socket, and there is no owner-shell path.
        self.assertEqual(guest.sandboxed_calls, [command])
        self.assertEqual(guest.agent_socket_calls, [])

    def test_an_id_filter_runs_only_that_check(self):
        catalog = [
            {
                "id": "a",
                "category": 2,
                "title": "t",
                "status": "automated",
                "caller": AGENT_SESSION,
                "command": "torlek todo approve 1",
            },
            {
                "id": "b",
                "category": 2,
                "title": "t",
                "status": "automated",
                "caller": AGENT_SESSION,
                "command": "torlek todo approve 2",
            },
        ]
        guest = FakeGuest()

        records = redteam_harness.run_catalog(catalog, self.results_log, guest, only_ids={"b"})

        self.assertEqual([record["id"] for record in records], ["b"])

    def test_results_log_is_appended_to_not_overwritten(self):
        self.results_log.parent.mkdir(parents=True, exist_ok=True)
        self.results_log.write_text(json.dumps({"id": "earlier"}) + "\n")
        catalog = [
            {
                "id": "a",
                "category": 2,
                "title": "t",
                "status": "automated",
                "caller": AGENT_SESSION,
                "command": "torlek todo approve 1",
            }
        ]
        guest = FakeGuest()

        redteam_harness.run_catalog(catalog, self.results_log, guest)

        logged = [json.loads(line) for line in self.results_log.read_text().splitlines()]
        self.assertEqual([entry.get("id") for entry in logged], ["earlier", "a"])


class Section1RefusalTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="redteam-harness-test-"))
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", str(self.root)]))
        self.results_log = self.root / "results.jsonl"
        self.catalog = [
            {
                "id": "1.1",
                "category": 1,
                "title": "t",
                "status": "automated",
                "caller": SANDBOXED_SESSION,
                "command": "cat /home/arch/.bash_history 2>&1",
            }
        ]

    def assert_refuses_naming(self, guest, needle):
        with self.assertRaises(redteam_harness.Refusal) as caught:
            redteam_harness.run_catalog(self.catalog, self.results_log, guest)
        self.assertIn(needle, str(caught.exception))
        # Nothing ran: the refusal is up front, never a half-run logged as a result.
        self.assertFalse(self.results_log.exists())
        self.assertEqual(guest.sandboxed_calls, [])

    def test_refuses_when_sandbox_is_not_required(self):
        self.assert_refuses_naming(FakeGuest(required=False), "agents.sandbox")

    def test_refuses_when_the_backend_is_missing(self):
        self.assert_refuses_naming(FakeGuest(backend_present=False), "backend is missing")

    def test_refuses_naming_the_missing_agent_program_apart_from_the_credential(self):
        guest = FakeGuest(agent_present=False, key_present=False)
        self.assert_refuses_naming(guest, "does not list claude as usable")

    def test_refuses_when_no_provider_key_is_present(self):
        self.assert_refuses_naming(FakeGuest(key_present=False), "provider credential")

    def test_section2_still_runs_when_section1_is_refused(self):
        # A full run with no key: section 2 needs none, so it runs and is logged; section 1 is
        # skipped and the refusal is raised (loud, non-zero), carrying what did run.
        catalog = self.catalog + [
            {
                "id": "2.1",
                "category": 2,
                "title": "t",
                "status": "automated",
                "caller": AGENT_SESSION,
                "command": "torlek todo approve 1",
            }
        ]
        guest = FakeGuest(key_present=False)

        with self.assertRaises(redteam_harness.Refusal) as caught:
            redteam_harness.run_catalog(catalog, self.results_log, guest)

        # Section 1 never ran; section 2 did and was logged.
        self.assertEqual(guest.sandboxed_calls, [])
        self.assertEqual(guest.agent_socket_calls, ["torlek todo approve 1"])
        logged = [json.loads(line) for line in self.results_log.read_text().splitlines()]
        self.assertEqual([entry["id"] for entry in logged], ["2.1"])
        # The refusal carries the section-2 records so the caller can still summarize them.
        self.assertEqual([record["id"] for record in caught.exception.records], ["2.1"])


class NotRunTests(unittest.TestCase):
    """#1175: a section-1 check whose attack never ran in the sandbox is `not-run`, never a
    refusal or a block that reads as "the sandbox held"."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="redteam-harness-test-"))
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", str(self.root)]))
        self.results_log = self.root / "results.jsonl"
        self.command = "cat /home/arch/.bash_history 2>&1"
        self.catalog = [
            {
                "id": "1.1",
                "category": 1,
                "title": "t",
                "status": "automated",
                "caller": SANDBOXED_SESSION,
                "command": self.command,
            }
        ]

    def start_refused_by_the_daemon(self):
        """The run of 2026-11-08: the daemon rejected the provider credential, so `session
        new` failed with a `torlek: refused:` line and no session started."""
        stderr = (
            "torlek: refused: the claude provider's credential cannot be used, so its "
            "sandboxed sessions do not launch"
        )
        start_failure = subprocess.CompletedProcess("session new", 1, "", stderr)

        class StartFails(redteam_harness.VmGuest):
            def _ssh(self, remote_command, timeout=60):
                if "torlek session new" in remote_command:
                    return start_failure
                return subprocess.CompletedProcess(remote_command, 0, "", "")

        return StartFails().run_in_sandboxed_session(self.command)

    def test_a_failed_session_start_logs_not_run_not_refused(self):
        guest = FakeGuest(sandbox_runs={self.command: self.start_refused_by_the_daemon()})

        records = redteam_harness.run_catalog(self.catalog, self.results_log, guest)

        logged = [json.loads(line) for line in self.results_log.read_text().splitlines()]
        self.assertEqual([entry["outcome"] for entry in logged], ["not-run"])
        self.assertIn("did not start", logged[0]["not_run_reason"])
        self.assertIn("credential cannot be used", logged[0]["not_run_reason"])
        self.assertEqual(records[0]["outcome"], "not-run")

    def test_a_session_that_wrote_no_result_is_not_run(self):
        self.assertEqual(redteam_harness.drop_not_run_reason(""), "the session wrote no result")

    def test_a_result_without_its_exit_line_is_not_run(self):
        reason = redteam_harness.drop_not_run_reason("torlek: refused: something\n")

        self.assertIn("__REDTEAM_EXIT__", reason)

    def test_a_result_with_its_exit_line_ran(self):
        self.assertIsNone(
            redteam_harness.drop_not_run_reason("Permission denied\n__REDTEAM_EXIT__=1\n")
        )

    def test_the_summary_counts_not_run_apart_and_says_it_is_no_pass(self):
        records = [
            {"id": "1.1", "outcome": "not-run", "command": "c", "not_run_reason": "why"},
            {"id": "2.1", "outcome": "refused", "command": "d"},
        ]
        printed = io.StringIO()

        with contextlib.redirect_stdout(printed):
            redteam_harness.print_summary(records)

        lines = printed.getvalue().splitlines()
        self.assertIn("1.1: not-run (c): why", lines)
        self.assertIn("not-run: 1", lines)
        self.assertIn("refused: 1", lines)
        self.assertIn("not-run checks tested nothing: none of them is a pass", lines)

    def test_a_run_with_a_not_run_check_exits_non_zero(self):
        records = [
            {"id": "1.1", "outcome": "not-run"},
            {"id": "2.1", "outcome": "refused"},
        ]

        self.assertEqual(redteam_harness.exit_status(records), 1)

    def test_a_run_with_a_succeeded_check_exits_non_zero(self):
        for section_check_id in ("1.1", "2.1"):
            with self.subTest(check=section_check_id):
                records = [
                    {"id": section_check_id, "outcome": "succeeded"},
                    {"id": "2.2", "outcome": "refused"},
                ]

                self.assertEqual(redteam_harness.exit_status(records), 1)

    def test_the_summary_names_each_succeeded_check_as_a_finding(self):
        records = [
            {"id": "1.1", "outcome": "succeeded", "command": "cat /etc/shadow"},
            {"id": "2.1", "outcome": "succeeded", "command": "torlek project add"},
            {"id": "2.2", "outcome": "refused", "command": "d"},
        ]
        printed = io.StringIO()

        with contextlib.redirect_stdout(printed):
            redteam_harness.print_summary(records)

        lines = printed.getvalue().splitlines()
        self.assertIn("FINDING to file: 1.1 succeeded, read its raw output (cat /etc/shadow)", lines)
        self.assertIn("FINDING to file: 2.1 succeeded, read its raw output (torlek project add)", lines)
        self.assertFalse(any(line.startswith("FINDING") and "2.2" in line for line in lines))

    def test_a_run_where_every_check_ran_exits_zero(self):
        records = [{"id": "1.1", "outcome": "blocked"}, {"id": "2.1", "outcome": "refused"}]

        self.assertEqual(redteam_harness.exit_status(records), 0)


class DropAndSessionParsingTests(unittest.TestCase):
    """The pure parsing of the section-1 runner's drop file and session id (no ssh)."""

    def setUp(self):
        self.guest = redteam_harness.VmGuest()

    def test_the_exit_sentinel_wins_over_a_stray_exit_line_in_attack_output(self):
        text = "first line\nexit=0\n__REDTEAM_EXIT__=7\n"
        exit_code, body = self.guest._parse_drop(text)
        self.assertEqual(exit_code, 7)
        # The attack's own `exit=0` stays in the body; only the sentinel sets the status.
        self.assertIn("exit=0", body)
        self.assertNotIn("__REDTEAM_EXIT__", body)

    def test_the_last_sentinel_wins(self):
        exit_code, _ = self.guest._parse_drop("__REDTEAM_EXIT__=1\nmore\n__REDTEAM_EXIT__=0\n")
        self.assertEqual(exit_code, 0)

    def test_a_missing_sentinel_is_an_unknown_nonzero_exit(self):
        exit_code, body = self.guest._parse_drop("some output with no sentinel\n")
        self.assertNotEqual(exit_code, 0)
        self.assertIn("some output", body)

    def test_a_session_id_is_found_after_a_banner_line(self):
        text = "starting session...\ns0123456789ab\tlaunching\tclaude\t-\t/home/redteam/p\n"
        self.assertEqual(self.guest._parse_session_id(text), "s0123456789ab")

    def test_no_session_id_returns_none(self):
        self.assertIsNone(self.guest._parse_session_id("torlek: error: could not start\n"))


DOCTOR_REPORT = """\
available             sandbox: sandboxed: every agent runs in torlek-sandbox with bubblewrap 0.13.1 and Landlock ABI 9
available             provider claude: 2.1.319 (tested); reports resume, submit, events
needs-authorization   provider claude sign-in: not signed in (tested on 2.1.319)
needs-authorization   provider claude sandboxed sign-in: none: a sandboxed claude session never reads your own sign-in
available             provider codex: not usable, optional: no `codex` on PATH
"""


class ReportedGuest(redteam_harness.VmGuest):
    """The real guest's parsing of `torlek doctor`, fed a recorded report instead of ssh."""

    def __init__(self, report, config='[agents]\nsandbox = "required"\n'):
        super().__init__()
        self.report = report
        self.config = config

    def _ssh(self, remote_command, timeout=60):
        output = self.report if "torlek doctor" in remote_command else self.config
        return subprocess.CompletedProcess(remote_command, 0, output, "")


class VmGuestDoctorParsingTests(unittest.TestCase):
    def test_reads_the_backend_from_the_sandbox_line(self):
        status = ReportedGuest(DOCTOR_REPORT).sandbox_status()
        self.assertTrue(status.required)
        self.assertTrue(status.backend_present)
        self.assertEqual(status.backend, "torlek-sandbox")

    def test_reports_no_backend_when_the_sandbox_line_is_not_sandboxed(self):
        report = "available             sandbox: off: agents run as you\n"
        status = ReportedGuest(report, config="").sandbox_status()
        self.assertFalse(status.required)
        self.assertFalse(status.backend_present)

    def test_lists_the_agent_from_its_provider_line_not_its_sign_in_lines(self):
        agent = ReportedGuest(DOCTOR_REPORT).agent_status()
        self.assertTrue(agent.present)
        self.assertTrue(agent.detail.startswith("2.1.319"))

    def test_an_agent_with_no_provider_line_is_absent(self):
        agent = ReportedGuest("available             sandbox: sandboxed: every agent runs in x\n").agent_status()
        self.assertFalse(agent.present)

    def test_an_agent_not_usable_is_absent(self):
        report = "available             provider claude: not usable: no `claude` on PATH\n"
        self.assertFalse(ReportedGuest(report).agent_status().present)


if __name__ == "__main__":
    unittest.main()

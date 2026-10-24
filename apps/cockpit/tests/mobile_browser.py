"""Real-browser phone regressions against an isolated cockpit-remote server.

After `make cockpit`, run with a Python environment containing Playwright:
    python apps/cockpit/tests/mobile_browser.py -v
Chromium must be installed with `playwright install chromium` beforehand.
No commands, sessions, pairing secrets or todo items from the host are used.
"""

import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

from playwright.sync_api import expect, sync_playwright


REPO = Path(__file__).resolve().parents[3]
BINARY = REPO / "apps/target/release/cockpit-remote"
SESSION = "aaaaaaaa-0000-4000-8000-000000000000"


class MobileBrowserTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.playwright = sync_playwright().start()
        cls.browser = cls.playwright.chromium.launch(headless=True)

    @classmethod
    def tearDownClass(cls):
        cls.browser.close()
        cls.playwright.stop()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cockpit-mobile-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.runtime = self.root / "runtime"
        self.state = self.root / "state"
        self.home = self.root / "home"
        self.shims = self.root / "shims"
        for folder in [self.runtime, self.state, self.home, self.shims]:
            folder.mkdir()
        self.project = self.home / "IdeaProjects/shop"
        self.project.mkdir(parents=True)
        self.environment = dict(os.environ, HOME=str(self.home),
                                XDG_RUNTIME_DIR=str(self.runtime), XDG_STATE_HOME=str(self.state),
                                COCKPIT_STATE_HOME=str(self.state),
                                COCKPIT_REMOTE_DIR=str(self.state / "cockpit-remote"),
                                COCKPIT_REMOTE_ADDRESS="127.0.0.1:0", CLAUDE_AUDIT_JOURNAL="off",
                                PATH=f"{self.shims}:{os.environ['PATH']}")
        self.shim("tailscale", "echo '{\"Self\":{\"DNSName\":\"test.ts.net\"}}'")
        self.shim("qrencode", "exit 0")
        self.shim("cockpit", 'printf "%s\\n" "$*" >> "$XDG_RUNTIME_DIR/actions"')
        self.shim("tmux", 'if [ "$1" = capture-pane ]; then cat "$XDG_RUNTIME_DIR/pane.txt"; '
                          'else printf "%s\\n" "$*" >> "$XDG_RUNTIME_DIR/keys"; fi')
        self.shim("gh", "echo '[]'")
        self.snapshot()
        transcripts = self.home / ".claude/projects/shop"
        transcripts.mkdir(parents=True)
        (transcripts / f"{SESSION}.jsonl").write_text(json.dumps({"uuid": "user1", "type": "user", "message": {"content": "Fix checkout"}}) + "\n")
        (self.runtime / "pane.txt").write_text("\x1b[32mReady\x1b[0m\n❯ \n")
        self.server = subprocess.Popen([str(BINARY), "serve"], env=self.environment,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(self.stop_server)
        address = self.server.stdout.readline().strip().rsplit(" ", 1)[-1]
        self.url = f"http://{address}"
        self.context = self.browser.new_context(viewport={"width": 390, "height": 844},
                                                is_mobile=True, has_touch=True, service_workers="block")
        self.addCleanup(self.context.close)
        self.page = self.context.new_page()
        self.page.set_default_timeout(2500)
        self.errors = []
        self.page.on("pageerror", lambda error: self.errors.append(str(error)))

    def stop_server(self):
        self.server.terminate()
        self.server.communicate(timeout=5)

    def shim(self, name, body):
        script = self.shims / name
        script.write_text(f"#!/bin/sh\n{body}\n")
        script.chmod(0o755)

    def snapshot(self, sessions=None):
        if sessions is None:
            sessions = [{"session": SESSION, "project": "shop", "state": "waiting",
                         "task": "Fix checkout", "tmux_pane": "%4"}]
        path = self.runtime / "cockpit.json"
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps({"sessions": sessions}))
        temporary.replace(path)

    def pairing(self):
        output = subprocess.check_output([str(BINARY), "pair"], env=self.environment, text=True)
        digits = re.search(r"within 2 minutes: (\d{3}) (\d{3})", output)
        return re.search(r"/pair/([a-f0-9]{32})", output)[1], digits[1] + digits[2]

    def open_paired(self, suffix=""):
        code, _ = self.pairing()
        response = self.context.request.get(f"{self.url}/pair/{code}", max_redirects=0)
        self.assertEqual(response.status, 303)
        self.page.goto(self.url + "/" + suffix)
        self.page.wait_for_load_state("networkidle")
        if not suffix:
            expect(self.page.locator("#list .card")).to_have_count(1)

    def open_detail(self):
        self.open_paired()
        self.page.locator("#list .card").tap()
        self.page.get_by_role("button", name="More", exact=True).tap()
        self.page.locator("#terminal-details > summary").tap()
        expect(self.page.locator("#screen")).to_contain_text("Ready")

    def refresh(self):
        self.page.evaluate("refresh()")

    def test_unpaired_phone_shows_only_pairing(self):
        self.page.goto(self.url)
        self.page.wait_for_load_state("networkidle")
        expect(self.page.locator("#pairing")).to_be_visible()
        for section in ["#tabs", "#list", "#starter", "#phone", "#approvals"]:
            with self.subTest(section=section):
                expect(self.page.locator(section)).to_be_hidden()

    def test_six_digit_pairing_opens_sessions(self):
        _, digits = self.pairing()
        self.page.goto(self.url)
        self.page.get_by_placeholder("123 456").fill(digits)
        self.page.get_by_role("button", name="Pair", exact=True).tap()
        expect(self.page.locator("#list .card")).to_have_count(1)
        expect(self.page.locator("#pairing")).to_be_hidden()

    def test_notification_link_loads_chat_with_optional_terminal(self):
        self.open_paired(f"#session={SESSION}")
        expect(self.page.locator("#conversation")).to_contain_text("Fix checkout")
        expect(self.page.locator("#terminal-details")).not_to_have_attribute("open", "")
        self.page.get_by_role("button", name="More", exact=True).tap()
        self.page.locator("#terminal-details > summary").tap()
        expect(self.page.locator("#screen")).to_contain_text("Ready")

    def test_back_from_detail_restores_new_session(self):
        self.open_detail()
        self.page.get_by_role("button", name="‹ Sessions").tap()
        expect(self.page.get_by_role("button", name="+ New session")).to_be_visible()

    def test_disappearing_session_returns_to_the_list(self):
        self.open_detail()
        self.snapshot([])
        self.refresh()
        expect(self.page.locator("#detail")).to_be_hidden()
        expect(self.page.locator("#list")).to_be_visible()
        expect(self.page.get_by_role("button", name="+ New session")).to_be_visible()

    def test_disappearing_session_clears_its_draft(self):
        self.open_detail()
        self.page.locator("#detail textarea").fill("Private draft for shop")
        self.snapshot([])
        self.refresh()
        self.snapshot()
        self.refresh()
        self.page.locator("#list .card").tap()
        expect(self.page.locator("#detail textarea")).to_have_value("")

    def test_polling_preserves_a_draft(self):
        self.open_detail()
        self.page.locator("#detail textarea").fill("Keep my draft")
        self.snapshot([{"session": SESSION, "project": "shop", "state": "working",
                        "task": "Changed task", "tmux_pane": "%4"}])
        self.refresh()
        expect(self.page.locator("#detail textarea")).to_have_value("Keep my draft")

    def test_a_message_network_failure_is_visible(self):
        self.open_detail()
        self.page.route("**/api/message/*", lambda route: route.abort())
        self.page.locator("#detail textarea").fill("Hello")
        self.page.get_by_role("button", name="Send", exact=True).tap()
        expect(self.page.locator("#send-note")).to_contain_text("Failed")
        expect(self.page.locator("#detail textarea")).to_have_value("Hello")
        self.assertEqual(self.errors, [])

    def test_approval_feedback_survives_polling(self):
        runs = self.runtime / "claude-runs"
        runs.mkdir()
        (runs / "0123456789ab.asking").write_text(json.dumps({"id": "0123456789ab", "project": "shop",
                                                                  "cwd": str(self.project), "command": "echo hello"}))
        self.open_paired()
        self.page.get_by_role("button", name="Decline", exact=True).tap()
        expect(self.page.locator("#approvals .note")).to_contain_text("Declined")
        self.refresh()
        expect(self.page.locator("#approvals .note")).to_contain_text("Declined")

    def test_confirmation_does_not_lose_the_terminal(self):
        runs = self.runtime / "claude-runs"
        runs.mkdir()
        (runs / "0123456789ab.asking").write_text(json.dumps({"id": "0123456789ab", "project": "shop", "session": SESSION,
                                                                  "cwd": str(self.project), "command": "echo hello"}))
        self.open_detail()
        self.page.get_by_role("button", name="Run", exact=True).tap()
        expect(self.page.locator("#screen")).to_contain_text("Ready")
        self.assertFalse((runs / "0123456789ab.remote").exists())
        with self.page.expect_response("**/api/decide/0123456789ab"):
            self.page.get_by_role("button", name="Tap again to run").tap()
        self.assertEqual((runs / "0123456789ab.remote").read_text(), "y")
        self.refresh()
        expect(self.page.locator("#chat-actions .note")).to_contain_text("Approved")

    def test_phone_widths_do_not_scroll_sideways(self):
        self.snapshot([{"session": SESSION, "project": "long-project-" * 12, "state": "waiting", "tmux_pane": "%4"}])
        self.open_paired()
        for width in [320, 375, 390, 430, 768]:
            with self.subTest(width=width):
                self.page.set_viewport_size({"width": width, "height": 844})
                self.assertLessEqual(self.page.evaluate("document.documentElement.scrollWidth"), width)

    def test_all_work_tabs_load(self):
        self.open_paired()
        for label, heading in [("Todo", "New task"), ("Pull requests", "No open pull request."), ("Usage", "Cost estimate (API prices)")]:
            with self.subTest(tab=label):
                self.page.get_by_role("button", name=label, exact=True).tap()
                expect(self.page.locator("#work")).to_contain_text(heading)
        self.assertEqual(self.errors, [])

    def test_starting_a_session_uses_the_selected_agent(self):
        self.open_paired()
        self.page.get_by_role("button", name="+ New session").tap()
        self.page.locator("#starter select").nth(1).select_option("opencode")
        self.page.get_by_placeholder("What should the session do?").fill("Review checkout")
        self.page.get_by_role("button", name="Start", exact=True).tap()
        expect(self.page.locator("#starter .note")).not_to_have_text("…")
        self.assertIn("--agent opencode parallel", (self.runtime / "actions").read_text())

    def test_revoked_pairing_hides_session_details(self):
        self.open_detail()
        subprocess.check_call([str(BINARY), "unpair"], env=self.environment, stdout=subprocess.DEVNULL)
        self.refresh()
        expect(self.page.locator("#pairing")).to_be_visible()
        expect(self.page.locator("#detail")).to_be_hidden()

    def test_a_slow_tab_cannot_replace_the_current_tab(self):
        self.open_paired()
        pending = []
        self.page.route("**/api/todos", lambda route: pending.append(route))
        with self.page.expect_request("**/api/todos"):
            self.page.get_by_role("button", name="Todo", exact=True).tap()
        self.page.get_by_role("button", name="Usage", exact=True).tap()
        expect(self.page.locator("#work")).to_contain_text("Cost estimate (API prices)")
        pending[0].fulfill(json=[])
        self.page.wait_for_load_state("networkidle")
        expect(self.page.locator("#work")).to_contain_text("Cost estimate (API prices)")
        expect(self.page.get_by_placeholder("A new item")).to_have_count(0)

    def test_switching_sessions_does_not_copy_a_private_draft(self):
        self.open_detail()
        self.page.locator("#detail textarea").fill("Only for shop")
        self.page.get_by_role("button", name="‹ Sessions").tap()
        second = "bbbbbbbb-0000-4000-8000-000000000000"
        self.snapshot([{"session": second, "project": "other", "state": "waiting", "tmux_pane": "%5"}])
        self.refresh()
        self.page.locator("#list .card").tap()
        expect(self.page.locator("#detail textarea")).to_have_value("")

    def test_double_tapping_start_launches_only_one_session(self):
        self.open_paired()
        self.page.get_by_role("button", name="+ New session").tap()
        self.page.get_by_placeholder("What should the session do?").fill("Review checkout")
        pending = []
        self.page.route("**/api/sessions", lambda route: pending.append(route))
        self.page.get_by_role("button", name="Start", exact=True).tap()
        self.page.get_by_role("button", name="Start", exact=True).tap()
        self.page.wait_for_timeout(100)
        count = len(pending)
        for route in pending:
            route.fulfill(json={"ok": True, "message": "started"})
        self.assertEqual(count, 1)

    def test_a_screen_network_failure_is_visible(self):
        self.open_detail()
        self.page.route("**/api/screen/*", lambda route: route.abort())
        self.page.evaluate("loadScreen()")
        expect(self.page.locator("#screen-note")).to_contain_text("Failed")
        expect(self.page.locator("#screen")).to_contain_text("Ready")
        self.assertEqual(self.errors, [])

    def test_an_expired_pairing_is_reported_when_sending(self):
        self.open_detail()
        subprocess.check_call([str(BINARY), "unpair"], env=self.environment, stdout=subprocess.DEVNULL)
        self.page.locator("#detail textarea").fill("Keep this message")
        self.page.get_by_role("button", name="Send", exact=True).tap()
        expect(self.page.locator("#pairing")).to_be_visible()
        expect(self.page.locator("#pairing .note")).to_contain_text("not paired")

    def test_todo_creation_reaches_the_update_queue(self):
        self.open_paired()
        self.page.get_by_role("button", name="Todo", exact=True).tap()
        self.page.get_by_role("button", name="+ New task", exact=True).tap()
        self.page.get_by_placeholder("A new item").fill("Fix cart")
        self.page.get_by_label("Kind", exact=True).select_option("bug")
        self.page.get_by_placeholder("Notes (optional)").fill("Phone regression")
        with self.page.expect_response("**/api/todos"):
            self.page.get_by_role("button", name="Add", exact=True).tap()
        update = json.loads((self.runtime / "cockpit-todo-updates.jsonl").read_text().splitlines()[0])
        self.assertEqual((update["text"], update["category"]), ("Fix cart", "bug"))
        expect(self.page.get_by_placeholder("A new item")).to_have_value("")

    def todo_items(self, items):
        todo = self.state / "cockpit/todo.json"
        todo.parent.mkdir(parents=True)
        todo.write_text(json.dumps({"items": [dict({"cwd": str(self.project), "status": "todo", "created": 1}, **item) for item in items]}))

    def test_marking_a_todo_done_takes_two_taps(self):
        self.todo_items([{"id": "aaaa1111", "project": "shop", "text": "Fix cart"}])
        self.open_paired()
        self.page.get_by_role("button", name="Todo", exact=True).tap()
        self.page.locator("#work .todo-card summary").tap()
        self.page.get_by_role("button", name="Mark done", exact=True).tap()
        self.assertFalse((self.runtime / "cockpit-todo-updates.jsonl").exists())
        with self.page.expect_response("**/api/todos/aaaa1111/done"):
            self.page.get_by_role("button", name="Tap again: done", exact=True).tap()
        update = json.loads((self.runtime / "cockpit-todo-updates.jsonl").read_text().splitlines()[0])
        self.assertEqual((update["action"], update["id"]), ("done", "aaaa1111"))

    def test_the_project_filter_shows_only_its_items(self):
        other = self.home / "IdeaProjects/blog"
        other.mkdir(parents=True)
        self.todo_items([{"id": "aaaa1111", "project": "shop", "text": "Fix cart"},
                         {"id": "bbbb2222", "project": "blog", "cwd": str(other), "text": "Write post"}])
        self.open_paired()
        self.page.get_by_role("button", name="Todo", exact=True).tap()
        expect(self.page.locator("#work .todo-card")).to_have_count(2)
        self.page.get_by_role("button", name=re.compile("^blog")).tap()
        expect(self.page.locator("#work .todo-card")).to_have_count(1)
        expect(self.page.locator("#work .todo-card")).to_contain_text("Write post")

    def test_items_in_progress_come_first_and_cannot_start_again(self):
        self.todo_items([{"id": "aaaa1111", "project": "shop", "text": "Fix cart"},
                         {"id": "bbbb2222", "project": "shop", "text": "Ship tax", "status": "doing"}])
        self.open_paired()
        self.page.get_by_role("button", name="Todo", exact=True).tap()
        expect(self.page.locator("#work h2").first).to_have_text("In progress · 1")
        self.page.locator("#work .todo-card summary").first.tap()
        expect(self.page.locator("#work .todo-card").first.get_by_role("button", name="Start in a new session")).to_have_count(0)

    def test_a_todo_starts_with_the_selected_agent(self):
        todo = self.state / "cockpit/todo.json"
        todo.parent.mkdir(parents=True)
        todo.write_text(json.dumps({"items": [{"id": "aaaa1111", "project": "shop", "cwd": str(self.project),
                                                "text": "Fix cart", "status": "todo", "created": 1}]}))
        self.open_paired()
        self.page.get_by_role("button", name="Todo", exact=True).tap()
        self.page.locator("#work .card").tap()
        self.page.locator("#work .card select").select_option("opencode")
        with self.page.expect_response("**/api/todos/aaaa1111/start"):
            self.page.get_by_role("button", name="Start in a new session").tap()
        self.assertIn("--agent opencode parallel", (self.runtime / "actions").read_text())

    def test_terminal_updates_preserve_colours_and_escape_markup(self):
        self.open_detail()
        (self.runtime / "pane.txt").write_text("\x1b[32mUpdated <img src=x onerror=alert(1)>\x1b[0m\n")
        self.page.evaluate("loadScreen()")
        expect(self.page.locator("#screen")).to_contain_text("Updated <img")
        expect(self.page.locator("#screen img")).to_have_count(0)
        self.assertEqual(self.page.locator("#screen span").first.evaluate("node => node.style.color"), "rgb(158, 206, 106)")

    def test_failed_project_loading_shows_an_error(self):
        self.open_paired()
        self.page.route("**/api/projects", lambda route: route.abort())
        self.page.get_by_role("button", name="+ New session").tap()
        expect(self.page.locator("#starter")).to_contain_text("Failed")
        self.assertEqual(self.errors, [])

    def test_a_sent_message_clears_the_input(self):
        self.open_detail()
        self.page.locator("#detail textarea").fill("Review cart")
        self.page.get_by_role("button", name="Send", exact=True).tap()
        expect(self.page.locator("#send-note")).to_have_text("sent")
        expect(self.page.locator("#detail textarea")).to_have_value("")
        self.assertIn(f"say {SESSION} Review cart", (self.runtime / "actions").read_text())

    def test_a_working_session_queues_a_message(self):
        self.snapshot([{"session": SESSION, "project": "shop", "state": "working", "tmux_pane": "%4"}])
        self.open_detail()
        self.page.locator("#detail textarea").fill("Then check tax")
        self.page.get_by_role("button", name="Queue", exact=True).tap()
        expect(self.page.locator("#send-note")).to_contain_text("queued")
        expect(self.page.locator("#detail textarea")).to_have_value("")

    def test_a_message_keeps_its_paragraphs_and_indentation(self):
        self.open_detail()
        self.page.locator("#detail textarea").fill("First line\n\n  Second line")
        expect(self.page.locator("#reply-hint")).to_have_count(0)
        self.page.get_by_role("button", name="Send", exact=True).tap()
        expect(self.page.locator("#send-note")).to_contain_text("sent")
        self.assertIn(f"say {SESSION} First line\n\n  Second line", (self.runtime / "actions").read_text())

    def test_interrupt_requires_two_taps(self):
        self.snapshot([{"session": SESSION, "project": "shop", "state": "working", "tmux_pane": "%4"}])
        self.open_detail()
        self.page.get_by_role("button", name="Interrupt", exact=True).tap()
        self.assertFalse((self.runtime / "keys").exists())
        self.page.get_by_role("button", name="Tap again to interrupt").tap()
        expect(self.page.locator("#session-controls > .note")).to_have_text("interrupted")
        self.assertIn("send-keys -t %4 Escape", (self.runtime / "keys").read_text())

    def test_yes_and_no_answer_the_real_dialog(self):
        self.snapshot([{"session": SESSION, "project": "shop", "state": "needs-you", "tmux_pane": "%4"}])
        (self.runtime / "pane.txt").write_text("Bash command\nDo you want to proceed?\n❯ 1. Yes\n  2. No\nReady\n")
        self.open_detail()
        for label, key, result in [("Yes", "Enter", "approved"), ("No", "Escape", "declined")]:
            with self.subTest(answer=label):
                self.page.get_by_role("button", name=label, exact=True).tap()
                expect(self.page.locator("#chat-actions .note")).to_have_text(result)
                self.assertIn(f"send-keys -t %4 {key}", (self.runtime / "keys").read_text())

    def test_opencode_permission_is_answered_from_the_phone(self):
        self.snapshot([{"session": SESSION, "project": "shop", "agent": "opencode",
                        "state": "needs-you", "tmux_pane": "%4"}])
        (self.runtime / "pane.txt").write_text("┃ △ Permission required\n┃   $ npm test\n┃   Allow once   Always allow   Reject\n")
        self.open_detail()
        expect(self.page.get_by_role("button", name="Remote Control: off", exact=True)).to_have_count(0)
        self.page.get_by_role("button", name="Yes", exact=True).tap()
        expect(self.page.locator("#chat-actions .note")).to_have_text("approved")
        self.assertIn("send-keys -t %4 Enter", (self.runtime / "keys").read_text())

    def test_codex_permission_must_be_answered_on_the_computer(self):
        self.snapshot([{"session": SESSION, "project": "shop", "agent": "codex",
                        "state": "needs-you", "tmux_pane": "%4"}])
        self.open_detail()
        expect(self.page.get_by_role("button", name="Yes", exact=True)).to_have_count(0)
        expect(self.page.locator("#detail")).to_contain_text("must be answered on the computer")

    def test_remote_control_reaches_the_session_manager(self):
        self.open_detail()
        self.page.get_by_role("button", name="Remote Control: off", exact=True).tap()
        expect(self.page.locator("#session-controls > .note")).to_have_text("Remote Control on")
        self.assertIn(f"remote {SESSION}", (self.runtime / "actions").read_text())

    def test_invalid_pairing_code_shows_a_useful_error(self):
        self.page.goto(self.url)
        self.page.get_by_placeholder("123 456").fill("not-a-code")
        self.page.get_by_role("button", name="Pair", exact=True).tap()
        expect(self.page.locator("#pairing .note")).to_contain_text("6 digits")

    def test_pairing_network_failures_show_an_error(self):
        _, digits = self.pairing()
        self.page.goto(self.url)
        self.page.route("**/pair-code", lambda route: route.abort())
        self.page.get_by_placeholder("123 456").fill(digits)
        self.page.get_by_role("button", name="Pair", exact=True).tap()
        expect(self.page.locator("#pairing .note")).to_contain_text("Failed")
        self.assertEqual(self.errors, [])

    def test_an_ended_approval_disappears_after_its_action(self):
        runs = self.runtime / "claude-runs"
        runs.mkdir()
        (runs / "0123456789ab.asking").write_text(json.dumps({"id": "0123456789ab", "project": "shop",
                                                                  "cwd": str(self.project), "command": "echo hello"}))
        self.open_paired()
        self.shim("tmux", 'if [ "$1" = has-session ]; then exit 1; fi')
        self.page.get_by_role("button", name="Decline", exact=True).tap()
        expect(self.page.locator("#approvals .card")).to_have_count(0)
        self.assertFalse((runs / "0123456789ab.remote").exists())

    def test_session_cards_are_keyboard_accessible(self):
        self.open_paired()
        session = self.page.get_by_role("button", name=re.compile("shop.*Ready", re.S))
        session.focus()
        self.page.keyboard.press("Enter")
        expect(self.page.locator("#conversation")).to_contain_text("Fix checkout")

    def test_new_session_fields_have_visible_labels(self):
        self.open_paired()
        self.page.get_by_role("button", name="+ New session").tap()
        expect(self.page.get_by_label("Project", exact=True)).to_be_visible()
        expect(self.page.get_by_label("Agent", exact=True)).to_be_visible()
        expect(self.page.get_by_label("First message", exact=True)).to_be_visible()

    def test_navigation_marks_the_current_section(self):
        self.open_paired()
        self.page.get_by_role("button", name="Todo", exact=True).tap()
        expect(self.page.get_by_role("button", name="Todo", exact=True)).to_have_attribute("aria-current", "page")
        expect(self.page.get_by_role("button", name="Sessions", exact=True)).not_to_have_attribute("aria-current", "page")

    def test_failed_work_tab_can_be_retried(self):
        self.open_paired()
        self.page.route("**/api/usage", lambda route: route.abort())
        self.page.get_by_role("button", name="Usage", exact=True).tap()
        expect(self.page.locator("#work")).to_contain_text("Failed")
        self.page.unroute("**/api/usage")
        self.page.get_by_role("button", name="Retry", exact=True).tap()
        expect(self.page.locator("#work")).to_contain_text("Cost estimate (API prices)")

    def test_todo_expansion_is_keyboard_accessible(self):
        todo = self.state / "cockpit/todo.json"
        todo.parent.mkdir(parents=True)
        todo.write_text(json.dumps({"items": [{"id": "aaaa1111", "project": "shop", "cwd": str(self.project),
                                                "text": "Fix cart", "notes": "Use the phone", "status": "todo", "created": 1}]}))
        self.open_paired()
        self.page.get_by_role("button", name="Todo", exact=True).tap()
        summary = self.page.locator("#work summary")
        summary.focus()
        self.page.keyboard.press("Enter")
        expect(self.page.get_by_role("button", name="Start in a new session")).to_be_visible()

    def test_the_back_gesture_returns_from_a_session_to_the_list(self):
        self.open_detail()
        self.page.go_back()
        expect(self.page.locator("#detail")).to_be_hidden()
        expect(self.page.locator("#list .card")).to_have_count(1)

    def test_the_chat_hides_the_tab_bar_and_keeps_the_reply_box_on_screen(self):
        self.open_detail()
        expect(self.page.locator("#tabs")).to_be_hidden()
        box = self.page.locator("#chat-send").bounding_box()
        self.assertLessEqual(box["y"] + box["height"], self.page.viewport_size["height"])

    def test_header_refresh_is_available_without_changing_the_tab(self):
        self.open_paired()
        self.page.get_by_role("button", name="Refresh view", exact=True).tap()
        expect(self.page.locator("#list .card")).to_have_count(1)

    def test_polling_recovers_after_a_connection_failure(self):
        self.open_paired()
        self.page.route("**/api/state", lambda route: route.abort())
        self.refresh()
        expect(self.page.locator("#updated")).to_have_text("offline")
        self.page.unroute("**/api/state")
        self.refresh()
        expect(self.page.locator("#updated")).not_to_have_text("offline")
        expect(self.page.locator("#list .card")).to_have_count(1)

    def test_screen_recovers_without_losing_its_last_output(self):
        self.open_detail()
        self.page.route("**/api/screen/*", lambda route: route.fulfill(status=409, body="pane closed"))
        self.page.evaluate("loadScreen()")
        expect(self.page.locator("#screen-note")).to_contain_text("pane closed")
        expect(self.page.locator("#screen")).to_contain_text("Ready")
        self.page.unroute("**/api/screen/*")
        self.page.evaluate("loadScreen()")
        expect(self.page.locator("#screen-note")).to_have_text("")

    def test_long_todo_and_pull_names_fit_a_small_phone(self):
        todo = self.state / "cockpit/todo.json"
        todo.parent.mkdir(parents=True)
        todo.write_text(json.dumps({"items": [{"id": "aaaa1111", "project": "shop", "cwd": str(self.project),
                                                "text": "Unbroken" * 50, "status": "todo", "created": 1}]}))
        self.shim("gh", "echo '" + json.dumps([{"number": 7, "title": "Fix cart", "url": "https://github.com/o/shop/pull/7",
                                                 "repository": {"name": "shop", "nameWithOwner": "LongName" * 20},
                                                 "author": {"login": "sam"}, "updatedAt": "2026-10-19T10:00:00Z"}]) + "'")
        self.open_paired()
        self.page.set_viewport_size({"width": 320, "height": 640})
        for label, content in [("Todo", "Unbroken"), ("Pull requests", "longname")]:
            with self.subTest(tab=label):
                self.page.get_by_role("button", name=label, exact=True).tap()
                expect(self.page.locator("#work .card")).to_contain_text(content)
                self.assertLessEqual(self.page.evaluate("document.documentElement.scrollWidth"), 320)

    def test_screenshot_at_phone_size(self):
        self.open_paired()
        folder = os.environ.get("COCKPIT_SCREENSHOTS")
        if folder:
            Path(folder).mkdir(parents=True, exist_ok=True)
            self.page.screenshot(path=str(Path(folder) / "sessions.png"), full_page=True)
            self.page.get_by_role("button", name="+ New session").tap()
            expect(self.page.get_by_label("First message", exact=True)).to_be_visible()
            self.page.screenshot(path=str(Path(folder) / "new-session.png"), full_page=True)
            self.page.get_by_role("button", name="Close", exact=True).tap()
            runs = self.runtime / "claude-runs"
            runs.mkdir()
            asking = runs / "0123456789ab.asking"
            asking.write_text(json.dumps({"id": "0123456789ab", "project": "shop", "cwd": str(self.project),
                                          "command": "make test", "why": "Check the checkout changes before applying them."}))
            self.refresh()
            expect(self.page.locator("#approvals .card")).to_be_visible()
            self.page.screenshot(path=str(Path(folder) / "approval.png"), full_page=True)
            asking.unlink()
            self.refresh()
            self.page.locator("#list .card").tap()
            expect(self.page.locator("#screen")).to_contain_text("Ready")
            self.page.screenshot(path=str(Path(folder) / "detail.png"), full_page=True)
            for label, content in [("Todo", "New task"), ("Pull requests", "No open pull request."),
                                   ("Usage", "Cost estimate (API prices)")]:
                self.page.get_by_role("button", name=label, exact=True).tap()
                expect(self.page.locator("#work")).to_contain_text(content)
                self.page.screenshot(path=str(Path(folder) / f"{label.lower().replace(' ', '-')}.png"), full_page=True)


if __name__ == "__main__":
    unittest.main()

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


HELPER = Path(__file__).resolve().parents[2] / "config/claude/.claude/hooks/billing-capture.py"
SPEC = importlib.util.spec_from_file_location("billing_capture", HELPER)
capture = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(capture)
SINCE = "2026-10-23T10:00:00Z"


def reply(message_id, newline=True):
    return json.dumps({"timestamp": "2026-10-23T10:01:00Z", "requestId": "request",
                       "message": {"id": message_id, "model": "claude-opus-5-5",
                                   "usage": {"output_tokens": 10}, "content": "private conversation"}}).encode() + (b"\n" if newline else b"")


class CountingInput:
    def __init__(self, source, counter):
        self.source = source
        self.counter = counter

    def __enter__(self):
        self.source.__enter__()
        return self

    def __exit__(self, *arguments):
        return self.source.__exit__(*arguments)

    def __getattr__(self, name):
        return getattr(self.source, name)

    def read(self, *arguments):
        result = self.source.read(*arguments)
        self.counter[0] += len(result)
        return result

    def readline(self, *arguments):
        result = self.source.readline(*arguments)
        self.counter[0] += len(result)
        return result

    def __iter__(self):
        return self

    def __next__(self):
        result = self.readline()
        if not result:
            raise StopIteration
        return result


class BillingCaptureTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name)
        self.environment = patch.dict(os.environ, {"HOME": str(self.home)})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.transcript = self.home / "transcript.jsonl"

    def run_capture(self, mode="api", since=SINCE):
        capture.capture_transcript(mode, since, str(self.transcript))

    def records(self):
        return {tuple(sorted(json.loads(path.read_text()).items()))
                for path in (self.home / ".cache/claude/billing").glob("*.json")}

    def assert_record(self, key, mode="api"):
        self.assertIn(tuple(sorted({"key": key, "billing": mode}.items())), self.records())

    def test_unchanged_capture_reads_bounded_validation_bytes(self):
        self.transcript.write_bytes(b'{"content":"not a reply"}\n' * 20000 + reply("first"))
        self.run_capture()
        counter = [0]
        open_regular = capture.open_regular
        with patch.object(capture, "open_regular", side_effect=lambda path: CountingInput(open_regular(path), counter)):
            self.run_capture()
        self.assertLess(counter[0], 4096)
        self.assert_record("first:request")

    def test_append_does_not_republish_old_evidence(self):
        self.transcript.write_bytes(reply("first"))
        self.run_capture()
        for path in (self.home / ".cache/claude/billing").glob("*.json"):
            path.unlink()
        with self.transcript.open("ab") as output:
            output.write(reply("second"))
        self.run_capture()
        self.assertEqual(len(self.records()), 1)
        self.assert_record("second:request")

    def test_launch_and_mode_have_independent_cursors(self):
        self.transcript.write_bytes(reply("first"))
        for mode, since in [("api", SINCE), ("subscription", SINCE), ("api", "2026-10-23T10:00:30Z")]:
            self.run_capture(mode, since)
        self.assert_record("first:request", "api")
        self.assert_record("first:request", "subscription")
        self.assertEqual(len(list((self.home / ".cache/claude/billing-cursors").glob("*.json"))), 3)

    def test_truncation_replacement_and_boundary_rewrite_replay_new_replies(self):
        for change in ["truncate", "replace", "rewrite"]:
            with self.subTest(change=change):
                self.transcript.write_bytes(reply("original") + (reply("padding") if change == "truncate" else b""))
                self.run_capture()
                replacement = reply("new-" + change)
                if change == "replace":
                    temporary = self.home / "replacement"
                    temporary.write_bytes(replacement)
                    temporary.replace(self.transcript)
                else:
                    self.transcript.write_bytes(replacement)
                self.run_capture()
                self.assert_record("new-" + change + ":request")

    def test_partial_line_is_retried_when_completed(self):
        self.transcript.write_bytes(reply("first") + reply("second")[:80])
        self.run_capture()
        with self.transcript.open("ab") as output:
            output.write(reply("second")[80:])
        self.run_capture()
        self.assert_record("second:request")

    def test_complete_final_line_without_newline_is_captured(self):
        self.transcript.write_bytes(reply("first", newline=False))
        self.run_capture()
        self.assert_record("first:request")
        with self.transcript.open("ab") as output:
            output.write(b"\n" + reply("second"))
        self.run_capture()
        self.assert_record("second:request")

    def test_failed_publication_does_not_advance_the_cursor(self):
        self.transcript.write_bytes(reply("first"))
        with patch.object(capture, "save", side_effect=OSError("disk full")):
            with self.assertRaisesRegex(OSError, "disk full"):
                self.run_capture()
        self.run_capture()
        self.assert_record("first:request")

    def test_cursor_files_reject_symlinks_and_fifos_without_writing_targets(self):
        self.transcript.write_bytes(reply("first"))
        self.run_capture()
        cursors = list((self.home / ".cache/claude/billing-cursors").glob("*.json"))
        self.assertEqual(len(cursors), 1)
        target = self.home / "target"
        target.write_text("do not overwrite")
        for kind in ["symlink", "fifo"]:
            with self.subTest(kind=kind):
                cursors[0].unlink()
                if kind == "symlink":
                    cursors[0].symlink_to(target)
                else:
                    os.mkfifo(cursors[0])
                with self.assertRaises((OSError, ValueError)):
                    self.run_capture()
                self.assertEqual(target.read_text(), "do not overwrite")

    def test_concurrent_cli_captures_preserve_all_evidence(self):
        self.transcript.write_bytes(b"".join(reply(str(index)) for index in range(100)))
        processes = [subprocess.Popen([sys.executable, str(HELPER), "record-transcript-billing", "api", SINCE, str(self.transcript)],
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(4)]
        for process in processes:
            _, errors = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 0, errors)
        self.assertEqual(len(self.records()), 100)

    def test_cursor_contains_no_conversation_text(self):
        self.transcript.write_bytes(reply("first"))
        self.run_capture()
        cursors = list((self.home / ".cache/claude/billing-cursors").glob("*.json"))
        self.assertEqual(len(cursors), 1)
        self.assertNotIn("private conversation", cursors[0].read_text())

    def test_replacing_cursor_directory_cannot_redirect_writes(self):
        self.transcript.write_bytes(reply("first"))
        self.run_capture()
        directory = self.home / ".cache/claude/billing-cursors"
        outside = self.home / "outside"
        outside.mkdir()
        open_file = os.open
        replaced = False

        def replace_before_cursor_open(path, *arguments, **keywords):
            nonlocal replaced
            if not replaced and (Path(path) == directory or Path(path).parent == directory):
                replaced = True
                descriptor = open_file(path, *arguments, **keywords) if Path(path) == directory else None
                directory.rename(self.home / "original-cursors")
                directory.symlink_to(outside)
                if descriptor is not None:
                    return descriptor
            return open_file(path, *arguments, **keywords)

        with patch.object(os, "open", side_effect=replace_before_cursor_open):
            self.run_capture()
        self.assertEqual(list(outside.iterdir()), [])


if __name__ == "__main__":
    unittest.main()

import contextlib
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest


REPOSITORY = Path(__file__).resolve().parents[2]
SCRIPT = REPOSITORY / "config/bin/.local/bin/greyproxy-prune"

# The tables greyproxy 0.4.5 creates (internal/greyproxy/migrations.go), reduced to the columns pruning reads.
SCHEMA = """
CREATE TABLE rules (id INTEGER PRIMARY KEY AUTOINCREMENT, destination_pattern TEXT,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')));
CREATE TABLE pending_requests (id INTEGER PRIMARY KEY AUTOINCREMENT, destination_host TEXT,
    last_seen DATETIME NOT NULL DEFAULT (datetime('now')));
CREATE TABLE request_logs (id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME NOT NULL DEFAULT (datetime('now')), destination_host TEXT);
CREATE TABLE http_transactions (id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME NOT NULL DEFAULT (datetime('now')), url TEXT, response_body BLOB);
CREATE TABLE conversations (id TEXT PRIMARY KEY,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')));
CREATE TABLE turns (id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE, turn_number INTEGER);
CREATE TABLE middleware_events (id INTEGER PRIMARY KEY AUTOINCREMENT, transaction_id INTEGER,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')));
"""
MIB = 1024 ** 2


class GreyproxyPruneTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.folder = Path(temporary.name)
        self.database = self.folder / "greyproxy.db"
        with self.open() as connection:
            connection.execute("PRAGMA journal_mode=WAL")
            connection.executescript(SCHEMA)
        self.bin = self.folder / "bin"
        self.bin.mkdir()
        self.fake_command("systemctl", "exit 3")
        self.fake_command("claude-sessions", 'printf "%s\\n" "$*" >> "$PRUNE_LOG"')
        self.environment = dict(os.environ, GREYPROXY_DB=str(self.database),
                                PATH=f"{self.bin}:{os.environ['PATH']}", PRUNE_LOG=str(self.folder / "log"))

    @contextlib.contextmanager
    def open(self):
        with contextlib.closing(sqlite3.connect(self.database, isolation_level=None)) as connection:
            connection.execute("PRAGMA foreign_keys=ON")
            yield connection

    def fake_command(self, name, body):
        command = self.bin / name
        command.write_text(f"#!/bin/sh\n{body}\n")
        command.chmod(0o700)

    def run_prune(self, *arguments):
        return subprocess.run([sys.executable, str(SCRIPT), *arguments], env=self.environment,
                              capture_output=True, text=True, timeout=60)

    def insert(self, sql, rows):
        with self.open() as connection:
            connection.executemany(sql, rows)

    def count(self, table):
        with self.open() as connection:
            return connection.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]

    def warnings(self):
        log = self.folder / "log"
        return log.read_text() if log.exists() else ""

    def test_deletes_traffic_history_older_than_the_age_limit(self):
        for table, column in [("http_transactions", "timestamp"), ("request_logs", "timestamp"),
                              ("middleware_events", "created_at")]:
            self.insert(f"INSERT INTO {table} ({column}) VALUES (datetime('now', ?))", [("-20 days",), ("-1 days",)])
        self.insert("INSERT INTO conversations (id, updated_at) VALUES (?, datetime('now', ?))",
                    [("old", "-20 days"), ("recent", "-1 days")])
        result = self.run_prune("--max-age-days", "14")
        self.assertEqual(result.returncode, 0, result.stderr)
        for table in ["http_transactions", "request_logs", "middleware_events", "conversations"]:
            with self.subTest(table=table):
                self.assertEqual(self.count(table), 1)

    def test_deleting_a_conversation_deletes_its_turns(self):
        self.insert("INSERT INTO conversations (id, updated_at) VALUES (?, datetime('now', '-30 days'))", [("old",)])
        self.insert("INSERT INTO turns (conversation_id, turn_number) VALUES ('old', ?)", [(1,), (2,)])
        result = self.run_prune()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.count("turns"), 0)

    def test_never_touches_rules_or_pending_requests(self):
        self.insert("INSERT INTO rules (destination_pattern, created_at) VALUES (?, datetime('now', '-400 days'))",
                    [("github.com",)])
        self.insert("INSERT INTO pending_requests (destination_host, last_seen) VALUES (?, datetime('now', '-400 days'))",
                    [("example.com",)])
        result = self.run_prune("--max-age-days", "1", "--max-gb", "0.0001")
        self.assertEqual(self.count("rules"), 1, result.stderr)
        self.assertEqual(self.count("pending_requests"), 1)

    def test_size_cap_deletes_the_oldest_transactions_first(self):
        body = b"x" * (16 * 1024)
        self.insert("INSERT INTO http_transactions (url, response_body) VALUES (?, ?)",
                    [(f"https://example.com/{index}", body) for index in range(600)])
        result = self.run_prune("--max-gb", str(4 * MIB / 1024 ** 3))
        self.assertEqual(result.returncode, 0, result.stderr)
        with self.open() as connection:
            remaining = [row[0] for row in connection.execute("SELECT id FROM http_transactions ORDER BY id")]
        self.assertTrue(0 < len(remaining) < 600, len(remaining))
        self.assertEqual(remaining, list(range(remaining[0], 601)))

    def test_fails_loudly_when_nothing_left_to_delete_still_exceeds_the_cap(self):
        self.insert("INSERT INTO request_logs (destination_host) VALUES (?)",
                    [("x" * 4096,) for _ in range(600)])
        result = self.run_prune("--max-gb", str(1 * MIB / 1024 ** 3))
        self.assertEqual(result.returncode, 1)
        self.assertIn("over the", self.warnings())

    def test_warns_until_the_database_is_compacted(self):
        result = self.run_prune()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("greyproxy-prune --compact", self.warnings())

    def test_compact_switches_on_incremental_vacuum_and_shrinks_the_file(self):
        body = b"x" * (64 * 1024)
        self.insert("INSERT INTO http_transactions (timestamp, response_body) VALUES (datetime('now', '-30 days'), ?)",
                    [(body,) for _ in range(100)])
        with self.open() as connection:
            connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        size_before = self.database.stat().st_size
        result = self.run_prune("--compact")
        self.assertEqual(result.returncode, 0, result.stderr)
        with self.open() as connection:
            self.assertEqual(connection.execute("PRAGMA auto_vacuum").fetchone()[0], 2)
        self.assertLess(self.database.stat().st_size, size_before / 10)
        self.assertEqual(self.warnings(), "")

    def test_after_compaction_daily_runs_return_freed_space_to_disk(self):
        self.assertEqual(self.run_prune("--compact").returncode, 0)
        body = b"x" * (64 * 1024)
        self.insert("INSERT INTO http_transactions (timestamp, response_body) VALUES (datetime('now', '-30 days'), ?)",
                    [(body,) for _ in range(100)])
        with self.open() as connection:
            connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        size_before = self.database.stat().st_size
        result = self.run_prune()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertLess(self.database.stat().st_size, size_before / 10)

    def test_compact_refuses_while_greyproxy_runs(self):
        self.fake_command("systemctl", "exit 0")
        self.insert("INSERT INTO http_transactions (timestamp) VALUES (datetime('now', '-30 days'))", [()])
        result = self.run_prune("--compact")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("systemctl --user stop greyproxy", result.stderr)
        self.assertEqual(self.count("http_transactions"), 1)

    def test_missing_database_fails(self):
        self.database.unlink()
        result = self.run_prune()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no database", result.stderr)


if __name__ == "__main__":
    unittest.main()

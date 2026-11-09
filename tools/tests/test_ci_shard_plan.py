"""The shard plan covers every test binary of the archive exactly once (#1261)."""

import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/ci-shards.py"


def shards_module():
    spec = importlib.util.spec_from_file_location("ci_shards", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def nextest_list(*binary_ids):
    return {"rust-suites": {binary_id: {"testcases": {}} for binary_id in binary_ids}}


def ids_in(filter_expression):
    return {
        part.removeprefix("binary_id(=").removesuffix(")")
        for part in filter_expression.split(" | ")
    }


class ShardPlanTests(unittest.TestCase):
    def test_every_binary_lands_in_exactly_one_shard(self):
        ci_shards = shards_module()
        binaries = [f"crate-{number}::suite" for number in range(40)]
        weights = {binary: float(number % 7 + 1) for number, binary in enumerate(binaries)}
        for shard_count in (1, 3, 5, 8):
            with self.subTest(shards=shard_count):
                plan = ci_shards.plan(nextest_list(*binaries), weights, shard_count)
                planned = [binary for shard in plan for binary in ids_in(shard["filter"])]
                self.assertEqual(sorted(planned), sorted(binaries))

    def test_a_binary_the_weights_do_not_know_still_runs(self):
        ci_shards = shards_module()
        plan = ci_shards.plan(nextest_list("known", "brand-new"), {"known": 50.0}, 2)
        self.assertEqual(
            sorted(binary for shard in plan for binary in ids_in(shard["filter"])),
            ["brand-new", "known"],
        )

    def test_a_weight_for_a_binary_no_longer_in_the_archive_is_ignored(self):
        ci_shards = shards_module()
        plan = ci_shards.plan(nextest_list("kept"), {"kept": 3.0, "removed": 900.0}, 2)
        self.assertEqual([ids_in(shard["filter"]) for shard in plan], [{"kept"}])

    def test_the_slowest_binaries_are_spread_over_the_shards(self):
        ci_shards = shards_module()
        weights = {"a": 100.0, "b": 100.0, "c": 100.0, "d": 1.0, "e": 1.0, "f": 1.0}
        plan = ci_shards.plan(nextest_list(*weights), weights, 3)
        for shard in plan:
            self.assertEqual(len(ids_in(shard["filter"]) & {"a", "b", "c"}), 1)

    def test_a_binary_id_the_filterset_language_would_misread_is_refused(self):
        ci_shards = shards_module()
        for binary_id in ("has space::a", "a,b", "a)b", "a'b"):
            with self.subTest(binary=binary_id), self.assertRaises(SystemExit):
                ci_shards.plan(nextest_list(binary_id), {}, 2)
        plan = ci_shards.plan(nextest_list("cockpit-cli::bin/torlek"), {}, 2)
        self.assertEqual(plan[0]["filter"], "binary_id(=cockpit-cli::bin/torlek)")

    def test_an_empty_archive_is_refused(self):
        with self.assertRaises(SystemExit):
            shards_module().plan(nextest_list(), {}, 3)

    def test_the_plan_prints_one_matrix_line_for_github_output(self):
        ci_shards = shards_module()
        with redirect_stdout(io.StringIO()) as output:
            stdin = io.StringIO(json.dumps(nextest_list("one::a", "two::b")))
            with patch.object(sys, "stdin", stdin):
                self.assertEqual(ci_shards.main(["plan", "--shards", "2"]), 0)
        (line,) = output.getvalue().splitlines()
        matrix = json.loads(line.removeprefix("matrix="))
        self.assertEqual([entry["shard"] for entry in matrix["include"]], [1, 2])

    def test_the_committed_weights_are_numbers_by_binary_id(self):
        weights = json.loads((ROOT / "tools/ci-test-weights.json").read_text())
        for binary_id, seconds in weights.items():
            with self.subTest(binary=binary_id):
                self.assertIsInstance(seconds, float | int)
                self.assertGreaterEqual(seconds, 0)


class WeightsFromJunitTests(unittest.TestCase):
    def test_a_binarys_seconds_are_the_sum_of_its_tests_and_fast_ones_are_dropped(self):
        junit = """<testsuites><testsuite name="slow::suite">
            <testcase name="a" time="2.5"/><testcase name="b" time="1.5"/></testsuite>
            <testsuite name="fast::suite"><testcase name="c" time="0.2"/></testsuite></testsuites>"""
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "junit.xml"
            report.write_text(junit)
            weights = shards_module().weights_from_junit([report])
        self.assertEqual(weights, {"slow::suite": 4.0})

    def test_the_command_line_refuses_unknown_arguments(self):
        result = subprocess.run(
            ["python3", str(SCRIPT), "bogus"], capture_output=True, text=True, check=False
        )
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()

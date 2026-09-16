#!/usr/bin/env python3
"""Unit tests for Experiment 4 pair-scoped accrual (no live provider)."""

import unittest
from unittest import mock

import run_trials as rt


def gate(ok=True):
    return {"gate": "pass" if ok else "fail", "max_s": 1.0 if ok else 20.0,
            "p95_s": 1.0 if ok else 20.0, "failures": 0 if ok else 3,
            "failure_details": [], "probes": [], "probe_count": 3, "p50_s": 1.0}


def attempt(attempt_id, health_valid, reason=None):
    return {"attempt_id": attempt_id, "health_valid": health_valid,
            "health_invalid_reason": reason}


class ShouldContinueTests(unittest.TestCase):
    def test_continues_with_no_attempts(self):
        keep_going, _ = rt.should_continue([])
        self.assertTrue(keep_going)

    def test_stops_at_target_valid_pairs(self):
        attempts = [attempt(i, True) for i in range(1, rt.TARGET_VALID_PAIRS + 1)]
        keep_going, reason = rt.should_continue(attempts)
        self.assertFalse(keep_going)
        self.assertIn("target reached", reason)

    def test_stops_immediately_after_health_invalid_pair(self):
        attempts = [attempt(1, True), attempt(2, False, "PROVIDER_HEALTH_FAILED_AFTER_PAIR")]
        keep_going, reason = rt.should_continue(attempts)
        self.assertFalse(keep_going)
        self.assertIn("health-invalid", reason)

    def test_stops_at_max_attempts_below_target(self):
        attempts = [attempt(i, i % 2 == 0) for i in range(1, rt.MAX_PAIR_ATTEMPTS + 1)]
        # Ensure last attempt is healthy so the cap (not the invalid-pair
        # rule) is what triggers the stop.
        attempts[-1] = attempt(rt.MAX_PAIR_ATTEMPTS, True)
        keep_going, reason = rt.should_continue(attempts)
        self.assertFalse(keep_going)
        self.assertIn("max attempts", reason)

    def test_does_not_stop_on_healthy_pair_below_target_and_cap(self):
        attempts = [attempt(1, True)]
        keep_going, _ = rt.should_continue(attempts)
        self.assertTrue(keep_going)


class ConditionSummaryTests(unittest.TestCase):
    def test_sums_and_leaves_uninstrumented_fields_none(self):
        rows = [
            {"success": True, "timed_out": False, "wall_clock_s": 10.0,
             "model_calls": 3, "input_tokens": 100, "output_tokens": 20,
             "cached_read_tokens": 5, "cached_write_tokens": 1,
             "code_query_calls": 2, "served_fresh": 1,
             "avoided_underlying_computations": 1, "file_reads": 4,
             "repo_searches": 1, "shell_cmds": 2, "captured_now": 1,
             "tool_validation_us": 500},
            {"success": False, "timed_out": True, "wall_clock_s": 900.0,
             "model_calls": 5, "input_tokens": 200, "output_tokens": 0,
             "cached_read_tokens": 0, "cached_write_tokens": 0,
             "code_query_calls": 1, "served_fresh": 1,
             "avoided_underlying_computations": 0, "file_reads": 2,
             "repo_searches": 0, "shell_cmds": 1, "captured_now": 1,
             "tool_validation_us": 300},
        ]
        summary = rt.condition_summary(rows)
        self.assertEqual(summary["success_count"], 1)
        self.assertEqual(summary["timeout_count"], 1)
        self.assertEqual(summary["wall_time_s"], 910.0)
        self.assertEqual(summary["avoided_computations"], 1)
        self.assertEqual(summary["valid_reuse"], 1)
        self.assertEqual(summary["underlying_computations"], 2)
        self.assertEqual(summary["ceremony_calls"], 0)
        for key in ("stale_withholding", "unknown_results", "false_valid_reuse"):
            self.assertIsNone(summary[key])


class PreHealthFailureShortCircuitTests(unittest.TestCase):
    """A pre-pair failure must not start the A/B pair at all."""

    def test_pair_never_starts_on_pre_pair_failure(self):
        with mock.patch.object(rt, "run_pair_sentinel", return_value=gate(ok=False)), \
             mock.patch.object(rt, "write_pair_report", side_effect=lambda g, m, i, p, path: {**g, "position": p}), \
             mock.patch.object(rt, "fresh_condition_workspace") as fake_ws:
            result = rt.run_pair_attempt(1, "test/model")

        fake_ws.assert_not_called()
        self.assertFalse(result["health_valid"])
        self.assertEqual(result["health_invalid_reason"], "PROVIDER_HEALTH_FAILED_BEFORE_PAIR")
        self.assertIsNone(result["condition_a"])
        self.assertIsNone(result["condition_b"])
        self.assertIsNone(result["post_health"])


if __name__ == "__main__":
    unittest.main()

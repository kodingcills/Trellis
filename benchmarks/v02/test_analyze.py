#!/usr/bin/env python3
"""Unit tests for Experiment 4 pair-level analysis (inclusion/exclusion,
discordance, diffs) — no live provider, synthetic PairAttempt fixtures."""

import unittest

from analyze import pair_metric_diffs, paired_task_discordance, split_valid


def condition(rows_success: dict, **summary_overrides) -> dict:
    summary = {"wall_time_s": 0, "model_calls": 0, "input_tokens": 0}
    summary.update(summary_overrides)
    rows = [{"task": task, "success": ok} for task, ok in rows_success.items()]
    return {"rows": rows, "summary": summary}


def pair_attempt(attempt_id, health_valid, a_success, b_success,
                  a_summary=None, b_summary=None, reason=None):
    return {
        "attempt_id": attempt_id,
        "health_valid": health_valid,
        "health_invalid_reason": reason,
        "condition_a": condition(a_success, **(a_summary or {})),
        "condition_b": condition(b_success, **(b_summary or {})),
    }


class SplitValidTests(unittest.TestCase):
    def test_splits_by_health_valid(self):
        attempts = [pair_attempt(1, True, {}, {}), pair_attempt(2, False, {}, {}, reason="X")]
        valid, invalid = split_valid(attempts)
        self.assertEqual([a["attempt_id"] for a in valid], [1])
        self.assertEqual([a["attempt_id"] for a in invalid], [2])


class PairMetricDiffsTests(unittest.TestCase):
    def test_diff_is_b_minus_a(self):
        attempts = [pair_attempt(1, True, {}, {}, a_summary={"wall_time_s": 100},
                                  b_summary={"wall_time_s": 80})]
        self.assertEqual(pair_metric_diffs(attempts, "wall_time_s"), [-20])

    def test_excluded_pairs_never_reach_diffs_when_prefiltered(self):
        attempts = [
            pair_attempt(1, True, {}, {}, a_summary={"wall_time_s": 100}, b_summary={"wall_time_s": 90}),
            pair_attempt(2, False, {}, {}, a_summary={"wall_time_s": 100}, b_summary={"wall_time_s": 900},
                         reason="PROVIDER_HEALTH_FAILED_AFTER_PAIR"),
        ]
        valid, _ = split_valid(attempts)
        diffs = pair_metric_diffs(valid, "wall_time_s")
        self.assertEqual(diffs, [-10])


class PairedTaskDiscordanceTests(unittest.TestCase):
    def test_counts_all_four_outcomes(self):
        attempts = [
            pair_attempt(1, True, {"T1": True, "T2": True}, {"T1": True, "T2": False}),
            pair_attempt(2, True, {"T1": False, "T2": False}, {"T1": True, "T2": False}),
        ]
        result = paired_task_discordance(attempts)
        self.assertEqual(result, {
            "both_pass": 1, "both_fail": 1, "a_pass_b_fail": 1, "a_fail_b_pass": 1,
        })


if __name__ == "__main__":
    unittest.main()

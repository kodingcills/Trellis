#!/usr/bin/env python3
"""Unit tests for the provider-health preflight gate (no live provider)."""

import json
import tempfile
import unittest
from pathlib import Path

from provider_health import (
    POST_PAIR_FAILURE,
    PRE_PAIR_FAILURE,
    classify_pair_health,
    evaluate_gate,
    write_pair_report,
    write_report,
)


def probe(ok=True, latency_s=2.0, error=None):
    return {"ok": ok, "latency_s": latency_s, "error": error}


class GateTests(unittest.TestCase):
    def test_healthy_set_passes(self):
        result = evaluate_gate([probe(latency_s=l) for l in (1.0, 1.5, 2.0, 2.5, 3.0, 14.9)])
        self.assertEqual(result["gate"], "pass")
        self.assertEqual(result["failures"], 0)
        self.assertEqual(result["p95_s"], 14.9)

    def test_p95_at_limit_fails(self):
        result = evaluate_gate([probe(latency_s=l) for l in (1, 2, 3, 4, 5, 15.0)])
        self.assertEqual(result["gate"], "fail")

    def test_single_failure_aborts(self):
        result = evaluate_gate([probe(latency_s=1)] * 5 + [probe(ok=False, error="timeout>60s")])
        self.assertEqual(result["gate"], "fail")
        self.assertEqual(result["failures"], 1)
        self.assertIn("timeout", result["failure_details"][0])

    def test_all_failed_aborts(self):
        result = evaluate_gate([probe(ok=False, error="exit 1: boom")] * 6)
        self.assertEqual(result["gate"], "fail")
        self.assertIsNone(result["p95_s"])

    def test_report_schema(self):
        result = evaluate_gate([probe(latency_s=l) for l in (1, 2, 3, 4, 5, 6)])
        path = Path(tempfile.mkdtemp()) / "health.json"
        write_report(result, "test/model", path)
        report = json.loads(path.read_text())
        for key in ("model", "timestamp_utc", "probe_count", "probes",
                    "p50_s", "p95_s", "max_s", "failures", "gate", "gate_rule"):
            self.assertIn(key, report)
        self.assertEqual(report["model"], "test/model")
        self.assertEqual(report["probe_count"], 6)
        self.assertEqual(report["gate"], "pass")


class PairSentinelGateTests(unittest.TestCase):
    """N=3 nearest-rank p95 must degenerate to max, so the pair sentinel
    reuses evaluate_gate's existing check unmodified."""

    def test_n3_p95_equals_max(self):
        result = evaluate_gate([probe(latency_s=l) for l in (1.0, 2.0, 14.9)])
        self.assertEqual(result["p95_s"], 14.9)
        self.assertEqual(result["p95_s"], result["max_s"])

    def test_n3_one_slow_probe_fails(self):
        result = evaluate_gate([probe(latency_s=l) for l in (1.0, 2.0, 15.1)])
        self.assertEqual(result["gate"], "fail")


class PairHealthClassificationTests(unittest.TestCase):
    """Health validity comes from sentinels only, never from outcome."""

    def test_both_pass_is_valid(self):
        pre = evaluate_gate([probe(latency_s=1)] * 3)
        post = evaluate_gate([probe(latency_s=1)] * 3)
        valid, reason = classify_pair_health(pre, post)
        self.assertTrue(valid)
        self.assertIsNone(reason)

    def test_pre_fail_short_circuits_before_post(self):
        pre = evaluate_gate([probe(ok=False, error="timeout>60s")] * 3)
        valid, reason = classify_pair_health(pre, post_gate=None)
        self.assertFalse(valid)
        self.assertEqual(reason, PRE_PAIR_FAILURE)

    def test_post_fail_after_healthy_pre(self):
        pre = evaluate_gate([probe(latency_s=1)] * 3)
        post = evaluate_gate([probe(ok=False, error="exit 1")] * 3)
        valid, reason = classify_pair_health(pre, post)
        self.assertFalse(valid)
        self.assertEqual(reason, POST_PAIR_FAILURE)

    def test_pair_report_carries_position_and_attempt_id(self):
        gate = evaluate_gate([probe(latency_s=1)] * 3)
        path = Path(tempfile.mkdtemp()) / "pair-health.json"
        report = write_pair_report(gate, "test/model", pair_attempt_id=3,
                                    position="post", path=path)
        self.assertEqual(report["pair_attempt_id"], 3)
        self.assertEqual(report["position"], "post")
        self.assertEqual(json.loads(path.read_text()), report)


if __name__ == "__main__":
    unittest.main()

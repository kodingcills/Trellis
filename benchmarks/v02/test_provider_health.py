#!/usr/bin/env python3
"""Unit tests for the provider-health preflight gate (no live provider)."""

import json
import tempfile
import unittest
from pathlib import Path

from provider_health import evaluate_gate, write_report


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


if __name__ == "__main__":
    unittest.main()

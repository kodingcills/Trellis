#!/usr/bin/env python3
"""Provider-health preflight for v0.2 real-agent benchmarks.

Issues a small number of trivial fixed probes against Claude Code
headless (`claude -p`) with the same model the benchmark will use, then
applies the predeclared gate frozen in ABLATION.md:

    trivial-prompt p95 < 15s AND zero probe failures (N=6)

If the gate fails, the benchmark must ABORT cleanly; aborted
provider-health runs are not Trellis trials. The probe report is
written next to the benchmark raw data so later readers know why a
run proceeded (or did not).

Stdlib only. No daemon, no background process, no monitoring.
"""

from __future__ import annotations

import json
import math
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

CLAUDE_BIN = "claude"
RESULTS = Path(__file__).resolve().parent / "results"

PROBE_PROMPT = "Reply with exactly the word OK and nothing else."
# 10 probes (upper end of the 5-10 range): with nearest-rank percentiles
# p95 is effectively the max at this sample size, so intermittent spikes
# reliably fail the gate instead of slipping through a lucky window.
PROBE_COUNT = 10
PROBE_TIMEOUT_S = 60
P95_LIMIT_S = 15.0
GATE_RULE = f"trivial-prompt p95 < {P95_LIMIT_S}s and 0 failures (N={PROBE_COUNT})"

# Pair-scoped sentinel (Experiment 4): 3 probes before + 3 after each
# real A/B pair, so mid-experiment degradation is caught, not just launch
# conditions. At N=3, nearest-rank p95 is the max observation, so
# evaluate_gate's existing p95 check already IS "every probe < 15s" here
# without a separate percentile computation.
PAIR_PROBE_COUNT = 3


def run_probe(model: str, claude_bin: str = CLAUDE_BIN, cwd: Path | None = None) -> dict:
    """One trivial fixed probe against Claude Code headless (`claude -p`).
    Repository- and Trellis-independent by design (Sec 8). Returns
    {ok, latency_s, error}."""
    if cwd is None:
        cwd = Path(tempfile.mkdtemp(prefix="trellis-probe-"))
    started = time.time()
    try:
        proc = subprocess.run(
            [claude_bin, "-p", PROBE_PROMPT, "--model", model,
             "--output-format", "json", "--setting-sources", "",
             "--permission-prompts", "none"],
            capture_output=True,
            text=True,
            timeout=PROBE_TIMEOUT_S,
            cwd=cwd,
        )
    except subprocess.TimeoutExpired:
        return {"ok": False, "latency_s": round(time.time() - started, 2),
                "error": f"timeout>{PROBE_TIMEOUT_S}s"}
    latency = round(time.time() - started, 2)
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout).strip()[-200:]
        return {"ok": False, "latency_s": latency, "error": f"exit {proc.returncode}: {tail}"}
    try:
        result = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return {"ok": False, "latency_s": latency, "error": "response not valid JSON"}
    if result.get("is_error") or "OK" not in result.get("result", ""):
        return {"ok": False, "latency_s": latency, "error": f"unexpected result: {result.get('result', '')[:100]}"}
    return {"ok": True, "latency_s": latency, "error": None}


def run_probes(model: str, count: int = PROBE_COUNT) -> list:
    return [run_probe(model) for _ in range(count)]


def _nearest_rank(sorted_vals: list, pct: float):
    if not sorted_vals:
        return None
    k = max(0, math.ceil(pct / 100 * len(sorted_vals)) - 1)
    return sorted_vals[k]


def evaluate_gate(probes: list) -> dict:
    """Pure gate evaluation over probe results (unit-testable)."""
    latencies = sorted(p["latency_s"] for p in probes if p["ok"])
    failures = [p for p in probes if not p["ok"]]
    p50 = _nearest_rank(latencies, 50)
    p95 = _nearest_rank(latencies, 95)
    latency_ok = p95 is not None and p95 < P95_LIMIT_S
    return {
        "probe_count": len(probes),
        "probes": probes,
        "p50_s": p50,
        "p95_s": p95,
        "max_s": latencies[-1] if latencies else None,
        "failures": len(failures),
        "failure_details": [p["error"] for p in failures],
        "gate": "pass" if (latency_ok and not failures) else "fail",
        "gate_rule": GATE_RULE,
    }


def write_report(gate_result: dict, model: str, path: Path) -> None:
    report = {"model": model,
              "timestamp_utc": datetime.now(timezone.utc).isoformat(),
              **gate_result}
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2))


def run_pair_sentinel(model: str, count: int = PAIR_PROBE_COUNT) -> dict:
    """Pre/post-pair sentinel: same gate logic as the launch preflight,
    smaller sample. Independent of pair outcome by construction — callers
    must invoke this before inspecting any treatment metric."""
    return evaluate_gate(run_probes(model, count))


def write_pair_report(gate_result: dict, model: str, pair_attempt_id: int,
                       position: str, path: Path) -> dict:
    """position is 'pre' or 'post'. Returns the written report dict."""
    report = {
        "model": model,
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "pair_attempt_id": pair_attempt_id,
        "position": position,
        **gate_result,
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2))
    return report


PRE_PAIR_FAILURE = "PROVIDER_HEALTH_FAILED_BEFORE_PAIR"
POST_PAIR_FAILURE = "PROVIDER_HEALTH_FAILED_AFTER_PAIR"


def classify_pair_health(pre_gate: dict, post_gate: dict | None) -> tuple[bool, str | None]:
    """Health validity from sentinels only — never from treatment outcome
    (wall time, tokens, success). A task timeout is not evidence here."""
    if pre_gate["gate"] != "pass":
        return False, PRE_PAIR_FAILURE
    if post_gate is None or post_gate["gate"] != "pass":
        return False, POST_PAIR_FAILURE
    return True, None


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default="claude-sonnet-5")
    parser.add_argument("--suffix", default="adhoc",
                        help="report filename suffix (e.g. n5, smoke)")
    args = parser.parse_args()

    result = evaluate_gate(run_probes(args.model))
    path = RESULTS / f"provider_health-{args.suffix}.json"
    write_report(result, args.model, path)
    print(f"provider health: p50={result['p50_s']}s p95={result['p95_s']}s "
          f"failures={result['failures']}/{result['probe_count']} "
          f"gate={result['gate'].upper()} -> {path}")
    if result["gate"] != "pass":
        for err in result["failure_details"]:
            print(f"  failure: {err}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

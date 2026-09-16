#!/usr/bin/env python3
"""Provider-health preflight for v0.2 real-agent benchmarks.

Issues a small number of trivial fixed probes against the same
model/provider the benchmark will use, then applies the predeclared
gate frozen in ABLATION.md:

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

OPC = Path.home() / ".opencode" / "bin" / "opencode"
RESULTS = Path(__file__).resolve().parent / "results"

PROBE_PROMPT = "Reply with exactly the word OK and nothing else."
# 10 probes (upper end of the 5-10 range): with nearest-rank percentiles
# p95 is effectively the max at this sample size, so intermittent spikes
# reliably fail the gate instead of slipping through a lucky window.
PROBE_COUNT = 10
PROBE_TIMEOUT_S = 60
P95_LIMIT_S = 15.0
GATE_RULE = f"trivial-prompt p95 < {P95_LIMIT_S}s and 0 failures (N={PROBE_COUNT})"


def run_probe(model: str, opc: Path = OPC, cwd: Path | None = None) -> dict:
    """One trivial fixed probe. Returns {ok, latency_s, error}."""
    if cwd is None:
        cwd = Path(tempfile.mkdtemp(prefix="trellis-probe-"))
    started = time.time()
    try:
        proc = subprocess.run(
            [str(opc), "run", PROBE_PROMPT, "-m", model, "--format", "json"],
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
    if "OK" not in proc.stdout:
        return {"ok": False, "latency_s": latency, "error": "response missing OK"}
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


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default="cheaperinference/gpt-5.6-luna")
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

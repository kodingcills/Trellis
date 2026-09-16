#!/usr/bin/env python3
"""v0.2 Phase 3 — aggregate paired-trial results into a comparison table.

Reads benchmarks/v02/results/raw-*.json and prints per-condition totals,
per-task breakdowns, and paired deltas. No statistics beyond simple
means/counts: n is small and the honest picture is the raw rows.
"""

from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

RESULTS = Path(__file__).resolve().parent / "results"

# Experiment 4 pair-level metrics (Sec 15): report every paired diff,
# not just aggregates. Health-invalid pairs are excluded from this
# primary comparison but never dropped from the raw file.
PAIR_METRICS = [
    "wall_time_s", "model_calls", "input_tokens", "output_tokens",
    "logical_tool_calls", "underlying_computations", "avoided_computations",
    "success_count", "timeout_count", "ceremony_calls",
]

METRICS = [
    "success", "wall_clock_s", "model_calls", "input_tokens", "output_tokens",
    "cached_read_tokens", "tool_ops", "file_reads", "repo_searches",
    "shell_cmds", "edits", "code_query_calls", "trellis_agent_ceremony_calls",
    "avoided_underlying_computations", "captured_now", "tool_validation_us",
    "trellis_status", "trellis_retrieve", "trellis_query", "trellis_publish",
]
LEDGER = ["publish", "retrieve_valid", "retrieve_stale", "retrieve_unknown", "validation_us"]


def load(path: Path) -> list:
    rows = json.loads(path.read_text())
    return decumulate(rows)


LEDGER_FIELDS = ["publish", "retrieve_valid", "retrieve_stale",
                 "retrieve_unknown", "validation_us"]
TASK_ORDER = {"T1": 0, "T2": 1, "T3": 2, "T4": 3, "T5": 4}


def decumulate(rows: list) -> list:
    """The store ledger is cumulative across a trial's task chain; convert
    each row's ledger fields to per-task deltas so sums are operation
    counts, not double-counted totals."""
    out = []
    for trial in sorted({r["trial"] for r in rows}):
        for cond in ("A", "B"):
            chain = sorted(
                (r for r in rows if r["trial"] == trial and r["condition"] == cond),
                key=lambda r: TASK_ORDER.get(r["task"], 99),
            )
            prev = {k: 0 for k in LEDGER_FIELDS}
            for row in chain:
                fixed = dict(row)
                for k in LEDGER_FIELDS:
                    cur = row.get(k, 0)
                    fixed[k] = cur - prev[k]
                    prev[k] = cur
                out.append(fixed)
    return out


def summarize(rows: list) -> dict:
    out = {}
    for cond in ("A", "B"):
        sel = [r for r in rows if r["condition"] == cond]
        summary = {}
        for key in METRICS + LEDGER:
            vals = [r.get(key, 0) for r in sel]
            summary[key] = {
                "total": sum(vals),
                "mean": round(sum(vals) / len(vals), 2) if vals else 0,
            }
        n_ok = sum(1 for r in sel if r["success"])
        summary["success_rate"] = f"{n_ok}/{len(sel)}"
        out[cond] = summary
    return out


def print_comparison(title: str, rows: list) -> None:
    print(f"\n=== {title} ===")
    s = summarize(rows)
    print(f"{'metric':24} {'A (baseline)':>18} {'B (trellis)':>18}")
    for key in ["success_rate", "wall_clock_s", "model_calls", "input_tokens",
                "output_tokens", "cached_read_tokens", "tool_ops", "file_reads",
                "repo_searches", "shell_cmds", "code_query_calls",
                "trellis_agent_ceremony_calls",
                "avoided_underlying_computations", "captured_now",
                "tool_validation_us",
                "trellis_status", "trellis_retrieve",
                "trellis_query", "trellis_publish", "publish", "retrieve_valid",
                "retrieve_stale", "retrieve_unknown", "validation_us"]:
        if key == "success_rate":
            print(f"{key:24} {s['A'][key]:>18} {s['B'][key]:>18}")
        else:
            a, b = s["A"][key], s["B"][key]
            print(f"{key:24} {a['total']:>10} (μ{a['mean']:>6}) {b['total']:>10} (μ{b['mean']:>6})")


def print_task_breakdown(rows: list) -> None:
    print("\n=== per-task means (A vs B) ===")
    tasks = sorted({r["task"] for r in rows})
    print(f"{'task':6} {'okA':>4} {'okB':>4} {'wallA':>7} {'wallB':>7} "
          f"{'inA':>8} {'inB':>8} {'opsA':>5} {'opsB':>5} {'reuseB':>7}")
    for task in tasks:
        a = [r for r in rows if r["condition"] == "A" and r["task"] == task]
        b = [r for r in rows if r["condition"] == "B" and r["task"] == task]
        mean = lambda sel, k: round(sum(r.get(k, 0) for r in sel) / len(sel), 1) if sel else 0
        reuse = sum(r.get("retrieve_valid", 0) for r in b)
        print(f"{task:6} {sum(r['success'] for r in a)}/{len(a):<3}"
              f" {sum(r['success'] for r in b)}/{len(b):<3}"
              f" {mean(a, 'wall_clock_s'):>7} {mean(b, 'wall_clock_s'):>7}"
              f" {mean(a, 'input_tokens'):>8} {mean(b, 'input_tokens'):>8}"
              f" {mean(a, 'tool_ops'):>5} {mean(b, 'tool_ops'):>5}"
              f" {reuse:>7}")


def paired_delta(rows: list, metric: str) -> None:
    print(f"\n=== paired per-trial {metric} (B - A; negative = trellis cheaper) ===")
    trials = sorted({r["trial"] for r in rows})
    for trial in trials:
        a = [r for r in rows if r["trial"] == trial and r["condition"] == "A"]
        b = [r for r in rows if r["trial"] == trial and r["condition"] == "B"]
        total = lambda sel: sum(r.get(metric, 0) for r in sel)
        print(f"trial {trial}: A={total(a):>10} B={total(b):>10} delta={total(b) - total(a):>+10}")


# ─────────────────────────────────────────────────────────────────────
# Experiment 4 — pair-scoped (pairs-*.json) analysis
# ─────────────────────────────────────────────────────────────────────

def paired_task_discordance(valid_attempts: list) -> dict:
    """Per-task success discordance across health-valid pairs (Sec 15)."""
    counts = {"both_pass": 0, "both_fail": 0, "a_pass_b_fail": 0, "a_fail_b_pass": 0}
    for attempt in valid_attempts:
        a_rows = {r["task"]: r["success"] for r in attempt["condition_a"]["rows"]}
        b_rows = {r["task"]: r["success"] for r in attempt["condition_b"]["rows"]}
        for task in a_rows:
            a_ok, b_ok = a_rows[task], b_rows.get(task, False)
            if a_ok and b_ok:
                counts["both_pass"] += 1
            elif not a_ok and not b_ok:
                counts["both_fail"] += 1
            elif a_ok and not b_ok:
                counts["a_pass_b_fail"] += 1
            else:
                counts["a_fail_b_pass"] += 1
    return counts


def split_valid(attempts: list) -> tuple[list, list]:
    return [a for a in attempts if a["health_valid"]], [a for a in attempts if not a["health_valid"]]


def pair_metric_diffs(valid_attempts: list, metric: str) -> list:
    """B-A per health-valid pair. Callers must pre-filter with split_valid —
    this never inspects health_valid itself, so an invalid pair silently
    included by a caller bug would not be caught here."""
    return [a["condition_b"]["summary"][metric] - a["condition_a"]["summary"][metric]
            for a in valid_attempts]


def analyze_pairs(path: Path) -> None:
    attempts = json.loads(path.read_text())
    valid, invalid = split_valid(attempts)
    print(f"\n=== {path.name}: {len(attempts)} pair attempts "
          f"({len(valid)} health-valid, {len(invalid)} excluded) ===")
    for a in invalid:
        print(f"  EXCLUDED attempt {a['attempt_id']}: {a['health_invalid_reason']}")
    if not valid:
        print("  no health-valid pairs — nothing to aggregate")
        return

    print(f"\n{'metric':24}{'diffs (B-A per pair)':>40}{'median':>10}{'mean':>10}")
    for metric in PAIR_METRICS:
        diffs = pair_metric_diffs(valid, metric)
        median = statistics.median(diffs)
        mean = round(statistics.mean(diffs), 2)
        print(f"{metric:24}{str(diffs):>40}{median:>10}{mean:>10}")

    discordance = paired_task_discordance(valid)
    print(f"\n=== per-task correctness discordance ({len(valid)} valid pairs) ===")
    for key, count in discordance.items():
        print(f"  {key}: {count}")

    ceremony_total = sum(a["condition_b"]["summary"]["ceremony_calls"] for a in valid)
    avoided_total = sum(a["condition_b"]["summary"]["avoided_computations"] for a in valid)
    print(f"\nsafety gates: ceremony_calls={ceremony_total} (must be 0), "
          f"avoided_computations={avoided_total} (must be >0 to show mechanism use)")


def main() -> None:
    paths = sys.argv[1:] or sorted(
        str(p) for p in list(RESULTS.glob("raw-*.json")) + list(RESULTS.glob("pairs-*.json"))
    )
    for path in paths:
        if Path(path).name.startswith("pairs-"):
            analyze_pairs(Path(path))
            continue
        rows = load(Path(path))
        name = Path(path).name
        print_comparison(f"{name}: {len(rows)} task-runs", rows)
        print_task_breakdown(rows)
        paired_delta(rows, "input_tokens")
        paired_delta(rows, "wall_clock_s")


if __name__ == "__main__":
    main()

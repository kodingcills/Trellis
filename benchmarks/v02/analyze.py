#!/usr/bin/env python3
"""v0.2 Phase 3 — aggregate paired-trial results into a comparison table.

Reads benchmarks/v02/results/raw-*.json and prints per-condition totals,
per-task breakdowns, and paired deltas. No statistics beyond simple
means/counts: n is small and the honest picture is the raw rows.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

RESULTS = Path(__file__).resolve().parent / "results"

METRICS = [
    "success", "wall_clock_s", "model_calls", "input_tokens", "output_tokens",
    "cached_read_tokens", "tool_ops", "file_reads", "repo_searches",
    "shell_cmds", "edits", "trellis_status", "trellis_retrieve",
    "trellis_query", "trellis_publish",
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
                "repo_searches", "shell_cmds", "trellis_status", "trellis_retrieve",
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


def main() -> None:
    paths = sys.argv[1:] or sorted(str(p) for p in RESULTS.glob("raw-*.json"))
    for path in paths:
        rows = load(Path(path))
        name = Path(path).name
        print_comparison(f"{name}: {len(rows)} task-runs", rows)
        print_task_breakdown(rows)
        paired_delta(rows, "input_tokens")
        paired_delta(rows, "wall_clock_s")


if __name__ == "__main__":
    main()

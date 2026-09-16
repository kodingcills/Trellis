#!/usr/bin/env python3
"""v0.2 Phase 2 — paired real-agent experiment harness.

Two conditions, identical in every respect except the independent
variable (artifact persistence across a related-task chain):
  A (baseline): Trellis store reset before every task -> nothing to reuse.
  B (trellis):  store persists across the task chain -> validated reuse.

Same model, same agent prompt, same MCP tools, same CLI, same timeout.
The agent is Claude Code itself, headless (`claude -p --output-format
stream-json`); the harness parses tokens, model calls, and tool
operations from the event stream, then runs deterministic per-task
verifiers (structural checks + unittest).

Usage: python3 benchmarks/v02/run_trials.py [--model M] [--smoke]
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from provider_health import (
    classify_pair_health,
    evaluate_gate,
    run_pair_sentinel,
    run_probes,
    write_pair_report,
    write_report,
)

ROOT = Path(__file__).resolve().parents[2]
CLAUDE_BIN = "claude"
FIXTURE = ROOT / "fixtures" / "python_auth" / "base"
RESULTS = ROOT / "benchmarks" / "v02" / "results"
DEFAULT_MODEL = "claude-sonnet-5"
TASK_TIMEOUT_S = 900
WORKDIR = Path("/tmp/trellis-v02-trials")

# Agent-facing tool surface, byte-identical across A/B (Sec 12). Fixed
# across both conditions so tool availability is never the independent
# variable — only Trellis's store persistence is.
ALLOWED_TOOLS = "Bash,Edit,Write,Read,Grep,Glob,ToolSearch,mcp__trellis__code_query"

# Experiment 4 — stable-provider transparent reuse, pair-scoped health.
# Frozen per REPORT.md gate decision after Experiment 3 (mid-run provider
# degradation slipped past a single upfront preflight). Same tasks,
# verifiers, and timeout as Experiments 1/3. Agent harness switched from
# OpenCode to Claude Code headless before any real pair ran under this
# id (no data existed yet — not a mid-experiment methodology change).
# Do not edit task/verifier/metric definitions inside this experiment
# id; cut a new one instead.
EXPERIMENT_ID = "exp4-claude-code"
TARGET_VALID_PAIRS = 5
MAX_PAIR_ATTEMPTS = 8


# ─────────────────────────────────────────────────────────────────────
# Task chain (distinct tasks sharing repository understanding)
# ─────────────────────────────────────────────────────────────────────

TASKS = [
    {
        "id": "T1",
        "spec": (
            "Security hardening task: raise the PBKDF2 iteration count in "
            "auth/password.py from 100_000 to 200_000, and update any test "
            "that depends on the old count. Run the test suite to confirm."
        ),
        "verify": lambda repo: (
            "200_000" in read(repo, "auth/password.py")
            and suite_ok(repo)
        ),
    },
    {
        "id": "T2",
        "spec": (
            "Add account lockout to the login path: after 5 consecutive "
            "failed authentication attempts for the same user, authentication "
            "must fail with a new AccountLocked exception (define it in "
            "auth/errors.py and raise it from the users login flow). A "
            "successful login resets the failure counter. Add a test for the "
            "lockout behavior and run the suite."
        ),
        "verify": lambda repo: (
            "AccountLocked" in read(repo, "auth/errors.py")
            and "AccountLocked" in read(repo, "users/service.py")
            and suite_ok(repo)
        ),
    },
    {
        "id": "T3",
        "spec": (
            "Add lightweight auth metrics: create auth/metrics.py exposing "
            "increment(counter: str) backed by an in-process dict of counts "
            "(with a get(counter) accessor). Instrument the users login flow "
            "to increment 'login_success' and 'login_failure'. Add a test "
            "for the counters and run the suite."
        ),
        "verify": lambda repo: (
            "def increment" in read(repo, "auth/metrics.py")
            and "increment" in read(repo, "users/service.py")
            and suite_ok(repo)
        ),
    },
    {
        "id": "T4",
        "spec": (
            "Change refresh-token behavior in auth/tokens.py: refresh_token "
            "must now issue a token whose TTL is double the configured "
            "default (Settings.token_ttl_seconds() * 2) when no explicit "
            "ttl_seconds is passed. Then add an endpoint post_refresh to "
            "api/routes.py: it takes (users, body) like post_login, expects "
            "body['token'], verifies it with auth.tokens.verify_token and "
            "returns json_response(200, {'token': <refreshed>}). Add a test "
            "and run the suite."
        ),
        "verify": lambda repo: (
            "post_refresh" in read(repo, "api/routes.py")
            and "verify_token" in read(repo, "api/routes.py")
            and "* 2" in read(repo, "auth/tokens.py")
            and suite_ok(repo)
        ),
    },
    {
        "id": "T5",
        "spec": (
            "Test audit task: the earlier tasks added behavior. Add at least "
            "three new test functions covering: (1) expired tokens raise "
            "InvalidToken, (2) tampered token signatures raise InvalidToken, "
            "(3) the refresh flow (auth.tokens.refresh_token + "
            "api.routes.post_refresh). Put them in tests/test_v02_audit.py. "
            "Run the full suite and make sure everything passes."
        ),
        "verify": lambda repo: (
            path_exists(repo, "tests/test_v02_audit.py")
            and read(repo, "tests/test_v02_audit.py").count("def test_") >= 3
            and suite_ok(repo)
        ),
    },
]


def read(repo: Path, rel: str) -> str:
    path = repo / rel
    return path.read_text() if path.exists() else ""


def path_exists(repo: Path, rel: str) -> bool:
    return (repo / rel).exists()


def suite_ok(repo: Path) -> bool:
    proc = subprocess.run(
        ["python3", "-m", "unittest", "discover", "-s", "tests", "-t", "."],
        cwd=repo,
        capture_output=True,
        text=True,
        timeout=120,
    )
    return proc.returncode == 0


# ─────────────────────────────────────────────────────────────────────
# Trial machinery
# ─────────────────────────────────────────────────────────────────────

def write_mcp_config(repo: Path, store: Path, mode: str) -> Path:
    """Bake per-condition env directly into the MCP config rather than
    relying on subprocess env inheritance into the MCP child process."""
    config = {
        "mcpServers": {
            "trellis": {
                "command": "python3",
                "args": [str(ROOT / "benchmarks/v02/mcp_trellis.py")],
                "env": {
                    "TRELLIS_BIN": str(ROOT / "target/debug/trellis"),
                    "TRELLIS_REPO": str(repo),
                    "TRELLIS_STORE": str(store),
                    "TRELLIS_MODE": mode,
                },
            }
        }
    }
    path = repo.parent / "mcp.json"
    path.write_text(json.dumps(config, indent=2))
    return path


def fresh_condition_workspace(trial: int, condition: str) -> tuple[Path, Path]:
    base = WORKDIR / f"trial{trial}-{condition}"
    if base.exists():
        shutil.rmtree(base)
    repo = base / "repo"
    shutil.copytree(FIXTURE, repo)
    for pycache in repo.rglob("__pycache__"):
        shutil.rmtree(pycache)
    store = base / "store.db"
    env = cli_env(repo, store)
    run_cli(["init", "--repo", str(repo), "--store", str(store)], env)
    return repo, store


def cli_env(repo: Path, store: Path) -> dict:
    import os

    env = dict(os.environ)
    env["TRELLIS_BIN"] = str(ROOT / "target/debug/trellis")
    env["TRELLIS_REPO"] = str(repo)
    env["TRELLIS_STORE"] = str(store)
    return env


def run_cli(args: list, env: dict) -> str:
    proc = subprocess.run(
        [str(ROOT / "target/debug/trellis")] + args,
        capture_output=True,
        text=True,
        env=env,
        timeout=120,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"trellis {' '.join(args[:1])} failed: {proc.stderr}")
    return proc.stdout


def store_path(repo: Path) -> Path:
    return repo.parent / "store.db"


def run_agent(task_spec: str, repo: Path, model: str, condition: str) -> dict:
    mode = "trellis" if condition == "B" else "baseline"
    mcp_config = write_mcp_config(repo, store_path(repo), mode)
    started = time.time()
    ledger = repo / ".trellis-events.jsonl"
    ledger_before = ledger.read_text().splitlines() if ledger.exists() else []
    try:
        proc = subprocess.run(
            [
                CLAUDE_BIN, "-p", task_spec,
                "--model", model,
                "--output-format", "stream-json", "--verbose",
                # Excludes user/project CLAUDE.md, hooks, skills, and any
                # other MCP servers on this machine — the benchmarked
                # agent must run with nothing but the fixed tool surface
                # below, not this operator's personal configuration.
                "--setting-sources", "",
                "--mcp-config", str(mcp_config), "--strict-mcp-config",
                "--allowedTools", ALLOWED_TOOLS,
                "--permission-prompts", "none",
            ],
            cwd=repo,
            capture_output=True,
            text=True,
            timeout=TASK_TIMEOUT_S,
        )
        stdout, failed, timed_out = proc.stdout, proc.returncode != 0, False
    except subprocess.TimeoutExpired as exc:
        # Keep partial metrics: the JSON event stream up to the timeout
        # is on the exception object. Timeout is a distinct outcome from
        # a crash/nonzero-exit failure — Sec 9.3: a timeout alone must
        # not be conflated with provider-health invalidity or treated
        # differently from any other treatment outcome.
        stdout = exc.stdout.decode() if isinstance(exc.stdout, bytes) else (exc.stdout or "")
        failed, timed_out = True, True
    wall = time.time() - started
    metrics = parse_events(stdout)
    metrics["wall_clock_s"] = round(wall, 1)
    metrics["run_failed"] = failed
    metrics["timed_out"] = timed_out
    ledger_after = ledger.read_text().splitlines() if ledger.exists() else []
    new_events = ledger_after[len(ledger_before):] if len(ledger_after) > len(ledger_before) else []
    metrics.update(parse_tool_ledger(new_events))
    return metrics


def parse_tool_ledger(lines: list) -> dict:
    """Direct mechanism metrics from the CLI's per-tool-call ledger."""
    out = {
        "avoided_underlying_computations": 0,
        "served_fresh": 0,
        "captured_now": 0,
        "tool_validation_us": 0,
    }
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("op") != "tool":
            continue
        tag = event.get("tag")
        out["tool_validation_us"] += event.get("elapsed_us", 0)
        if tag == "reuse":
            out["avoided_underlying_computations"] += 1
        elif tag == "fresh+captured":
            out["captured_now"] += 1
            out["served_fresh"] += 1
        else:
            out["served_fresh"] += 1
    return out


def parse_events(stdout: str) -> dict:
    """Parse Claude Code's `--output-format stream-json` event stream.
    Tool-use blocks are deduped by their block id: the same growing
    assistant message is re-emitted across multiple lines as content
    streams in, so a naive per-line count double/triple-counts. Token
    and turn totals are reconstructed the same way (dedup by message
    id) rather than read from the final `result` event, so a run that
    hits the task timeout before that event exists still yields real
    partial metrics instead of all-zero."""
    metrics = {
        "model_calls": 0,
        "input_tokens": 0,
        "output_tokens": 0,
        "cached_read_tokens": 0,
        "cached_write_tokens": 0,
        "tool_ops": 0,
        "file_reads": 0,
        "repo_searches": 0,
        "shell_cmds": 0,
        "edits": 0,
        "code_query_calls": 0,
        "trellis_agent_ceremony_calls": 0,
        "tool_search_calls": 0,
    }
    seen_messages: set = set()
    seen_tools: set = set()
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") != "assistant":
            continue
        message = event.get("message", {})
        msg_id = message.get("id")
        if msg_id and msg_id not in seen_messages:
            seen_messages.add(msg_id)
            metrics["model_calls"] += 1
            usage = message.get("usage", {})
            metrics["input_tokens"] += usage.get("input_tokens", 0)
            metrics["output_tokens"] += usage.get("output_tokens", 0)
            metrics["cached_read_tokens"] += usage.get("cache_read_input_tokens", 0)
            metrics["cached_write_tokens"] += usage.get("cache_creation_input_tokens", 0)
        for block in message.get("content", []):
            if block.get("type") != "tool_use":
                continue
            tool_id = block.get("id")
            if not tool_id or tool_id in seen_tools:
                continue
            seen_tools.add(tool_id)
            name = block.get("name", "")
            metrics["tool_ops"] += 1
            if name == "Read":
                metrics["file_reads"] += 1
            elif name in ("Grep", "Glob"):
                metrics["repo_searches"] += 1
            elif name == "Bash":
                metrics["shell_cmds"] += 1
            elif name in ("Edit", "Write"):
                metrics["edits"] += 1
            elif name == "ToolSearch":
                metrics["tool_search_calls"] += 1
            elif name == "mcp__trellis__code_query":
                metrics["code_query_calls"] += 1
            elif name.startswith("mcp__trellis__"):
                metrics["trellis_agent_ceremony_calls"] += 1
    return metrics


QUERY_ENVELOPE = (
    "\n\nBefore making any edits, first use the code_query tool to map the "
    "affected code: call code_query(kind=\"definitions\", subject=\"<the main "
    "dotted module of this task>\") and code_query(kind=\"callers\", "
    "subject=\"<the primary dotted symbol this task changes>\"). Then do "
    "the task and run the test suite."
)


def run_trial(trial: int, model: str) -> list:
    """Dev/wiring-check path only (used by --smoke). Not part of the
    frozen Experiment 4 protocol — no pair-scoped health, no accrual."""
    rows = []
    order = ["A", "B"] if trial % 2 == 0 else ["B", "A"]
    for condition in order:
        repo, store = fresh_condition_workspace(trial, condition)
        for task in TASKS:
            metrics = run_agent(task["spec"] + QUERY_ENVELOPE, repo, model, condition)
            success = task["verify"](repo)
            rows.append({
                "trial": trial,
                "condition": condition,
                "task": task["id"],
                "success": success,
                **metrics,
            })
            print(f"  trial {trial} {condition} {task['id']}: "
                  f"success={success} wall={metrics['wall_clock_s']}s "
                  f"in={metrics['input_tokens']} out={metrics['output_tokens']} "
                  f"toolops={metrics['tool_ops']} "
                  f"(reuse={metrics['avoided_underlying_computations']} "
                  f"captured={metrics['captured_now']} "
                  f"ceremony={metrics['trellis_agent_ceremony_calls']})")
    return rows


# ─────────────────────────────────────────────────────────────────────
# Experiment 4 — pair-scoped provider health + resumable accrual
# ─────────────────────────────────────────────────────────────────────

def git_head() -> str:
    proc = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT,
                           capture_output=True, text=True, timeout=10)
    return proc.stdout.strip() if proc.returncode == 0 else "unknown"


def condition_summary(rows: list) -> dict:
    """ConditionResult aggregate over one condition's task rows. Fields
    with no independent instrumentation on this transparent path (a
    reuse serve is only made when pins are verified current against the
    live tree, so stale/unknown serves are not separately observable —
    see REPORT.md limitations) are None, never a fabricated 0."""
    total = lambda k: sum(r.get(k, 0) for r in rows)
    return {
        "success_count": total("success"),
        "timeout_count": total("timed_out"),
        "wall_time_s": round(total("wall_clock_s"), 1),
        "model_calls": total("model_calls"),
        "input_tokens": total("input_tokens"),
        "output_tokens": total("output_tokens"),
        "provider_cached_tokens": total("cached_read_tokens") + total("cached_write_tokens"),
        "logical_tool_calls": total("code_query_calls"),
        "underlying_computations": total("served_fresh"),
        "avoided_computations": total("avoided_underlying_computations"),
        "file_reads": total("file_reads"),
        "repository_searches": total("repo_searches"),
        "shell_operations": total("shell_cmds"),
        "automatic_captures": total("captured_now"),
        "valid_reuse": total("avoided_underlying_computations"),
        "stale_withholding": None,
        "unknown_results": None,
        "false_valid_reuse": None,
        "trellis_runtime_time_s": round(total("tool_validation_us") / 1e6, 3),
        "ceremony_calls": total("trellis_agent_ceremony_calls"),
        # Platform overhead (Claude Code's deferred-tool discovery),
        # identical mechanism in both conditions — not Trellis ceremony.
        "tool_search_calls": total("tool_search_calls"),
    }


def run_pair_attempt(attempt_id: int, model: str) -> dict:
    """One PairAttempt: pre-pair sentinel -> A/B pair -> post-pair
    sentinel. Health validity is decided from the sentinels alone, never
    from the A/B outcome (Sec 9). A pre-pair failure means the pair
    never runs at all."""
    order = ["A", "B"] if attempt_id % 2 == 0 else ["B", "A"]
    attempt = {
        "attempt_id": attempt_id,
        "experiment_id": EXPERIMENT_ID,
        "protocol_commit": git_head(),
        "timestamp_utc": datetime_now_iso(),
        "provider": model.split("/")[0],
        "model": model,
        "model_configuration": {"temperature": 0},
        "order": order,
    }

    pre_gate = run_pair_sentinel(model)
    pre_path = RESULTS / f"provider_health-pair{attempt_id}-pre.json"
    attempt["pre_health"] = write_pair_report(pre_gate, model, attempt_id, "pre", pre_path)
    print(f"  [pair {attempt_id}] pre-health: max={pre_gate['max_s']}s "
          f"failures={pre_gate['failures']}/{pre_gate['probe_count']} "
          f"gate={pre_gate['gate'].upper()}")

    if pre_gate["gate"] != "pass":
        valid, reason = classify_pair_health(pre_gate, None)
        attempt.update(condition_a=None, condition_b=None, post_health=None,
                        health_valid=valid, health_invalid_reason=reason)
        return attempt

    conditions = {}
    for condition in order:
        repo, store = fresh_condition_workspace(attempt_id, condition)
        rows = []
        for task in TASKS:
            metrics = run_agent(task["spec"] + QUERY_ENVELOPE, repo, model, condition)
            success = task["verify"](repo)
            row = {
                "trial": attempt_id,
                "condition": condition,
                "task": task["id"],
                "success": success,
                **metrics,
            }
            rows.append(row)
            print(f"  [pair {attempt_id}] {condition} {task['id']}: "
                  f"success={success} timed_out={metrics['timed_out']} "
                  f"wall={metrics['wall_clock_s']}s in={metrics['input_tokens']} "
                  f"(reuse={metrics['avoided_underlying_computations']} "
                  f"ceremony={metrics['trellis_agent_ceremony_calls']})")
        conditions[condition] = {"rows": rows, "summary": condition_summary(rows)}

    post_gate = run_pair_sentinel(model)
    post_path = RESULTS / f"provider_health-pair{attempt_id}-post.json"
    attempt["post_health"] = write_pair_report(post_gate, model, attempt_id, "post", post_path)
    print(f"  [pair {attempt_id}] post-health: max={post_gate['max_s']}s "
          f"failures={post_gate['failures']}/{post_gate['probe_count']} "
          f"gate={post_gate['gate'].upper()}")

    valid, reason = classify_pair_health(pre_gate, post_gate)
    attempt.update(condition_a=conditions["A"], condition_b=conditions["B"],
                    health_valid=valid, health_invalid_reason=reason)
    return attempt


def should_continue(attempts: list) -> tuple[bool, str]:
    """Pure accrual decision (Sec 10-11): stop on target reached, cap
    exhausted, or the most recent attempt being health-invalid (must not
    hammer a degraded provider — resume later under the same protocol)."""
    valid = [a for a in attempts if a["health_valid"]]
    if len(valid) >= TARGET_VALID_PAIRS:
        return False, f"target reached: {len(valid)} health-valid pairs"
    if attempts and not attempts[-1]["health_valid"]:
        return False, (f"health-invalid pair (attempt {attempts[-1]['attempt_id']}): "
                        f"{attempts[-1]['health_invalid_reason']} — stopping execution "
                        f"window, resume later under the same frozen protocol")
    if len(attempts) >= MAX_PAIR_ATTEMPTS:
        return False, (f"max attempts ({MAX_PAIR_ATTEMPTS}) reached with only "
                        f"{len(valid)}/{TARGET_VALID_PAIRS} valid pairs — provider/"
                        f"environment unsuitable for this evidence block")
    return True, "continue"


def datetime_now_iso() -> str:
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).isoformat()


def run_experiment4(model: str) -> None:
    pairs_path = RESULTS / f"pairs-{EXPERIMENT_ID}.json"
    attempts = json.loads(pairs_path.read_text()) if pairs_path.exists() else []

    while True:
        keep_going, reason = should_continue(attempts)
        if not keep_going:
            print(f"STOP: {reason}")
            break
        next_id = len(attempts) + 1
        print(f"=== pair attempt {next_id}/{MAX_PAIR_ATTEMPTS} "
              f"(order {'A,B' if next_id % 2 == 0 else 'B,A'}) ===")
        attempt = run_pair_attempt(next_id, model)
        attempts.append(attempt)
        # Checkpoint immediately: interruption must not lose prior pairs.
        pairs_path.write_text(json.dumps(attempts, indent=2))
        valid_n = sum(1 for a in attempts if a["health_valid"])
        print(f"  pair {next_id} health_valid={attempt['health_valid']} "
              f"reason={attempt['health_invalid_reason']} "
              f"({valid_n}/{TARGET_VALID_PAIRS} valid so far) -> {pairs_path}")

    valid_n = sum(1 for a in attempts if a["health_valid"])
    print(f"Experiment 4 session end: {valid_n}/{TARGET_VALID_PAIRS} valid pairs "
          f"across {len(attempts)} attempts. Raw: {pairs_path}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--smoke", action="store_true",
                        help="dev-only wiring check: 1 trial, T1+T2, single "
                             "upfront preflight. NOT part of Experiment 4 — "
                             "does not touch pairs-*.json.")
    args = parser.parse_args()

    if not (ROOT / "target/debug/trellis").exists():
        raise SystemExit("build the CLI first: cargo build -p trellis-cli")
    WORKDIR.mkdir(parents=True, exist_ok=True)
    RESULTS.mkdir(parents=True, exist_ok=True)

    if args.smoke:
        globals()["TASKS"][:] = TASKS[:2]
        health = evaluate_gate(run_probes(args.model))
        health_path = RESULTS / "provider_health-smoke.json"
        write_report(health, args.model, health_path)
        print(f"provider health: p50={health['p50_s']}s p95={health['p95_s']}s "
              f"gate={health['gate'].upper()} -> {health_path}")
        if health["gate"] != "pass":
            print("ABORTING smoke check: provider health gate failed")
            sys.exit(2)
        out = RESULTS / "raw-smoke.json"
        out.write_text(json.dumps(run_trial(1, args.model), indent=2))
        print(f"raw rows -> {out}")
        return

    # Experiment 4: no separate rehearsal, no standalone preflight. The
    # first pre-pair sentinel below belongs to the first real pair.
    run_experiment4(args.model)


if __name__ == "__main__":
    main()

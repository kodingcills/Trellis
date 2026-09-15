#!/usr/bin/env python3
"""v0.2 Phase 2 — paired real-agent experiment harness.

Two conditions, identical in every respect except the independent
variable (artifact persistence across a related-task chain):
  A (baseline): Trellis store reset before every task -> nothing to reuse.
  B (trellis):  store persists across the task chain -> validated reuse.

Same model, same agent prompt, same MCP tools, same CLI, same timeout.
`opencode run --format json` streams events; the harness parses tokens,
model calls, and tool operations, then runs deterministic per-task
verifiers (structural checks + unittest).

Usage: python3 benchmarks/v02/run_trials.py --trials 5 [--model M] [--smoke]
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OPC = Path.home() / ".opencode" / "bin" / "opencode"
FIXTURE = ROOT / "fixtures" / "python_auth" / "base"
RESULTS = ROOT / "benchmarks" / "v02" / "results"
DEFAULT_MODEL = "cheaperinference/gpt-5.6-luna"
TASK_TIMEOUT_S = 900
WORKDIR = Path("/tmp/trellis-v02-trials")


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

def wire_workspace(repo: Path) -> None:
    agent_dir = repo / ".opencode" / "agent"
    agent_dir.mkdir(parents=True, exist_ok=True)
    config = {
        "mcp": {
            "trellis": {
                "type": "local",
                "command": ["python3", str(ROOT / "benchmarks/v02/mcp_trellis.py")],
                "enabled": True,
            }
        }
    }
    (repo / ".opencode" / "opencode.json").write_text(json.dumps(config, indent=2))
    shutil.copy(
        ROOT / "benchmarks/v02/opencode/agent/trellis-worker.md",
        agent_dir / "trellis-worker.md",
    )


def fresh_condition_workspace(trial: int, condition: str) -> tuple[Path, Path]:
    base = WORKDIR / f"trial{trial}-{condition}"
    if base.exists():
        shutil.rmtree(base)
    repo = base / "repo"
    shutil.copytree(FIXTURE, repo)
    for pycache in repo.rglob("__pycache__"):
        shutil.rmtree(pycache)
    store = base / "store.db"
    wire_workspace(repo)
    env = cli_env(repo, store)
    run_cli(["init", "--repo", str(repo), "--store", str(store)], env)
    return repo, store


def cli_env(repo: Path, store: Path) -> dict:
    import os

    env = dict(os.environ)
    # opencode resolves its project from PWD; a stale inherited PWD makes
    # the headless run instantiate the wrong project and fail.
    env.pop("PWD", None)
    env.pop("OLDPWD", None)
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


def reset_store(repo: Path, store: Path) -> None:
    """Condition A: nothing may survive from earlier tasks in the chain."""
    store.unlink(missing_ok=True)
    sidecar = store.with_name("cli_state.json")
    sidecar.unlink(missing_ok=True)
    events = store.with_name("events.jsonl")
    events.unlink(missing_ok=True)
    (repo / ".trellis-events.jsonl").unlink(missing_ok=True)
    run_cli(["init", "--repo", str(repo), "--store", str(store)], cli_env(repo, store))


def store_path(repo: Path) -> Path:
    return repo.parent / "store.db"


def run_agent(task_spec: str, repo: Path, model: str, store: Path) -> tuple[dict, float]:
    started = time.time()
    proc = subprocess.run(
        [
            str(OPC), "run", task_spec,
            "--agent", "trellis-worker",
            "-m", model,
            "--auto",
            "--format", "json",
        ],
        cwd=repo,
        capture_output=True,
        text=True,
        timeout=TASK_TIMEOUT_S,
        env=cli_env(repo, store_path(repo)),
    )
    wall = time.time() - started
    metrics = parse_events(proc.stdout)
    metrics["wall_clock_s"] = round(wall, 1)
    metrics["run_failed"] = proc.returncode != 0
    return metrics


def parse_events(stdout: str) -> dict:
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
        "trellis_status": 0,
        "trellis_retrieve": 0,
        "trellis_query": 0,
        "trellis_publish": 0,
    }
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        etype = event.get("type")
        part = event.get("part", {})
        if etype == "step_start":
            metrics["model_calls"] += 1
        elif etype == "step_finish":
            tokens = part.get("tokens", {})
            cache = tokens.get("cache", {})
            metrics["input_tokens"] += tokens.get("input", 0)
            metrics["output_tokens"] += tokens.get("output", 0)
            metrics["cached_read_tokens"] += cache.get("read", 0)
            metrics["cached_write_tokens"] += cache.get("write", 0)
        elif etype == "tool_use":
            tool = part.get("tool", "")
            metrics["tool_ops"] += 1
            if tool == "read":
                metrics["file_reads"] += 1
            elif tool in ("grep", "glob"):
                metrics["repo_searches"] += 1
            elif tool == "bash":
                metrics["shell_cmds"] += 1
            elif tool in ("edit", "write"):
                metrics["edits"] += 1
            elif tool == "trellis_trellis_status":
                metrics["trellis_status"] += 1
            elif tool == "trellis_trellis_retrieve":
                metrics["trellis_retrieve"] += 1
            elif tool == "trellis_trellis_query":
                metrics["trellis_query"] += 1
            elif tool == "trellis_trellis_publish":
                metrics["trellis_publish"] += 1
    return metrics


def trellis_verdicts(store: Path) -> dict:
    """Aggregate the store's events ledger for one condition workspace."""
    events_path = store.with_name("events.jsonl")
    out = {"retrieve_valid": 0, "retrieve_stale": 0, "retrieve_unknown": 0,
           "publish": 0, "validation_us": 0}
    if not events_path.exists():
        return out
    for line in events_path.read_text().splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        op = event.get("op")
        tag = event.get("tag")
        out["validation_us"] += event.get("elapsed_us", 0)
        if op == "retrieve":
            out[f"retrieve_{tag}"] = out.get(f"retrieve_{tag}", 0) + 1
        elif op == "publish":
            out["publish"] += 1
    return out


def run_trial(trial: int, model: str) -> list:
    rows = []
    order = ["A", "B"] if trial % 2 == 0 else ["B", "A"]
    for condition in order:
        repo, store = fresh_condition_workspace(trial, condition)
        for task in TASKS:
            if condition == "A":
                reset_store(repo, store)  # nothing to reuse across tasks
            metrics = run_agent(task["spec"], repo, model, store)
            verdicts = trellis_verdicts(store)
            success = task["verify"](repo)
            rows.append({
                "trial": trial,
                "condition": condition,
                "task": task["id"],
                "success": success,
                **metrics,
                **verdicts,
            })
            print(f"  trial {trial} {condition} {task['id']}: "
                  f"success={success} wall={metrics['wall_clock_s']}s "
                  f"in={metrics['input_tokens']} out={metrics['output_tokens']} "
                  f"toolops={metrics['tool_ops']} "
                  f"(reused={verdicts['retrieve_valid']})")
    return rows


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--trials", type=int, default=5)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--smoke", action="store_true",
                        help="1 trial, T1+T2 only (wiring validation)")
    args = parser.parse_args()
    if args.smoke:
        args.trials = 1
    tasks = TASKS[:2] if args.smoke else TASKS
    globals()["TASKS"][:] = tasks

    if not (ROOT / "target/debug/trellis").exists():
        raise SystemExit("build the CLI first: cargo build -p trellis-cli")
    WORKDIR.mkdir(parents=True, exist_ok=True)
    RESULTS.mkdir(parents=True, exist_ok=True)

    all_rows: list = []
    for trial in range(1, args.trials + 1):
        print(f"=== trial {trial} (order {'A,B' if trial % 2 == 0 else 'B,A'}) ===")
        all_rows.extend(run_trial(trial, args.model))

    suffix = "smoke" if args.smoke else f"n{args.trials}"
    out = RESULTS / f"raw-{suffix}.json"
    out.write_text(json.dumps(all_rows, indent=2))
    print(f"raw rows -> {out}")


if __name__ == "__main__":
    main()

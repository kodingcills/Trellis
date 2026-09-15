#!/usr/bin/env bash
# v0.2 Phase 1 demo: real OpenCode agent + Trellis MCP adapter.
# Proves: (1) an agent publishes a reusable artifact through Trellis,
# (2) after a repository mutation the artifact is rejected as stale with
# an explanation (value withheld), (3) a second agent run recomputes.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MODEL="${TRELLIS_DEMO_MODEL:-cheaperinference/gpt-5.6-luna}"
W="$(mktemp -d /tmp/trellis-demo.XXXXXX)"
FAILURES=0

say() { printf '\n== %s ==\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; FAILURES=$((FAILURES + 1)); }

say "workspace: $W (kept for inspection on failure)"

cargo build -p trellis-cli >&2 || { fail "cargo build"; exit 1; }
mkdir -p "$W/repo"
cp -R "$ROOT/fixtures/python_auth/base/." "$W/repo/"
rm -rf "$W/repo/api/__pycache__" "$W/repo/auth/__pycache__" 2>/dev/null

"$ROOT/target/debug/trellis" init --repo "$W/repo" --store "$W/store.db" || { fail "init"; exit 1; }

# Wire the trial workspace's OpenCode config + fixed agent.
mkdir -p "$W/repo/.opencode/agent"
python3 - "$ROOT" "$W" <<'EOF'
import json, pathlib, shutil, sys
root, w = sys.argv[1], sys.argv[2]
cfg = {"mcp": {"trellis": {"type": "local",
        "command": ["python3", f"{root}/benchmarks/v02/mcp_trellis.py"],
        "enabled": True}}}
pathlib.Path(w, "repo/.opencode/opencode.json").write_text(json.dumps(cfg, indent=2))
shutil.copy(f"{root}/benchmarks/v02/opencode/agent/trellis-worker.md",
            pathlib.Path(w, "repo/.opencode/agent/trellis-worker.md"))
EOF

export TRELLIS_BIN="$ROOT/target/debug/trellis"
export TRELLIS_REPO="$W/repo"
export TRELLIS_STORE="$W/store.db"

TASK='Question: which functions in this repository call the function refresh_token defined in auth/tokens.py, and what arguments do they pass? First run the test suite (python3 -m unittest discover -s tests -t . 2>&1 | tail -5). Then answer with the caller list, and publish it with trellis_publish (key "auth.tokens.refresh_token", kind "callers", value = the caller names separated by "; ").'

say "agent run 1 (publish expected)"
OPC="$HOME/.opencode/bin/opencode"
RUNWRAP=()
command -v timeout >/dev/null 2>&1 && RUNWRAP=(timeout 900)
opencode_run() {
  (cd "$W/repo" && ${RUNWRAP[@]+"${RUNWRAP[@]}"} "$OPC" run "$1" --agent trellis-worker -m "$MODEL" --auto 2>&1 | tail -25)
}
OUT1="$(opencode_run "$TASK")" || fail "opencode run 1 exited nonzero"
printf '%s\n' "$OUT1"
if ! grep -q '"op":"publish"' "$W/events.jsonl"; then
  fail "no publish event after run 1"
fi

say "mutate repository: add a caller of refresh_token"
python3 - "$W/repo" <<'EOF'
import pathlib, sys
p = pathlib.Path(sys.argv[1], "api/routes.py")
s = p.read_text()
if "tok.refresh_token(" not in s:
    s = s.replace(
        "from auth.service import AuthService",
        "from auth.service import AuthService\nfrom auth import tokens as tok",
    )
    lines = s.split("\n")
    for i, l in enumerate(lines):
        if l.startswith("def "):
            lines.insert(i + 1, '    _sid = tok.refresh_token("probe")')
            break
    p.write_text("\n".join(lines))
print("routes.py now calls tok.refresh_token")
EOF

say "deterministic check: retrieve must reject the stale artifact"
STALE_JSON="$("$ROOT/target/debug/trellis" retrieve --repo "$W/repo" --store "$W/store.db" --key auth.tokens.refresh_token)"
printf '%s\n' "$STALE_JSON"
case "$STALE_JSON" in
  *'"verdict":"stale"'*) : ;;
  *) fail "expected stale verdict after mutation" ;;
esac
case "$STALE_JSON" in
  *'"value"'*) fail "stale retrieve must not carry a value" ;;
  *) : ;;
esac

say "agent run 2 (recompute expected)"
OUT2="$(opencode_run "$TASK")" || fail "opencode run 2 exited nonzero"
printf '%s\n' "$OUT2"

say "metrics ledger"
tail -12 "$W/events.jsonl"

QUERIES_RUN2=$(grep -c '"op":"query"' "$W/repo/.trellis-events.jsonl" 2>/dev/null || echo 0)
say "result"
if [ "$FAILURES" -eq 0 ]; then
  echo "DEMO PASS: publish -> stale rejection with explanation -> agent recomputed"
  echo "fresh query events recorded in repo ledger: $QUERIES_RUN2"
else
  echo "DEMO FAIL: $FAILURES failure(s)"
fi
exit "$FAILURES"

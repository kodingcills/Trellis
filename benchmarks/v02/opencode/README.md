# v0.2 OpenCode wiring

A trial workspace wires the Trellis MCP server + fixed agent like this:

1. Build the CLI: `cargo build -p trellis-cli`.
2. Create a trial copy of the target repo and a store:
   `trellis init --repo <trial-repo> --store <trial-store>/metadata.db`.
3. In the trial workspace root, create `.opencode/` containing:
   - `opencode.json` — copied from `benchmarks/v02/opencode/opencode.json`
     (paths are relative to the Trellis checkout; run opencode from the
     checkout root or rewrite them absolute).
   - `agent/trellis-worker.md` — copied from
     `benchmarks/v02/opencode/agent/trellis-worker.md`.
4. Export per-trial env before `opencode run`:
   - `TRELLIS_REPO` = absolute path of the trial repo copy.
   - `TRELLIS_STORE` = absolute path of the trial store db.

The adapter (`benchmarks/v02/mcp_trellis.py`) is condition-neutral: it
exposes the same four tools (status/retrieve/query/publish) to every
condition. Conditions differ ONLY in whether the store carries artifacts
from earlier tasks in the chain (baseline resets the store per task;
trellis persists it across the chain).

The agent prompt is fixed and identical across conditions. It instructs
the agent to check/retrieve artifacts, recompute on stale/unknown, and
publish reusable results — with no hint about whether reuse should help.

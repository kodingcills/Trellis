# .agent/runs/ — bounded engineering session logs

One directory per node, one file per session:

    .agent/runs/<NODE>/YYYY-MM-DD-<slug>.md

Required sections (per `CONTROLLER.md`):

    # Run <date> <node>
    orient: repo state, STATE, node contract, spec sections read
    plan: bounded slice — files, interfaces, tests (incl. oracle-first tests)
    execute: what was built, worker delegations used
    verify: gate results (GLOBAL_GREEN + node gates), verbatim
    review: findings summary + link to review record
    repair: blocker/dispositioned-IMPORTANT fixes; attempt counter
    commit: hash + message; STATE/ROADMAP updates made

Rules:
- Sessions are bounded: exit at a green state or a recorded escalation point.
- Update `STATE.json` at session end — `last_gate_result`, `next_candidates`, node status.
- Attempt counters for repeated failures live in the run log (REPAIR_POLICY).

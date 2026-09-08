# AGENTS.md — Trellis

## What this is

Trellis is a provenance-aware incremental-computation runtime for coding agents: it maintains agent-derived artifacts against an evolving repository state so agents reuse still-valid work and recompute only what changed. The core mechanism is typed **dependency projections** + **append-only validation attestations** + **red/green reevaluation** with a value-equality cutoff. v0.1 is a Rust CLI monolith (`crates/`); Python benchmark harness arrives later. One language (Python) first.

## Authority (highest first)

1. **`docs/architecture/TRELLIS_V0_1_FROZEN.md`** — the implementation specification (single authoritative copy of the frozen Draft 1.0). `§n` references point here. **Never modify it.** If implementation evidence contradicts it, record an `ARCHITECTURE_CONFLICT` in `.agent/STATE.json` and stop that branch.
2. `.agent/ROADMAP.yaml` — milestone contracts (the DAG).
3. `.agent/QUALITY_GATES.yaml` — what "green" means, finding severities.
4. `.agent/REVIEWER.md` — reviewer independence, verdict vocabulary, post-repair review closure.
5. `.agent/CONTROLLER.md` — the engineering loop, commit authorization, remote git policy, termination/escalation rules.
6. Individual worker judgment — lowest.

## Where state lives

All harness state below is **version-controlled** — a fresh clone of any green commit recovers the full engineering process.

- Controller state: `.agent/STATE.json`
- Milestone DAG + per-node contracts: `.agent/ROADMAP.yaml`
- Reviewer protocol: `.agent/REVIEWER.md`
- Non-blocking ideas: `.agent/BACKLOG.md`
- Append-only implementation decisions: `.agent/DECISIONS.md`
- Review records: `.agent/reviews/` — run logs: `.agent/runs/`

## Identifying the current task

Read `.agent/STATE.json` → `next_candidates` → open that node's contract in `ROADMAP.yaml`. Work **one node at a time**. Never invent milestones; never pull backlog items onto the critical path.

## Green commands (required before any change is "done")

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Plus the active node's own acceptance gates (`ROADMAP.yaml` → node → `gates`). A commit is a **verified green→green transition**, never a progress save. Commit requires a fresh reviewer's `COMMIT_ALLOWED` against the exact candidate state (`.agent/REVIEWER.md`). Local commits are autonomous; **push and all other remote mutations require explicit standing human permission** (`.agent/CONTROLLER.md` → Remote git policy).

## Forbidden architectural shortcuts

- No mutable epistemic state on artifacts or attestations; attestations never transition — append only (§10, §12).
- No daemon, no filesystem watcher, no continuous dirty tracking in v0.1 (§5).
- No SCIP-specific knowledge outside the SCIP adapter/certificate (§8).
- No LLM as a validity oracle (§30).
- No weakening of the reuse-policy floor: Valid ∧ Authoritative ∧ ¬Unobserved (§15).
- No engine sophistication before the oracle fixture's expected behavior is written (§33 oracle-first).
- No new dependencies in `trellis-core` without a `.agent/DECISIONS.md` entry (§24 crate layering).
- No benchmark conditions with unequal tool capability (§25 same-logical-tool-API).

## Deferred ideas

Go to `.agent/BACKLOG.md`. Backlog items must never keep a milestone loop alive.

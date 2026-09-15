# Trellis v0.2 Benchmark Report — Real-Agent Reuse Experiment

Date: 2026-09-15 · Model: `cheaperinference/gpt-5.6-luna` (temperature 0) ·
Harness: OpenCode 1.18.29 headless (`opencode run --auto --format json`) ·
Repository: `fixtures/python_auth/base` (35-file Python auth package, 18
passing unittest tests) · Raw rows: `benchmarks/v02/results/raw-*.json`.

## Question

Can a real coding agent reuse validated prior computation through Trellis
across related tasks — reducing recomputation/rediscovery — without
reducing task success?

## Setup

Five distinct but repository-coupled tasks (T1 PBKDF2 hardening → T2
account lockout → T3 login metrics → T4 refresh-token endpoint → T5 test
audit). Conditions:

- **A (baseline):** Trellis store reset before every task (nothing to reuse).
- **B (trellis):** store persists across the task chain (validated reuse).

Controls: same model, same fixed agent prompt, same MCP tools, same CLI,
same repo start state, same 900s timeout, ≥5 paired trials with
alternating condition order. The independent variable is artifact
persistence only; `trellis_query` is always fresh in both conditions.

Metrics: JSON event stream (tokens, model calls, tool mix) + CLI events
ledger (publish/retrieve verdicts, validation microseconds) +
deterministic per-task verifiers (structural assertions + full unittest
suite).

## Engineering history (before/after discipline)

| Run | Change | Result |
|---|---|---|
| 1 | initial harness | 0/43 publishes landed — agents guessed publish arguments; prompt lacked the contract |
| 2 | prompt documents publish contract | 0 publishes: `FOREIGN KEY constraint failed` — publish seeded observations against a snapshot id that was never inserted after agent edits |
| 3 | publish reconciles first (fix) | first valid signal: 46 valid reuses, 0 stale, capture works |
| 4 | Trellis calls made optional in prompt | agents ignore Trellis (1 status call, 0 publishes) → intervention reverted |

## Results (run 3, primary; n=25 task-runs per condition)

| metric | A baseline | B trellis |
|---|---|---|
| task success | 17/25 (68%) | 22/25 (88%) |
| wall clock mean/task | 47.4s | 63.6s |
| input tokens total | 1,234,066 | 1,447,061 |
| tool ops total | 496 | 577 |
| valid artifact reuses | 0 | 46 |
| stale rejections | 0 | 0 |
| unknown verdicts | 18 | 65 |
| publishes landed | 21 | 79 |
| validation time total | 0.96s | 4.53s |

Per-task deltas (B − A): T1 +9s, T2 +36s, T3 +26s, T4 +25s, **T5 −15s**.
T5 (the cross-task-coupled test audit) is the only task where reuse made
B cheaper — and it is also where reuse counts were highest (18 valid
reuses across 5 trials).

## Honest conclusion

**Trellis reuse works mechanically (capture, validation, stale rejection)
but does not yet pay for itself on this task chain.** Specifically:

1. Correctness held: B never regressed success (88% vs 68%; small n, but
   the primary gate is "must not regress" and it passed in every run).
2. Reuse happened (46 valid retrievals, 0 false-valid stale reuse), and
   the coupled task (T5) showed the predicted negative-cost signature.
3. But mid-chain tasks cost MORE with Trellis: the mandatory
   status/retrieve/publish ceremony adds model calls and tokens that the
   avoided recomputation does not yet offset. Validation itself is cheap
   (4.5s total vs ~9min of extra wall time) — the overhead is agent
   interaction cost, not engine cost.
4. When usage is optional, agents ignore the tools (run 4): 1 status
   call, 0 publishes across 25 runs. Artifact availability alone does
   not change agent behavior; the benefit only appears when usage is
   mandated, and then the ceremony overhead dominates.

This hits two of the campaign's kill conditions honestly: "agent behavior
does not materially change despite artifact availability" (optional
condition) and "validation overhead erases saved computation" (net
negative on 4 of 5 tasks). We report it rather than tune the benchmark.

## Limitations

- n=5 paired trials per condition; success-rate differences (68% vs 88%)
  are within noise for this n. Token/wall deltas are consistent in
  direction across 4 of 5 trials.
- One model, one small synthetic repository, one task chain. No OSS
  validation was run: with a net-negative result on the primary chain,
  extending the same protocol to a pinned OSS repo would multiply cost
  without changing the conclusion; the Phase-3 analysis says the next
  engineering step is reducing ceremony cost (e.g. batched
  status+retrieve, cheaper publish), not scaling the experiment.
- The approximate textual caller resolver is labeled and identical in
  both conditions; it is not a semantic-quality claim.
- Provider prompt-cache tokens (≈80% of input) are recorded separately
  and NOT credited to Trellis; cached-read totals were actually higher
  in B (more steps), reinforcing the overhead finding.

## What would change the verdict

- Ceremony reduction: one call returning all valid artifacts for the
  working set (replaces status+retrieve loops), lazy publish (only on
  explicit novelty) — projected to cut most of B's +2.3 model calls/task.
- Higher-coupling task chains: T5's negative cost suggests chains with
  denser cross-task dependency structure are where Trellis wins.

## Artifacts

- `benchmarks/v02/run_trials.py` — harness (paired protocol, verifiers).
- `benchmarks/v02/analyze.py` — aggregation.
- `benchmarks/v02/results/raw-*.json` — raw rows for all four runs.
- `benchmarks/v02/demo_reuse.sh` — reuse + stale-rejection demo (PASS).
- `crates/trellis-cli` — the adapter CLI (workspace-gated, 203 tests).

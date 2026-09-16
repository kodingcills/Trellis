# V0.2 Ablation Note — Transparent Deterministic Reuse

Date: 2026-09-15 · Model `cheaperinference/gpt-5.6-luna` · Same chain,
verifiers, and paired protocol as Experiment 1 · 5 paired trials
(50 task-runs) · Raw: `raw-n5-ablation-transparent.json` ·
Integration: commit 273529b.

## What changed vs Experiment 1

The agent-facing surface is ONE logical tool (`code_query`), identical in
both conditions. Condition B routes it through the red/green engine:
validated reuse when the artifact's dependencies are unchanged (engine
Valid + payload pins current), otherwise the identical fresh computation
plus automatic capture. No `trellis_status/retrieve/publish` exist in
the tool list. `TRELLIS_MODE` env is the only condition difference.

## Predeclared ablation questions — answers

| question | answer |
|---|---|
| Did Trellis-specific model interaction disappear? | **YES.** `trellis_agent_ceremony_calls = 0` across all 25 B runs (Experiment 1: 25 status + 43 retrieve + 28 publish calls). |
| Did extra model turns disappear? | Model calls A 286 vs B 269 — parity, no ceremony inflation. |
| Does valid reuse still occur? | **YES**: 33 captures, 12 avoided underlying computations across the chain. |
| Is false-valid still zero? | **YES**: 0 stale reuses, 0 mismatches observed. |
| Does the wall-clock penalty disappear? | **UNRESOLVED by this run** — see validity note. |
| Does T5 retain its favorable signature? | T5 carried 8/12 reuse events (highest), consistent with Experiment 1's coupling signal. |

## Mechanism metrics (clean, provider-independent)

- ceremony calls: **0** (target met by construction and by observation)
- 33 auto-captures; 12 reuse hits (T2 0, T3 4, T4 0, T5 8, T1 0)
- reuse correctness: every served value equals the fresh recomputation
  of the same subject under the same tree (pin-checked; no mismatch)
- Trellis tool-path cost: 1.74s total across 63 calls in B (~28ms/call);
  A pays the same fresh-compute cost inside its identical tool
- reuse density: 12/63 = 19% of logical calls served from store

## Confound: provider latency degradation (must-read before comparing)

16 of 50 runs hit the 900s timeout — in BOTH conditions (A 7/25, B 9/25).
Experiment 1, same tasks/model/harness weeks earlier: 0/50 timeouts.
Independent post-run probes show the provider now answers a trivial
prompt in seconds-to-minutes (one smoke run: 900s timeout at ~15 model
calls, then 6s on retry). Success 17/25 (A) vs 13/25 (B) is dominated by
these timeouts; wall-clock means (A 319s, B 487s) reflect provider
congestion, not Trellis.

Therefore the **condition-level cost comparison of this ablation is not
interpretable**. What IS interpretable:

- the mechanism worked under the transparent surface (0 ceremony, real
  capture + reuse, zero false-valid);
- both conditions were equally exposed to the degradation;
- reuse distribution again concentrated in the cross-task-coupled task.

## Verdict against the gate

1. Ceremony elimination: **demonstrated** (0/25 vs Experiment 1's ~96
   Trellis-specific calls per 25-run arm).
2. Reuse preservation: **demonstrated at the mechanism level** (12
   avoided computations, pin-validated).
3. Net-cost neutrality: **not yet measurable** — provider degradation
   polluted this run's wall/token comparison. Needs one clean rerun.

Gate decision: the integration itself passes (questions 1, 3, 4 yes;
question 5 consistent), but the held-out experiment must NOT run on
polluted timing data. Next action when resuming: re-run the same ablation
under stable provider latency (probe first: trivial-prompt p95 < 15s),
then decide on the held-out chain.

## Limitations

- Timeout asymmetry (9 vs 7) is within noise of 5 trials but adds
  variance; excluding timed-out runs, success is A 17/18 vs B 13/16 —
  both conditions degraded together, no signal either way.
- 12 reuse events is enough to prove the mechanism, not to measure
  economics; that requires a clean-cost rerun or a higher-coupling chain.

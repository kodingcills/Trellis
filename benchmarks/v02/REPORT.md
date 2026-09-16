# Trellis v0.2 Benchmark Report — Real-Agent Reuse Experiment

Date: 2026-09-15 (Exp 1) / 2026-09-16 (Exp 3) · Model:
`cheaperinference/gpt-5.6-luna` (temperature 0) ·
Harness: OpenCode 1.18.29 headless (`opencode run --auto --format json`) ·
Repository: `fixtures/python_auth/base` (35-file Python auth package, 18
passing unittest tests) · Raw rows: `benchmarks/v02/results/raw-*.json`.
Review status: independently audited (oracle, REVISIONS_REQUIRED); the
corrections below are applied. Campaign status: **Experiment 3 recorded;
Experiment 4 protocol frozen below, pair-scoped provider health added to
catch the mid-run degradation that invalidated Experiment 3's cost
comparison.**

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
same repo start state, same 900s timeout, 5 paired trials with
alternating condition order (3 B-first, 2 A-first — approximately
balanced). The independent variable is artifact persistence only;
`trellis_query` is always fresh in both conditions.

Metrics: JSON event stream (tokens, model calls, tool mix) + CLI events
ledger. Success verifiers are deterministic structural assertions + full
unittest suite — acceptance proxies, not independently authored hidden
tests. Ledger metrics are de-cumulated per trial (the store ledger is
cumulative across the chain; earlier drafts of this report summed
cumulative rows — corrected here after audit).

Measurement caveat: the ledger's `retrieve_valid` tag records the
engine's attestation verdict BEFORE the CLI's payload-pin check, which
can downgrade the served response to `unknown`. Counts below are
therefore "engine-valid retrieval verdicts", an upper bound on values
actually served; agent-side consumption of served values is not directly
observed.

## Engineering history (before/after discipline)

| Run | Change | Result |
|---|---|---|
| 1 | initial harness | 0/43 publishes landed — agents guessed publish arguments; prompt lacked the contract |
| 2 | prompt documents publish contract | 0 publishes: `FOREIGN KEY constraint failed` — publish seeded observations against a snapshot id that was never inserted after agent edits |
| 3 | publish reconciles first (fix) | first valid signal: captures and reuse occur |
| 4 | Trellis calls made optional in prompt | agents ignore optional Trellis tooling (1 status call, 0 publishes across 25 runs) → intervention reverted |

## Results (run 3, primary; n=25 task-runs per condition; ledger
metrics de-cumulated)

| metric | A baseline | B trellis |
|---|---|---|
| task success (aggregate) | 17/25 (68%) | 22/25 (88%) |
| paired outcomes | 6 A-fail → B-pass | 1 B-fail → A-pass |
| wall clock mean/task | 47.4s | 63.6s |
| wall clock per trial | B slower in 5/5 trials | |
| input tokens total | 1,234,066 | 1,447,061 |
| tool ops total | 496 | 577 |
| publishes landed (ledger) | 6 | 28 |
| engine-valid retrieval verdicts | 0 | 18 |
| stale rejections | 0 | 0 |
| unknown verdicts | 2 | 25 |
| Trellis CLI operation time (mixed ops) | 0.24s | 1.59s |

De-cumulated engine-valid verdicts by task (B): T2 4, T3 8, T4 0, T5 6.

## Honest conclusion

**Trellis reuse works mechanically (capture, validation, zero stale
reuse), but the current mandatory-ceremony integration costs more than
it saves on this task chain; and when usage is optional, agents do not
adopt the tooling at all.**

1. Correctness: aggregate success was higher in B (88% vs 68%), but this
   is n=25 per arm with 6 paired improvements vs 1 paired regression —
   too small to establish non-inferiority, and the single paired
   regression (trial 5, T2) means the primary gate "must not regress"
   is not literally met.
2. Capture and validity work: 28 publishes landed, 18 engine-valid
   retrievals, 0 stale rejections, 0 false-valid reuse observed.
3. The wall-clock result is robust: B was slower in every one of the 5
   trial-level pairs. The supported diagnosis is that the mandatory
   integration's total agent-interaction overhead (extra model calls and
   larger prompts) exceeds measured savings on 4 of 5 task types. Engine
   validation compute itself is small (1.59s of mixed CLI operation time
   across B's whole chain) — the loss is interaction cost, not
   validation cost, and those imply different product fixes.
4. T5 — the cross-task-coupled task — showed the predicted negative-cost
   signature (B cheaper: −15s mean, −1 model calls, −2.2 file reads) and
   carried 6 of the 18 valid retrievals. Coupling direction is a
   plausible effect, not an established one at this n.
5. With optional usage (run 4), agents made 1 status call and 0
   publishes in 25 runs — so no artifacts existed downstream and no
   reuse was possible. This run demonstrates tool-uptake failure, NOT
   that agents ignore available artifacts; the latter experiment was not
   run.

Against the campaign's kill conditions: the supported finding is
"interaction overhead exceeds savings on this chain" — a ceremony-design
problem, not evidence that validation or reuse is intrinsically
valueless. We report it rather than tune the benchmark.

## Experiment 3 — clean transparent rerun (2026-09-16, n=50 task-runs)

Protocol identical to the first transparent ablation (commit 273529b
surface; five tasks, verifiers, 900s timeout, alternating order, 5
paired trials). No task, tool, prompt, or analysis definition was
altered. Raw: `results/raw-n5.json`. One change to the harness only:
per-trial checkpointing of raw rows, plus a predeclared provider-health
preflight (`provider_health.py`, gate: trivial-prompt p95 < 15s, 0
failures, N=10) that aborts before any trial if the provider is
degraded; aborted runs are not trials. Probe reports:
`results/provider_health-*.json`.

**Validity warning: the provider degraded mid-experiment again.** The
preflight passed at launch (p50 5.0s, p95 6.0s, 10/10 probes) and
conditions were equally exposed, but timeouts recurred throughout:
19/50 runs hit the 900s cap (A 7, B 12), concentrated in T2–T5 late in
trials. Timeout counts by task: T1 0/10, T2 7/10, T3 5/10, T4 4/10,
T5 3/10. Condition-level wall-clock and success totals are therefore
**not interpretable as a Trellis effect**; the decomposition below is
the honest reading.

### Headline (unadjusted — polluted, shown for completeness)

| metric | A baseline | B trellis |
|---|---|---|
| task success | 16/25 (64%) | 12/25 (48%) |
| wall clock mean/task | 357.7s | 600.9s |
| 900s timeouts | 7/25 | 12/25 |
| ceremony calls | 0 | 0 |

### Decomposition (timeout-polluted runs excluded where stated)

On the 31 healthy runs (A 16, B 15):

| paired metric (B−A per trial) | diffs | median | mean |
|---|---|---|---|
| wall clock (healthy only) | +131, −120, +644, −199, −572 | −120s | −23s |
| input tokens (healthy only) | +36k, −117k, +11k, −187k, −453k | −117k | −142k |
| model calls (healthy only) | +17, −19, −4, −30, −45 | −19 | −16.2 |
| success (healthy only) | 0, −1, 0, −1, −3 | −1 | −1.0 |

On all 50 runs (for the record): input tokens B−A negative in 5/5
trials (median −83k), model calls median −14, tool ops median −29,
file reads median −11.

Mechanism metrics (clean, provider-independent):

- ceremony calls: **0/25** in B (gate met by construction and observation)
- 34 auto-captures, **11 avoided underlying computations** (trial
  distribution: T5 5, T3 2, T1 1 across trials; 8/11 in T3+T5, the
  cross-task-coupled tasks — consistent with Exp 1 and the first
  transparent ablation)
- 0 stale reuses, 0 false-valid reuse observed
- Trellis tool-path time 1.97s total across 64 logical calls in B
  (~31ms/call); A pays the same fresh-compute cost inside its identical
  tool

### Honest reading

1. **Timeout asymmetry (12 vs 7) is the dominant signal in the raw
   table and it is not attributable to Trellis by construction** — the
   only condition difference is whether the store persists, and the
   tool surface is byte-identical. But we cannot rule out that B's
   marginally different run trajectories (e.g. fewer fresh recomputes →
   different context growth) interact with provider latency variance.
   At n=5 trials this asymmetry is within noise; we report it rather
   than explain it away.
2. **Where runs completed, Trellis was never more expensive at the
   trial level on work metrics**: median −19 model calls, −117k input
   tokens, −120s wall per trial on healthy runs. The healthy wall
   median is negative but the mean is ≈0 (trial 3's +644s outlier);
   the honest claim is cost-neutral-to-slightly-favorable, not a
   demonstrated win.
3. **The success asymmetry on healthy runs (paired −1/trial) tracks the
   timeout asymmetry**: B's failed-but-not-timed-out rows include the
   trial-5 T5 audit task where B timed out in 3/5 trials and A in 0.
   With success deltas this size at n=5, no correctness claim is
   supported in either direction.
4. **Reuse remains real, transparent, and safe**: 11 avoided
   computations, concentrated in coupled tasks, zero false-valid.
   Reuse density 11/64 ≈ 17% of logical calls.

### Gate decision

The predeclared gate for proceeding to the held-out chain was "clean
cost evidence under stable provider conditions." This run did not
achieve it: 38% timeout rate despite a passing preflight. The mechanism
questions (ceremony elimination, reuse preservation, zero false-valid)
are now answered three times consistently across experiments. What
remains unmeasured is the economic question, and that requires a
provider window substantially more stable than anything observed today
or during the first transparent ablation. Next action when resuming:
re-run this exact frozen experiment in a verified-stable window
(preflight + mid-run provider spot-checks), before any held-out work.

## Experiment 4 — pair-scoped provider health (protocol frozen 2026-09-16)

Same tasks (T1-T5), verifiers, tool surface (`code_query` only, byte-
identical schema across conditions), timeout (900s), and alternating
A/B order as Experiments 1/3. Only the health-validity instrumentation
changed, motivated directly by Experiment 3's failure mode: a single
upfront preflight passed, then the provider degraded mid-run (19/50
timeouts), making the condition-level cost comparison uninterpretable.

**Protocol** (`run_trials.py::run_experiment4`, `provider_health.py`):

```
for each pair attempt (max 8):
    pre-pair sentinel:  3 fixed probes, same model, repo-independent
        prompt "Reply with exactly the word OK and nothing else."
        gate: every probe succeeds AND every latency < 15s
        (N=3 nearest-rank p95 == max, so this reuses the existing
        evaluate_gate() check unmodified — no separate percentile math)
    if pre-pair gate fails:
        do not start the pair
        record health_invalid_reason = PROVIDER_HEALTH_FAILED_BEFORE_PAIR
        STOP this execution window (resume later, same protocol)
    else:
        run the real A/B pair (5 tasks each condition, order alternates
        by attempt id, same as Experiments 1/3)
        post-pair sentinel: same 3-probe gate
        if post-pair gate fails:
            keep all raw condition data
            record health_invalid_reason = PROVIDER_HEALTH_FAILED_AFTER_PAIR
            STOP this execution window
        else:
            pair is health_valid = true
    checkpoint the full attempt list to
        results/pairs-exp4-stable-provider.json after every attempt
stop when: 5 health-valid pairs accrued, OR 8 attempts exhausted
```

Health validity is decided from the sentinels alone, never from wall
time/token/success deltas (`classify_pair_health`, unit-tested). A task
hitting the 900s timeout is a valid treatment outcome in either
condition and does **not** by itself make a pair health-invalid
(`timed_out` is tracked per task row, separate from provider health).

**Data model**: each pair attempt is a `PairAttempt` record (attempt
id, protocol commit, provider/model/config, order, pre/post health
reports, per-condition `ConditionResult` summaries + raw per-task rows,
health_valid, health_invalid_reason). `ConditionResult.stale_withholding`,
`.unknown_results`, and `.false_valid_reuse` are recorded as `null`, not
`0` — this integration's transparent path only ever serves a result
after verifying its dependency pins are current against the live tree,
so a stale/unknown serve is not independently observable as a distinct
event on this path (raising `null` rather than fabricating `0` follows
the same rule Trellis itself applies to incomplete coverage). The
safety claim for this integration remains: 0 stale reuse and 0
false-valid reuse observed across three prior experiments by
construction of the pin check, audited manually, not by a per-run
counter.

**Analysis** (`analyze.py::analyze_pairs`): primary aggregate is over
health-valid pairs only; health-invalid pairs are printed separately
and never silently dropped from the raw file. Reports every paired
diff (B-A) plus median/mean per metric (Sec 15), and per-task pass/fail
discordance across valid pairs. No formal statistics beyond
median/mean at n=5 — magnitude and consistency only.

**Decision criteria** (frozen, Sec 16 of the v0.2 directive; unchanged
by results): FAVORABLE requires >5% median wall-time improvement, ≥3/5
pairs favoring Trellis, no systematic correctness regression, and zero
false-valid reuse. COST-NEUTRAL requires median wall-clock within ±5%
and at least two of {model calls, input tokens, repo ops, underlying
computations} favoring Trellis, same correctness/safety bars. Anything
short of these, or a >5% regression with ≥3/5 pairs favoring Fresh, is
GENUINELY NEGATIVE; anything else is MIXED/INCONCLUSIVE. This section
was written and committed before any Experiment 4 pair was run.

Protocol commit: see git history at the commit introducing this
section (`bench(v02): freeze pair-scoped provider health protocol`).
Tests: `test_provider_health.py`, `test_run_trials.py`, `test_analyze.py`.

### Results

*(pending — filled in after the accrual run completes or exhausts the
8-attempt cap; see `results/pairs-exp4-stable-provider.json`)*

## Limitations

- n=5 paired trials; all success-rate figures are within noise. The
  wall-clock penalty is the only result consistent across every trial.
- One model, one small synthetic repository, one task chain.
- **OSS validation not run — campaign paused before the
  external-validity gate.** With a net-negative primary result, scaling
  the protocol to a pinned OSS repo multiplies cost without resolving
  the diagnosed bottleneck; the direction could differ on larger repos
  (higher artifact value AND higher ceremony overhead) and is empirical.
- The approximate textual caller resolver is labeled and identical in
  both conditions; it is not a semantic-quality claim.
- Provider prompt-cache reads (~80% of input tokens) are recorded
  separately and not credited to Trellis; B's cached reads were higher
  (more steps), reinforcing the overhead finding.
- Input-token totals measure the complete treatment (mandated tool
  turns, larger status outputs), not engine validation or reuse in
  isolation; differing task success also alters run length, so cost
  totals are a product metric, not a clean causal measure of avoided
  recomputation.

## What would change the verdict

- Ceremony reduction: one call returning all valid artifacts for the
  working set (replaces status+retrieve loops), lazy publish — targets
  the measured +2 to +4 model calls/task on mid-chain tasks.
- A genuine optional-availability experiment (mandate capture upstream,
  optional retrieval downstream, pre-seeded stores) before claiming
  anything about artifact-driven behavior.
- Higher-coupling task chains: T5's negative cost suggests chains with
  denser cross-task dependency structure are where Trellis wins.

## Artifacts

- `benchmarks/v02/run_trials.py` — harness (paired protocol, verifiers,
  preflight gate, per-trial checkpointing).
- `benchmarks/v02/provider_health.py` — predeclared provider-health
  preflight (gate + unit tests in `test_provider_health.py`).
- `benchmarks/v02/analyze.py` — aggregation (ledger de-cumulation added
  post-review).
- `benchmarks/v02/results/raw-*.json` — raw rows for Experiments 1-3.
- `benchmarks/v02/results/pairs-exp4-stable-provider.json` — Experiment
  4 PairAttempt records (immutable, checkpointed per attempt).
- `benchmarks/v02/results/provider_health-*.json` — preflight probe
  reports (why a run proceeded or aborted); `-pair<N>-pre/post.json`
  are Experiment 4's pair-scoped sentinels.
- `benchmarks/v02/test_provider_health.py`, `test_run_trials.py`,
  `test_analyze.py` — health-classification, accrual, and
  inclusion/exclusion regression tests (no live provider).
- `benchmarks/v02/demo_reuse.sh` — agent-driven publish + deterministic
  CLI stale-withholding smoke test (PASS; the stale-rejection assertion
  is CLI-level, not agent-level).
- `crates/trellis-cli` — the adapter CLI (workspace-gated, 203 tests).

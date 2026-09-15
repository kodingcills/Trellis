# Trellis v0.2 Benchmark Report — Real-Agent Reuse Experiment

Date: 2026-09-15 · Model: `cheaperinference/gpt-5.6-luna` (temperature 0) ·
Harness: OpenCode 1.18.29 headless (`opencode run --auto --format json`) ·
Repository: `fixtures/python_auth/base` (35-file Python auth package, 18
passing unittest tests) · Raw rows: `benchmarks/v02/results/raw-*.json`.
Review status: independently audited (oracle, REVISIONS_REQUIRED); the
corrections below are applied. Campaign status: **paused before the
external-validity (OSS) gate**.

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

- `benchmarks/v02/run_trials.py` — harness (paired protocol, verifiers).
- `benchmarks/v02/analyze.py` — aggregation (ledger de-cumulation added
  post-review).
- `benchmarks/v02/results/raw-*.json` — raw rows for all four runs.
- `benchmarks/v02/demo_reuse.sh` — agent-driven publish + deterministic
  CLI stale-withholding smoke test (PASS; the stale-rejection assertion
  is CLI-level, not agent-level).
- `crates/trellis-cli` — the adapter CLI (workspace-gated, 203 tests).

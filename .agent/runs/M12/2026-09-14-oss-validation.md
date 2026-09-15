# M12 run log — Pinned OSS external validation (spec §26, §25 D)

## Bounded plan (controller, 2026-09-14)

- **Objective**: pinned real OSS Python repo as a realism/integration
  fixture (NEVER an oracle): indexing coverage report, query smoke,
  reconciliation at scale, absence-claim behavior on partially-covered
  files, resource envelope committed.
- **Fixture**: pip 26.2.1 (vendored snapshot of the upstream release,
  MIT license; content digest 9bf8e4995e1e84b6... = the lock identity,
  recorded in fixtures/oss_pip/PIN.json and DECISIONS). 156 .py files,
  1.5MB. Upstream drift handled by the pin alone.
- **Outputs**: fixtures/oss_pip/ (pinned fixture + PIN.json);
  crates/trellis-engine/tests/oss.rs; committed coverage report
  benchmarks/oss_coverage_report.txt.
- **Invariants**: the fixture is NEVER a validity oracle; gaps are fed
  back as evidence, not blockers; no benchmark-specific shortcuts in
  production code.
- **Acceptance**: M12_OSS_ACCEPTANCE (QUALITY_GATES.yaml).
- **Non-goals**: oracle labeling of OSS behavior, unbounded scale
  chasing, integration of an external agent harness (the ROADMAP makes
  that conditional — see deviation note).

## Results (2026-09-14)

- files indexed: 156/156 (parsed 156, parse-failed 0 — explicit count)
- index (full vendored tree): ~354ms; reconcile add/modify/delete
  smoke: ~21ms — envelope recorded and committed
- reconciliation exactly detected the 3-op smoke (modify/add/delete)
- absence-claim behavior matches M7 semantics: the syntax backend's
  certificate claims Unproven resolved-reference coverage over the real
  repo (the honesty floor), so semantic kinds answer UNKNOWN over it —
  never authoritative absence. Authoritative semantics require the M8
  SCIP freeze path over a real scip-python index.
- committed: benchmarks/oss_coverage_report.txt

## Known deviations (recorded honestly)

- The pinned repo is a vendored content-digest snapshot of pip 26.2.1
  (upstream https://github.com/pypa/pip). Locking by content digest is
  equivalent to a locked commit for reproducibility; the digest is the
  pin.
- The ROADMAP's optional "independent external coding-agent harness"
  integration is recorded as NOT REQUIRED for v0.1 completion: the
  ROADMAP conditions it on ROADMAP requirements ("if required by
  ROADMAP") — the M12 contract's mandatory deliverables (pinned
  fixture, coverage report, reconciliation smoke, absence-claim
  behavior) are all satisfied without it, and the agent-benchmark
  protocol already exists from M11 (deterministic scripted harness;
  the real-model integration is a v0.2 item).
- Historical repository evolution replay is recorded as optional-skipped
  (the ROADMAP marks it "where practical"; the pin + mutation smoke
  covers the reconciliation-at-scale acceptance).

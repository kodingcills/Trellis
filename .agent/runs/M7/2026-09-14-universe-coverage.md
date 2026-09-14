# M7 run log — Universe + coverage certificates (spec §8)

## Bounded plan (controller, 2026-09-14)

- **Objective**: completeness-sensitive query support per §8 — every
  authoritative absence/completeness-sensitive observation binds to
  (Q, U, C): query, explicit eligible universe, coverage certificate.
  Requirements/capabilities split (Pin 2) with a backend-independent
  CompletenessEvaluator; universe/coverage changes dirty the query for
  reevaluation, never immediate downstream destruction.
- **Inputs**: M6 Transition + ReevaluationSource; M3 PythonSyntaxIndex
  (certificate producer: what a syntax backend can honestly establish);
  M4 store (additive completeness_bindings table); M2 oracle (X5 row now
  matched end-to-end).
- **Outputs**: trellis-core coverage module (Universe, CoverageState,
  CoverageCertificate, Completeness, CompletenessEvaluator);
  PythonSyntaxIndex::coverage_certificate; store binding APIs;
  Transition::with_coverage (CoverageContext) + claim dirtying +
  coverage-downgrade contract rule.
- **Invariants**: kinds carry no backend-specific knowledge (§8.2);
  certificate produced by the backend, never the engine; empty +
  unsatisfied → UNKNOWN, never authoritative absence (§8.2); M6
  semantics preserved when no coverage context is supplied.
- **Acceptance**: M7_ABSENCE_ACCEPTANCE (QUALITY_GATES.yaml).
- **Non-goals**: SCIP adapter (M8), provisional overlay (M9), benchmarks.

## Design decisions (DECISIONS entries follow)

- Observation value digests stay PLAIN value identity (no completeness
  marker folded in): a coverage downgrade dirties the CLAIM via the
  (Q, U, C) binding comparison (§8.1), not by masquerading as a value
  change. Downstream: would-hold + degraded → UNKNOWN (never
  authoritative absence); would-fail (real value change) → STALE
  regardless of coverage.
- Bindings: one current (Q, U, C) per projection (same-snapshot
  conflicting rebind = Constraint; new snapshot supersedes). Dirtying
  rule: universe digest / coverage digest / indexer identity mismatch vs
  persisted binding → reevaluation even with an empty changed set
  (catches indexer bumps); unbound → dirty (fail-closed baseline).
- coverage_dirty (verdict change) enters the artifact pass without being
  a value change; verdict-unchanged dirtying reevaluates but propagates
  nothing (step-12 cutoff holds).
- Syntax backend certificate: universe/declaration inventory Established
  only over indexed+parsed units; resolved-reference + inheritance =
  Unproven (a syntax backend can never claim them — M8 SCIP is the first
  producer that may); failed units = explicit failures.

## Gate results (2026-09-14)

- cargo test --workspace: 174 passed / 0 failed (6 cas + 43 core + 42 engine + 14 oracle + 22 program + 20 source + 27 store)
- cargo clippy --workspace --all-targets -- -D warnings: clean
- cargo fmt --all --check: clean
- fixtures/oracle untouched (git-verified)

## Test-name evidence per M7_ABSENCE_ACCEPTANCE criterion

- (Q, U, C) binding persisted for completeness-sensitive kinds only:
  indexer_bump_dirties_completeness_observations (binding round-trip
  asserted; seeding records baseline bindings in seed_base)
- indexer-version change dirties (empty changed set):
  indexer_bump_dirties_completeness_observations (re-runs with identical
  identity → empty report)
- universe growth dirties + reevaluation re-establishes (no downstream
  destruction): universe_growth_dirties_and_reestablishes_absence
- coverage degradation → UNKNOWN, definition-local unaffected:
  x5_coverage_degradation_downgrades_to_unknown (X5 oracle rows matched
  end-to-end: ART_NO_CALLERS_FACT/ART_CALLERS_REFRESH/ART_PROVIDER_SET/
  ART_LOGIN_CALLERS → UNKNOWN; ART_VALIDATE_SIG/ART_STATELESS VALID)
- empty + unsatisfied → UNKNOWN semantics: core coverage unit tests
  (evaluator_satisfies_only_complete_certificates) + x5 test
- X5 matched end-to-end: x5_coverage_degradation_downgrades_to_unknown;
  M6-compatible no-context path pinned by
  x5_without_coverage_context_keeps_standings
- kinds backend-independent; no SCIP knowledge: coverage module in
  std-only trellis-core; certificate produced by trellis-program; engine
  only evaluates requirements ⊆ capabilities
- M6 semantics preserved: all 11 pre-existing redgreen tests unchanged
  and green; catalog sweep R0-R4/X1-X4 still green
- oracle labels untouched; determinism: transitions_are_deterministic
  (unchanged) + new tests are deterministic (fixed timestamps, canonical
  digests)
- degraded coverage + value change composition: degraded_coverage_
  composes_with_value_changes (real value change stays STALE)

## Known deviations

- None. X5 previously documented as M7-gated is now matched end-to-end.

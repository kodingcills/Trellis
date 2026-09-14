# M6 run log — Red/green invalidation with value-equality cutoff (spec §18)

## Bounded plan (controller, 2026-09-14)

- **Objective**: implement the frozen §18 transition (steps 1-12) in
  `trellis-engine`: reconcile-changed-set → M5 candidate discovery →
  projection reevaluation → value-equality cutoff (GREEN) / value-change →
  dependent artifact validity-contract evaluation → attestation appends →
  explain record. First complete deterministic Trellis kernel.
- **Inputs**: M1 reconcile `ChangedSet` + new snapshot id; M5
  `recorded_from_store` + `reevaluation_candidates`; M3 `PythonSyntaxIndex`
  (syntactic values); M2 oracle catalog (assertions ONLY); M4 store
  (artifacts, attestations, observations — all existing APIs, one additive
  query `artifacts_by_dependency`).
- **Outputs**: `trellis-engine::redgreen` — `ReevaluationSource` trait
  (syntactic built-in + injectable semantic source), transition runner,
  `TransitionReport` (explain record). Test harness + acceptance traces.
- **Invariants**: EngineState internal-only (§11); attestations append-only
  (§12); capture trust propagated from derivation, never claimed (§13);
  candidate ≠ value-changed ≠ stale distinct (§2.1); oracle labels are
  ground truth, never read by evaluation code (§26); determinism (§19).
- **Acceptance**: M6_ORACLE_ACCEPTANCE (QUALITY_GATES.yaml), §33 first
  acceptance trace + red/green counterexample, R0-R4/X1-X4 sweep vs catalog.
- **Non-goals**: coverage certificates / absence authority (M7), SCIP
  adapter (M8), provisional overlay (M9), benchmark harness (M10).

## Contract semantics (decided before implementation; DECISIONS entry follows)

- STRUCTURAL_SET contract: valid iff every dependency projection's
  reevaluated canonical value equals the value recorded in the artifact's
  active payload (value-continuity per oracle README conventions; §18 step
  12 cutoff is exactly this comparison).
- Absence-FACT contract: valid iff each absence-dependency's reevaluated
  value is the canonical empty-set encoding. Proposition text is never
  parsed; the contract is selected by artifact kind + verifier identity.
- No-verifier + value-changed dependency → append UNKNOWN (conservative,
  §11/§15; never auto-reuse).
- Semantic-kind reevaluation: production has NO semantic source until M8;
  the engine treats an unresolvable evaluation as no-new-evidence (skip,
  never synthesize a value, §8). The M6 test harness injects a fixture
  textual call-graph source (test-support only, never reads catalog.json,
  never shipped in production code paths).

## Implementation notes

(appended as work proceeds)

## Mechanical gate results

(appended after runs)

## Gate results (2026-09-14)

- cargo test --workspace: 167 passed / 0 failed (6 cas + 40 core + 6 engine-observe + 22 engine-discovery + 10 engine-redgreen + 14 oracle + 22 program + 20 source + 27 store)
- cargo clippy --workspace --all-targets -- -D warnings: clean
- cargo fmt --all --check: clean
- fixture unittest suite: unaffected (18 OK, M2 labels untouched)

## Test-name evidence per M6_ORACLE_ACCEPTANCE criterion

- §33 first trace: first_acceptance_trace_r2_absence_breaks_to_stale
- §33 counterexample: red_green_counterexample_r3_body_change_stops_propagation
- R1 no false invalidation: r1_unrelated_change_produces_no_false_invalidation
- R4 dependents-only: r4_signature_widening_propagates_to_dependents_only
- X1 aliased caller / X2 re-export / X3 third implementation / X4 universe growth: oracle_catalog_sweep_matches_labels (chains [X1] [X2] [X3] [X4])
- catalog sweep R0-R4 + X1-X4 vs artifact_consequences: oracle_catalog_sweep_matches_labels
- X5 M7-gated deviation (documented, not asserted): x5_coverage_degradation_is_m7_gated
- explain record contents: asserted in first_acceptance_trace (projection, pinned old digest, new value, affected artifacts)
- EngineState isolation + append-only: asserted via attestation history round-trip + type surface in first_acceptance_trace and undecidable_contract_appends_unknown
- determinism: transitions_are_deterministic
- restart equivalence: transition_survives_restart_via_persistence
- fail-closed loader over seeded state: seeded_observations_load_fail_closed

## Implementation-scope notes (for DECISIONS follow-up)

- M6 contract semantics: STRUCTURAL_SET = value-continuity vs the latest attestation's pinned Observation evidence (oracle README conventions + catalog R3 STALE→VALID row); absence-FACT = reevaluated value equals canonical empty-set encoding; no-verifier + changed dependency → UNKNOWN append (§15/§18 step 9).
- Fixture semantic source is test-support only (trellis-engine/tests), never reads catalog.json; resolves name/aliased-module-attribute calls and relative imports; instance-attribute method calls (ART_LOGIN_CALLERS's subject) are outside its resolution — that artifact exercises continuity only. Production semantic backend arrives with M8 SCIP.

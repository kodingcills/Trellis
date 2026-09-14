# M8 run log — Authoritative SCIP freeze adapter (spec §9.1, §8.2, §6)

## Bounded plan (controller, 2026-09-14)

- **Objective**: authoritative SCIP freezes per spec — SCIP batch ingest
  into Trellis's own normalized program graph (SCIP knowledge ONLY in
  the adapter crate), SemanticSnapshotId production at freeze points,
  the tightened §6 freeze rule via a change classifier, and
  CoverageCertificate population from SCIP evidence.
- **Inputs**: scip crate 0.10 (Rust protobuf bindings — the adapter's
  only SCIP touchpoint); M7 coverage types; M3 syntactic backend;
  M2 oracle (R2/X1/X3 label shapes as independent expectations).
- **Outputs**: trellis-scip crate (ingest, freeze rule, ScipGraph,
  ScipIdentity, honesty-floored CoverageCertificate);
  FrozenSemanticSource in trellis-engine (production transition wiring).
- **Invariants**: no SCIP types/knowledge outside trellis-scip (§8.2,
  §24); certificate claims only what SCIP proves; unindexed eligible
  units are explicit failures, never silent gaps (§8.2); deterministic
  SemanticSnapshotId over graph + identity (§19); deterministic freeze
  decisions (§6 classifier is conservative: any .py change freezes).
- **Acceptance**: M8_FREEZE_ACCEPTANCE (QUALITY_GATES.yaml).
- **Non-goals**: overlay evaluation (M9), benchmarks (M10).

## Normalization contract (scip-python shape; probed against the real
## scip::symbol parser before implementation)

- File descriptor: backtick-escaped path (`` `auth/tokens.py` ``) with
  Namespace suffix — the ONLY legal home for `/` in symbols (verified:
  unescaped `users/__init__.py` fails the scip grammar parser).
- Dotted subject: path stem (`auth/tokens.py` → `auth.tokens`, with
  `/__init__` stripped) + Term/Method descriptors
  (`auth.service.AuthService.verify`).
- Cross-document references resolve through external_symbols + every
  document's SymbolInformation (scip-python emits SymbolInformation
  only for symbols DEFINED in a document).
- Caller member: innermost enclosing def (definition-range containment
  on 3/4-element ranges), module member fallback for module-level code.
- Definition sites are never members of their own caller/reference sets
  (oracle conventions).
- Implementation/subclass edges from Relationship.is_implementation /
  is_type_definition (precise SCIP proof → Established capabilities).

## Gate results (2026-09-14)

- cargo test --workspace: 186 passed / 0 failed
  (6 cas + 43 core + 43 engine + 7 scip + 14 oracle + 22 program + 20 source + 27 store + 4 freeze)
- cargo clippy --workspace --all-targets -- -D warnings: clean
- cargo fmt --all --check: clean
- fixtures/oracle untouched (git-verified)

## Test-name evidence per M8_FREEZE_ACCEPTANCE criterion

- SCIP confined to the adapter: workspace grep — `scip::` appears only
  in crates/trellis-scip (lib, freeze, ingest + their tests); engine
  consumes only trellis-scip's Trellis-typed exports (ScipGraph etc.)
- batch ingest → normalized graph:
  symbol_decoding_produces_dotted_subjects,
  definition_site_is_never_a_member (ingest accounting in IngestReport)
- freeze → SemanticSnapshotId + authoritative queries exact + honest
  certificate: fixture_freeze_callers_exact_vs_oracle (R2 label:
  exactly api.webhooks.handle_refresh),
  certificate_is_honest_about_universe (unindexed unit = explicit
  failure), freeze_certificate_binds... (subset Complete / full Unknown —
  honesty floor demonstrated both ways)
- §6 freeze rule (change classifier): freeze_rule_readme_vs_auth_change
  (README → Reuse with reason; auth/*.py → Freeze),
  classifier_is_conservative (mixed sets flip to PythonSource)
- callers exact vs oracle: fixture_freeze_callers_exact_vs_oracle +
  caller_sets_exact_and_aliased_caller_detected (X1 aliased shape)
- indexer identity in coverage evidence (§8.1):
  freeze_identity_and_evidence_are_deterministic (identity rendering
  asserted; ScipIdentity::ADAPTER_VERSION hashes into snapshot id)
- M0-M7 tests unchanged and green (all prior suites untouched)
- determinism: freeze_identity_and_evidence_are_deterministic (same
  index + identity → same SemanticSnapshotId, same graph, same report)

## Known deviations

- The freeze harness builds reduced SCIP indexes programmatically (the
  M8 contract does not require running real scip-python — that arrives
  with M12's OSS validation; the adapter itself is exercised against
  the real scip crate's parser and types throughout).
- freeze_semantic_snapshot takes the fresh index as input even on the
  Reuse path (v0.1 economics: the caller may run SCIP unconditionally
  and let the classifier discard the result; incremental SCIP reuse is
  deferred to M10's measured-bottleneck policy).

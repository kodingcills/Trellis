# Run 2026-09-08 M2 — synthetic oracle fixture + mutation catalog

orient: STATE.next_candidates=[M2, M4]; M2 selected (oracle-first critical path per spec §33; M4 also unblocked but M6 depends on M2's labels). Node contract activated; M2_ORACLE_REVIEW gate defined in QUALITY_GATES NODE_GATES BEFORE implementation (per DECISIONS convention).
plan:
  - fixtures/python_auth/base/ — 35 stdlib-only Python modules (auth incl. provider interface + 2 implementors, users, payments incl. gateway interface + 2 implementors, api, config, 9 test modules) + pyproject; no third-party deps, no Trellis annotations
  - fixtures/oracle/{README.md, catalog.json} — machine-readable mutation catalog schema trellis-oracle-catalog/1: R0–R4 (frozen spec §26/§99 lineage) + adversarial X1–X5 (alias import, re-export, subclass addition, universe growth, indexability failure)
  - crates/trellis-oracle — mechanical catalog integrity + independent textual cross-checks (serde/serde_json deps, test-support layer)
  - per-mutation: intended semantic change, affected projections (expected values), unchanged projections, artifact validity consequences (VALID/STALE/UNKNOWN), completeness/absence flag, ground-truth rationale, provenance array
  - label provenance restricted to: fixture-construction, python-import-semantics, textual-reference-cross-check, hand-reviewed-call-graph
execute: fixture authored by hand; labels hand-derived then cross-checked via independent textual scans (refresh_token occurs only in auth/tokens.py on BASE; exactly two `(AuthProvider):` implementors); R0–R4 consequences enumerable without any Trellis machinery, which does not exist yet
verify:
  - cargo test --workspace: 72 passed (39 core + 13 oracle + 20 source), 0 failed
  - cargo clippy --workspace --all-targets -- -D warnings: clean
  - cargo fmt --all --check: clean
  - fixture baseline: python3 -m unittest discover → Ran 18 tests, OK
  - M2_ORACLE_REVIEW criteria coverage: independence (labels authored before engine exists; no engine-derived labels possible), machine-readable catalog (JSON schema enforced by 13 oracle tests), unchanged-value cases (R1/R3/R4/X4), absence/coverage cases (R0/R2/X4/X5), provenance per entry, no fixture→Trellis coupling (fixture_has_no_trellis_coupling test)
review: fresh adversarial review cycle 1 → REPAIR_REQUIRED (0 BLOCKER, 1 IMPORTANT, 4 OPTIONAL). Record: reviews/2026-09-08-M2-cycle1.md
repair:
  - IMPORTANT under-enumerated artifact consequences → corrected (option a): all 10 mutations now enumerate all 6 seeded artifacts; X5 coverage-downgrade scope made explicit (ART_PROVIDER_SET + ART_LOGIN_CALLERS → UNKNOWN per §8.2; ART_VALIDATE_SIG/ART_STATELESS remain VALID with scope reasons); regression test consequence_tables_enumerate_every_artifact added
  - OPTIONAL X4 runtime bug → corrected in same batch (user.status = Status.SUSPENDED)
  - OPTIONAL R4 rationale wording → softened in same batch (mutated-tree test execution deferred to harness)
  - OPTIONAL X2 def-site convention → documented in oracle README
  - OPTIONAL vocab enforcement → BACKLOG; X1 textual cross-check → BACKLOG; mutated-tree harness → BACKLOG
  - gates re-run post-repair: 73 tests green (39+14+20), clippy -D warnings, fmt, fixture unittest 18 OK
  - post-repair fresh review: mandatory per POST-REPAIR RULE → reviews/2026-09-08-M2-cycle2.md (cycle 2)
  - attempt counter: 0 repeated failures
notes:
  - Fixture bug found and fixed during baseline verification: auth.tokens.verify_token crashed with ValueError instead of InvalidToken on malformed tokens (fixed; regression covered by tests.test_session).
  - X3 op content bug fixed pre-review: SsoAuthProvider referenced non-existent self._trusted_issuer attribute (now self._issuer).

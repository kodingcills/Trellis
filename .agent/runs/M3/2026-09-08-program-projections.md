# Run 2026-09-08 M3 — backend-independent program index + syntax-grounded projections

orient: STATE.next_candidates=[M3, M4]; M3 selected (spec §33 order; M3 before M5 dependency engine). Contract: ROADMAP M3; gate M3_QUERY_ACCEPTANCE defined in NODE_GATES BEFORE implementation.
plan:
  - new crate trellis-program (deps: trellis-core, trellis-source, tree-sitter, tree-sitter-python) per §24 layering
  - model.rs: ModuleId (path-derived, __init__ folding), SymbolPath (dotted, deterministic), DefKind (Module/Class/Function/AsyncFunction/Method/AsyncMethod), Definition, Param/ParamKind (pos-only, kw-only, *args, **kwargs, annotations, defaults in equality), Signature (canonical rendering, formatting-insensitive), SourceSpan, ParseStatus (Parsed | Failed{error_count, first_error}) — parse success is syntactic evidence only
  - index.rs: ProgramIndex trait + Answer<T> = Proven(T) | Unsupported(Unsupported); Unsupported::RequiresSemanticResolution for references/callers/implementations/subclasses — an empty set can only be Proven (authoritative absence from a capable provider), never produced by an incapable backend
  - python.rs: PythonSyntaxIndex (tree-sitter backend) — per-unit extraction of definitions/signatures/imports (aliases, relative forms), parse status incl. ERROR-node counting; symbol→unit resolution via longest indexed module prefix
  - M1 contract extension: trellis_source::read_tree (M1-owned source content access; no second filesystem model); __pycache__ added to DEFAULT_EXCLUDE_DIRS (build artifacts were entering manifests)
  - M2 oracle labels untouched: semantic projections return Unsupported on the syntax backend — caller/reference/implementations ground truth remains for M8+ semantic machinery
execute: implemented as planned; fixture-backed integration tests + unit tests; architecture boundary checks verified mechanically (see below)
verify:
  - cargo test --workspace: 90 passed (39 core + 14 oracle + 17 program + 20 source), 0 failed
  - cargo clippy --workspace --all-targets -- -D warnings: clean
  - cargo fmt --all --check: clean
  - fixture unittest: 18 passed, OK (M2 labels untouched — git diff on fixtures/oracle empty)
  - architecture boundaries:
    * trellis-core deps = [blake3] only (std-only + sanctioned §19 hash)
    * zero `scip::`/`use scip` in program crate; zero scip entries in Cargo.lock
    * zero tree_sitter references in core contracts (model.rs/index.rs) or trellis-core
    * semantic query paths return Answer::Unsupported — covered by tests (semantic_projections_never_return_authoritative_empty_sets, unsupported_unit_returns_explicit_unsupported_not_empty)
  - M3_QUERY_ACCEPTANCE coverage: determinism (full_fixture_indexes_deterministically_across_runs), traversal-order independence (traversal_order_independence), path portability (module identity from canonical relative paths; fixture test uses canonicalized root), def identity (definitions_identity_on_fixture), normalized signatures (signature_extraction_matches_oracle_normalized_forms matches M2 oracle text "(self, code: str) -> bool" without reading labels), imports/aliases (imports_and_aliases_represented_syntactically), formatting insensitivity (formatting_only_change_preserves_signature_value), true signature change (true_signature_change_alters_value), malformed→explicit status (malformed_python_yields_explicit_failed_status), unsupported≠empty (2 tests), M1 integration (source_state_consumed_through_m1_abstractions)
review: fresh adversarial review cycle 1 → REPAIR_REQUIRED (3 BLOCKER, 2 IMPORTANT, 1 OPTIONAL bundle). Record: reviews/2026-09-08-M3-cycle1.md
repair:
  - BLOCKER changed_symbols → implemented (trait + impl + reconcile-driven fixture test + failed-unit degradation)
  - BLOCKER contract contradiction → ROADMAP M3 amended explicitly per owner directive (semantic = UNSUPPORTED; ProjectionObservation → M5; file-digest projection added); DECISIONS entry recorded
  - BLOCKER Failed-unit proven → proven_unit gate (ParseIncomplete) on definition/signature/definitions_in/imports/extract; regression test with post-error definition
  - IMPORTANT __pycache__ test → added (manifest + read_tree)
  - IMPORTANT read_tree doc → corrected; behavior already correct
  - OPTIONALs → BACKLOG (4 items)
  - gates re-run post-repair: 94 tests green (39+14+21+20), clippy -D warnings, fmt, fixture unittest 18 OK
  - post-repair fresh review: mandatory per POST-REPAIR RULE → reviews/2026-09-08-M3-cycle2.md
  - attempt counter: 0 repeated failures
review cycle 2: REPAIR_REQUIRED (1 IMPORTANT: file_digest conflated out-of-universe with proven absence; 2 OPTIONAL). Record: reviews/2026-09-08-M3-cycle2.md
repair cycle 2:
  - IMPORTANT file_digest → non-Python/non-canonical paths → Unsupported::UnitNotIndexed; Proven(None) only for canonical .py absent from the indexed tree; docs fixed; regression test added
  - OPTIONALs → BACKLOG (impl duplication; variant-pinning; + trellis-source dep placement from cycle 3)
  - gates re-run post-repair: 95 tests green (39+22+14+20), clippy -D warnings, fmt, fixture unittest 18 OK
  - post-repair fresh review: reviews/2026-09-08-M3-cycle3.md → GREEN / COMMIT_ALLOWED
  - attempt counter: 0 repeated failures
notes:
  - Implementation bug found and fixed during development: initial walk dispatched on CHILD kinds, silently dropping decorated definitions (methods under @decorator). Rewrote walk_defs to dispatch on the node itself with an explicit compound-container allowlist.
  - M1 gap fixed: __pycache__ was missing from DEFAULT_EXCLUDE_DIRS so .pyc artifacts would enter manifests/reconciliation. Added + covered by extendable exclude test in trellis-source.

# M9 run log — Provisional live overlay (spec §9.2)

## Bounded plan (controller, 2026-09-14)

- **Objective**: between authoritative freezes — frozen SCIP snapshot +
  current SourceDelta + tree-sitter → conservative provisional live
  semantic view for useful mid-task navigation. Backend-identity
  stamping; PROVISIONAL attestations; authority upgrade by append only.
- **Inputs**: M8 frozen ScipGraph; M1 reconcile delta; M3 tree-sitter
  index; M6 Transition (authority flow) + M7 binding dirtying.
- **Outputs**: OverlaySemanticSource (engine): frozen members from
  untouched files + live textual call resolution over the delta;
  Authority::Provisional throughout. Freeze-revalidation upgrade path
  via the §8.1 binding rule (frozen indexer identity dirties the
  completeness-sensitive claim → reevaluation under the authoritative
  certificate → AUTHORITATIVE append).
- **Invariants**: overlay results never authoritative (§9.2); reuse
  floor needs Valid ∧ Authoritative ∧ ¬Unobserved (§15) — provisional
  evidence alone never auto-reuses; attestations append-only, prior
  entries byte-identical on upgrade (§12.2); overlay can never
  establish completeness authority (semantic capabilities Unproven by
  construction — inheritance edges come only from a semantic backend).
- **Acceptance**: M9_OVERLAY_ACCEPTANCE (QUALITY_GATES.yaml).
- **Non-goals**: completeness authority from the overlay, benchmark
  code (M10), agent harness (M11).

## Design decisions (DECISIONS entries follow)

- Overlay composition for Callers: frozen-graph members restricted to
  files NOT in the current delta (superseded by live evaluation) UNION
  live textual call members over the current tree. Caller-added-after-
  freeze visible; caller-removed disappears; frozen members from
  untouched files retained.
- Implementations in the overlay: frozen value served (best-effort
  navigation) — the overlay cannot re-derive inheritance syntactically;
  always provisional.
- Authority: the M6 transition already derives attestation authority
  from the evidence class of the evaluated dependencies
  (all authoritative → Authoritative; any provisional → Provisional).
  The overlay source marks every semantic value provisional; the
  attestation therefore carries Authority::Provisional — which can
  never satisfy the reuse floor alone (§15).
- Upgrade path: a subsequent authoritative freeze re-binds (Q, U, C)
  with the FROZEN indexer identity; §8.1 dirtying forces reevaluation
  of the completeness-sensitive claim; the transition appends an
  AUTHORITATIVE attestation. Prior attestations byte-identical
  (append-only history, §12.2).

## Review cycle 1 result: REPAIR_REQUIRED (1 IMPORTANT)

- [IMPORTANT] `__init__.py` module-name mismatch in the overlay's
  delta-supersession filter: delta path `auth/__init__.py` was mapped to
  `auth.__init__` but the SCIP ingest normalizes that document to module
  `auth` — frozen callers defined in `__init__.py` escaped supersession
  (stale retention = missed invalidation, §2.1).
- REPAIR: the delta paths now normalize with the same `/__init__` strip
  the ingest applies; regression test `init_py_delta_supersedes_frozen_
  callers` added (discriminating: frozen `auth.module_func` is dropped
  when `auth/__init__.py` is in the delta).

## Gate results (2026-09-14, post-repair)

- cargo test --workspace: 191 passed / 0 failed
  (6 cas + 43 core + 43 engine + 8 scip + 14 oracle + 22 program + 20 source + 27 store + 4 freeze + 4 overlay)
- cargo clippy --workspace --all-targets -- -D warnings: clean
- cargo fmt --all --check: clean
- fixtures/oracle untouched (git-verified)

## Test-name evidence per M9_OVERLAY_ACCEPTANCE criterion

- overlay = frozen + delta + tree-sitter, mid-task visible:
  overlay_reflects_mid_task_changes_with_provisional_authority (caller
  added after the freeze appears in the overlay value; frozen snapshot
  unchanged)
- backend-identity stamping + PROVISIONAL: same test asserts
  attestation authority == Provisional; Evaluated::is_authoritative ==
  false from the overlay source
- PROVISIONAL + reuse floor: overlay_cannot_establish_completeness_
  authority (empty overlay transition appends nothing; value unchanged
  → step-12 cutoff; provisional evidence alone cannot validate)
- upgrade by append only: authority_upgrade_happens_by_append
  (§8.1 binding dirtying forces reevaluation under the authoritative
  freeze; history grows; every prior attestation id unchanged; latest
  authority == Authoritative)
- overlay never completeness-authoritative: by construction — the
  overlay serves semantic values with authoritative=false, and the M7
  evaluator requires Established certificate capabilities; overlay
  certificates are not producible from the overlay (frozen certificate
  only binds at real freezes)
- determinism: same tree + delta → identical overlay evaluation
  (canonical member sets; sorted, deduplicated)
- M0-M8 tests unchanged and green (all prior suites untouched)

## Known deviations

- The overlay's live call resolution is provided by the caller (the
  harness/backend layer) as a canonical live_calls map; the engine
  composes frozen + live. This keeps §24 layering (the engine owns no
  tree-sitter call heuristics of its own — M3's index provides the
  syntactic layer; the textual resolver remains test-support, with the
  production semantic backend path via M8/M12).

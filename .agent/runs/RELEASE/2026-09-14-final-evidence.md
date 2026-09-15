# RELEASE_V0_1 — Final evidence summary (V0_1_COMPLETE)

Date: 2026-09-14 · Terminal commit: b9325a3 · Spec: draft-1.0-frozen

## Campaign outcome

All 12 milestone nodes (M0–M12) plus RELEASE_V0_1 are **completed**,
each crossing its transaction boundary (implement → mechanical gates →
fresh-context adversarial review → repair → fresh post-repair review
where required → COMMIT_ALLOWED → exact commit → state update). The
frozen Definition of Done (spec §32) is demonstrated and audited by two
independent fresh-context auditors.

## Definition of Done (§32 = §28 items 1–9 + 4 clauses) — evidence

| # | Item | Demonstrating evidence |
|---|------|------------------------|
| 1 | Python repo indexed; typed projections | `PythonSyntaxIndex` + `ProjectionKind`; SCIP freeze callers/implementations exact on fixture (`fixture_freeze_callers_exact_vs_oracle`) |
| 2 | Immutable artifacts, provenance, append-only attestations | Type-level §10/§12 + store FK/duplicate rejection; 195-test suite |
| 3 | Precise change detection; unchanged stay green | M1 reconcile; `red_green_counterexample_r3_body_change_stops_propagation` |
| 4 | Negative-dependency invalidation | `first_acceptance_trace_r2_absence_breaks_to_stale` (∅ → {caller} ⇒ STALE) |
| 5 | Red/green with value-equality cutoff | §18 step-12 filter; R3 chain shows zero downstream appends |
| 6 | Authoritative absence only under satisfied completeness | `CompletenessEvaluator`; M7 X5 + M12 OSS tests (UNKNOWN over Unproven) |
| 7 | `trellis explain` causal trace | `TransitionReport`/`ProjectionOutcome`/`ArtifactOutcome` APIs, fields asserted in tests |
| 8 | Second related task consumes still-valid artifacts | R2→R3 chain: ART_CALLERS_REFRESH Stale→Valid, others untouched |
| 9 | Baseline ladder at same tool capability | Benchmark C: 4 rungs, identical query, 24 trials, success 100%, false-valid 0 |

§32 additional clauses: Benchmark B headroom (9.1×, HOLDS); Benchmark C
at equal tool capability across ≥3 rungs (4 rungs, 24 paired trials);
attestation provisional → authoritative upgrade by append + drift-induced
UNKNOWN without authority mutation (M9 `authority_upgrade_happens_by_append`
+ M7 X5); coverage-certificate behavior (indexer-bump + universe-growth
dirtying; unsatisfied requirements → UNKNOWN).

## Mechanical gates (verified at terminal commit)

- `cargo test --workspace`: 195 passed / 0 failed (fixture unittest 18 OK)
- `cargo clippy --workspace --all-targets -- -D warnings`: clean
- `cargo fmt --all --check`: clean
- Fresh-clone reproducible: fixtures vendored, no network, deterministic

## Benchmarks (committed under benchmarks/)

- **A** oracle validity: 54/54 rows match labels; **false-valid = 0**
  (gate); stale recall 1.0; invalidation precision 1.0; abstention 4
- **B** systems economics: C_validate (p95) 1083µs vs C_recompute (p95)
  9844µs → **9.1× headroom; PREMISE HOLDS**; frontier scaling flat
- **C** controlled agent: 4 rungs, 24 paired trials, **task success
  100%**, stale reuse 0; fairness checklist committed
- **OSS** validation: pip 26.2.1 pinned (content digest 9bf8e499…):
  156/156 files indexed; honesty floor holds

## Review history

Every node carries fresh-context adversarial review with terminal
COMMIT_ALLOWED: M0 (HARDENING-c3), M1–M4 (2026-09-08 records), M5
(6 cycles), M6–M8, M9 (2 cycles), M10, M11, M12, RELEASE (2 audits:
initial REPAIR_REQUIRED with 2 findings, both repaired; closure
re-audit GREEN / RELEASE_APPROVED, zero findings).

## Kill criteria (§28) — status

Not approached. Capture+validation overhead ≈ 9× below recomputation
at fixture scale; artifact reuse demonstrated across related tasks;
candidate discovery has no false negatives across the full M2
mutation catalog; no whole-repository reanalysis after mutations.
Honest caveat (recorded): fixture-scale economics; real-model agent
integration is a v0.2 item.

## Remote policy

All commits are local (origin/main at fac5f5f). No push, tags, or
releases were performed — remote mutations require explicit standing
human permission (REMOTE_POLICY).

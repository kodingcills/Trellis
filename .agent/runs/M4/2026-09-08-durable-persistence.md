# Run 2026-09-08 M4 — durable SQLite metadata + local CAS

orient: STATE.next_candidates=[M4] (M5 deps = [M3, M4]); GLOBAL_GREEN verified at fac5f5f pre-work. Contract: ROADMAP M4; gate M4_PERSISTENCE_ACCEPTANCE defined in NODE_GATES BEFORE implementation.
plan:
  - crates/trellis-cas: FsCas (2-hex shard layout, put→tmp→fsync→atomic rename, get with content-vs-id verification, contains/verify), blob_id_of (BLAKE3 via trellis-core)
  - crates/trellis-store: Store (SQLite WAL + FK enforcement, schema_meta versioning = explicit rejection of unsupported versions), schema v1 (snapshots, artifacts, derivations, artifact_dependencies adjacency, validation_attestations w/ seq append-order, projection_observations, cas_objects), typed repo APIs (put/get_snapshot, put/get_artifact, get_derivation, append_attestation, attestation_history, put/get_projection_observation, register_blob, put_blob)
  - trellis-core: ValidationAttestation::from_persisted (capture trust carried forward, not re-derived — persistence is not a second authority); StoreError: From<DomainError>
  - 16 persistence tests + 6 CAS tests covering all 12 mandated adversarial scenarios + restart/recovery acceptance
  - identity invariants: canonical IDs persisted as TEXT PKs; no rowid/insertion-order/timestamp/path-derived identity; duplicate objects idempotent (INSERT OR IGNORE on canonical PK)
execute: CAS first (atomic protocol verified incl. crashed-put tmp exposure test), store second, tests third
verify:
  - cargo test --workspace: 101 passed (6 cas + 39 core + 14 oracle + 22 program + 20 source), 0 failed
  - cargo clippy --workspace --all-targets -- -D warnings: clean
  - cargo fmt --all --check: clean
  - fixture unittest: 18 OK; oracle diff vs HEAD empty
  - M4 gate coverage: WAL+FK tested; CAS idempotence/corruption/missing/tmp-isolation (6 tests); blob-first (metadata_never_references_absent_blob); identity order-independence (insertion_order_does_not_change_identity, absolute_paths_do_not_contaminate_identity); append-only + duplicate rejection + foreign-artifact rejection + monotonic created_at; schema rejection; transaction rollback; restart/recovery (restart_recovery_preserves_semantics_and_ordering — full A/B cycle, identity+domain equality, attestation ordering)
review: fresh adversarial review cycle 1 → REPAIR_REQUIRED (4 BLOCKER, 2 IMPORTANT, 2 OPTIONAL). Record: reviews/2026-09-08-M4-cycle1.md
repair:
  - BLOCKER cost loss → cost column persisted + decoded + regression test
  - BLOCKER evidence/verifier loss → typed-prefix encoding, round-trip, real persistence test replaces the fake
  - BLOCKER register_blob → durability gate (FsCas::contains) before registry insert; regression test
  - BLOCKER dec_list → Result-returning; malformed → Corrupt; tamper regression test
  - IMPORTANT traits/migrations deviation → DECISIONS entry
  - IMPORTANT GC → collect_orphans + crash-window test
  - gates re-run post-repair: 121 tests green (6+39+14+22+20+20), clippy -D warnings, fmt, fixture unittest 18 OK
  - post-repair fresh review: mandatory per POST-REPAIR RULE → reviews/2026-09-08-M4-cycle2.md
  - attempt counter: 0 repeated failures
review cycle 2: REPAIR_REQUIRED (1 BLOCKER: evidence element decode silently dropped malformed entries; 1 IMPORTANT: cost field-level mistyped values decoded to None; 1 OPTIONAL conflicting-duplicate → BACKLOG). Record: reviews/2026-09-08-M4-cycle2.md
repair cycle 2:
  - BLOCKER evidence decode → evidence_dec returns Result; collect::<Result> — malformed element → Corrupt; regression corrupt_evidence_element_is_explicit_not_silently_dropped
  - IMPORTANT cost decode → per-key strict decode (missing key / non-u64 / non-null → Corrupt); regression corrupt_cost_field_is_explicit_not_silently_zeroed
  - gates re-run post-repair: 123 tests green (6+39+14+22+20+22), clippy -D warnings, fmt, fixture unittest 18 OK
  - post-repair fresh review: reviews/2026-09-08-M4-cycle3.md
  - attempt counter: 0 repeated failures
review cycle 3: REPAIR_REQUIRED (1 IMPORTANT: producer_dec silently decoded mistyped model to None — same silent-drift class; 0 BLOCKER). Record: reviews/2026-09-08-M4-cycle3.md
repair cycle 3:
  - IMPORTANT producer decode → strict per-field decode (missing/mistyped → Corrupt); regression corrupt_producer_model_is_explicit_not_silently_dropped
  - gates re-run post-repair: 124 tests green (6+39+14+22+20+23), clippy -D warnings, fmt, fixture unittest 18 OK
  - post-repair fresh review: reviews/2026-09-08-M4-cycle4.md
  - attempt counter: 0 repeated failures
review cycle 4: GREEN / COMMIT_ALLOWED (0 BLOCKER, 0 unresolved IMPORTANT; 3 OPTIONAL → BACKLOG). Record: reviews/2026-09-08-M4-cycle4.md
commit: see STATE.json current_green_commit (recorded at checkpoint)
notes:
  - artifacts→snapshots FK dropped during development (domain constructor already requires created_snapshot); recorded in DECISIONS
  - GC deferred per node contract (orphan blobs acceptable; metadata never dangles — enforced by require_blob before metadata commit)

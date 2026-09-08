# Run 2026-09-08 M1 — deterministic source state

orient: STATE.next_candidates=[M1]; contract M1 (BLAKE3, canonical ser, manifest, reconcile, env fingerprints, crate boundary); spec §5/§6/§19/§24. M0 green at 8965d5e; gitignore pushed at 4e1cf16 (owner decision: `.agent/` stays out of repo).
plan:
  - trellis-core: canonical.rs (varint/length-prefixed/sorted primitives, domain tags), ContentHash::compute + ContentHasher (blake3), prelude updates
  - new crate trellis-source: manifest (walk+hash, excludes, normalization), reconcile (pure diff + against-tree lazy), environment (declared-only fingerprints, digests-only), freeze (snapshot construction)
  - DECISIONS entries for blake3 dep + crate boundary
  - non-goals honored: no watcher/daemon, no syntax/semantic indexing, no git traversal
execute: implemented as planned inline (single session); two fresh subagent contexts used for adversarial review (see reviews/2026-09-08-M1-fresh-adversarial.md)
verify:
  - cargo test --workspace: 39 (core) + 20 (source) = 59 passed, 0 failed
  - cargo clippy --workspace --all-targets -- -D warnings: clean
  - cargo fmt --all --check: clean
  - M1_RECONCILE_ACCEPTANCE (QUALITY_GATES NODE_GATES): all six criteria covered by tests — add/modify/delete (reconcile_against_tree_is_lazy_and_exact), identical→empty (two_independently_built_identical_trees_reconcile_to_empty), digest stability (manifest_is_sorted_and_stable_across_runs), ID reproducibility + BLAKE3 official vector (known_blake3_vector, compute_is_deterministic_and_input_sensitive, streaming_hasher_matches_one_shot), loud failures (conflicting_digests_for_one_path_rejected, non_utf8_paths_rejected_not_lossily_coerced), env fingerprints (same/different identity, raw values not retained, duplicate/empty keys rejected)
review: commit-blocked initially (1 BLOCKER, 4 IMPORTANT, 5 OPTIONAL) → all BLOCKER/IMPORTANT corrected, 3 OPTIONAL → BACKLOG, 2 OPTIONals corrected minimally where they were acceptance-wording or doc-guard items. Repair re-verification by fresh subagent context was attempted and CANCELLED (not a failed attempt); mechanical verification substituted: citation scan clean across control plane (DECISIONS historical wording excepted, documented), NODE_GATES defined, DECISIONS M1 entries present, spec headings confirmed.
repair:
  - BLOCKER: DECISIONS trail appended (crate boundary, blake3, symlink policy)
  - IMPORTANT: spec citations re-pointed 0.1→1.0 numbering (env→§19, layout/layering→§24, explain→§18/§28, benchmarks→§25/§28, crash/schema→§19)
  - IMPORTANT: NODE_GATES.M1_RECONCILE_ACCEPTANCE defined before evidence
  - IMPORTANT: ManifestError::ConflictingDigest (never silently resolve provenance conflicts) + test
  - IMPORTANT: non-UTF-8 components rejected (to_str strict), portable contract test
  - IMPORTANT: symlink-swap surfaces as `removed` (tested), policy recorded in DECISIONS
  - attempt counter: 0 repeated failures; no strategy changes needed
commit: see below (hash recorded in STATE.json after commit)
notes:
  - Deviation: repair re-review by fresh context was cancelled by orchestrator; mechanical verification substituted. Recorded per loop honesty rules.
  - `.agent/`, AGENTS.md, and the spec document remain untracked per owner decision; the spec document being out of repo is a noted hygiene gap.

# .agent/BACKLOG.md — non-blocking ideas

Optimizations, deferred architecture, possible v0.2 work, and review suggestions
that do not block the current milestone. Items here must NEVER keep a milestone
loop alive or land on the critical path without a human-approved ROADMAP
amendment.

Format: one line per item, tagged with the frozen-spec trigger that would justify it.

---

- Speculative fs-watcher dirty hints as a latency optimization — never authoritative; reconcile-at-query stays source of truth (spec §5).
- Rayon parallelism for CPU-bound indexing — only if profiling justifies (spec §24).
- trellisd + tonic gRPC boundary — enters when a genuine second-process consumer exists (spec §24 table).
- PostgreSQL MetadataStore + remote CAS + workers + leases — distributed phase (spec §24 table).
- Docker/OCI hermetic execution for environment-sensitive artifacts (spec §24 table).
- vLLM controlled open-model benchmarking (spec §24 table).
- PyTorch semantic-validity scorer — data first, Phase 3 (spec §27 phase ordering).
- Embedding-based relevance retrieval — relevance layer only, never validity (spec §22).
- Polyglot language support — after the invalidation engine is proven on Python (spec §9.4).
- Context budgeting / "context compiler" (spec §22).
- OpenTelemetry export — optional, not a core dependency (spec §24/§19 observability stance).
- LICENSE file: Apache-2.0 declared in Cargo.toml but no LICENSE file exists yet.
- M0 review residue: none outstanding at bootstrap.
- Windows path separator handling (`\` inside components) — reject or normalize when the M4 persistence boundary makes manifests cross-platform (M1 review, OPTIONAL).
- `write_sorted_str_pairs` duplicate-key guard at the canonical layer itself — today enforced by map builders; revisit when M3+ callers bypass the guards (M1 review, OPTIONAL).
- Environment fingerprint value-skip mechanism for sensitive keys — current policy: never declare secrets (doc'd on `from_declared`); consider explicit skip/allowlist UX later (M1 review, OPTIONAL).
- Numeric review-retry budget — REVIEW_INCOMPLETE retries currently share the bounded-retry spirit without an explicit number; candidate wording: review retries bound at 3, matching REPAIR_POLICY (hardening review cycle 1, see reviews/2026-09-08-HARDENING-cycle1-protocol.md, OPTIONAL).
- `crates/trellis-core/src/lib.rs` crate doc cites the frozen spec by title only; point it at `docs/architecture/TRELLIS_V0_1_FROZEN.md` in a future product-code commit (hardening review cycle 1, OUT_OF_SCOPE).
- Mechanical control-plane hygiene gate: every BACKLOG/STATE/ROADMAP attribution naming a review must resolve to an existing `.agent/reviews/` file, and every review record must carry a `review_closure` line (hardening review cycle 2, see reviews/2026-09-08-HARDENING-cycle2-provenance.md, candidate future gate).
- Oracle vocabulary enforcement: provenance tokens, validity/authority states, projection kinds/scopes in catalog.json are documented but not mechanically validated against their vocabularies (M2 review cycle 1, OPTIONAL).
- X1 alias-caller textual cross-check: add a `refresh_token(`-scan test for the X1 mutated tree, analogous to the R2/R3 caller test (M2 review cycle 1, OPTIONAL).
- Mutated-tree unittest execution harness: baseline-only unittest runs today; running the fixture suite under mutated trees belongs to the benchmark harness (M2 review cycle 1, OUT_OF_SCOPE for M2).
- fixtures/python_auth/README.md module count says "34"; actual/declared count is 35 — one-word fix in a future commit (M2 review cycle 2, OPTIONAL; deferred to keep the COMMIT_ALLOWED candidate state exact).

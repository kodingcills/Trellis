# Contributing to Trellis

## Build and test

```sh
cargo build --workspace
cargo test -p <affected-crate>        # focused iteration
```

Before a broad or release-bound change, run the full gate:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Requires Rust 1.85+ (see `Cargo.toml`). No network is needed — all test
fixtures are vendored.

Benchmarks have their own instructions: see `benchmarks/v02/`.

## Authoritative specification

`docs/architecture/TRELLIS_V0_1_FROZEN.md` is the frozen v0.1
architecture specification. `§n` references elsewhere in the repository
point into it. Treat it as the design of record; if implementation
evidence contradicts it, raise the conflict rather than silently
diverging.

## Crate boundaries

- `trellis-core` — domain invariants and the epistemic model. Keep its
  dependency surface minimal; adding a dependency here is a design
  decision, not a convenience (§24 crate layering).
- `trellis-source` — BLAKE3 manifests, lazy reconciliation.
- `trellis-program` — tree-sitter Python syntax projections.
- `trellis-scip` — authoritative SCIP freeze adapter. SCIP-specific
  knowledge lives here and nowhere else (§8).
- `trellis-engine` — candidate discovery and red/green transition.
- `trellis-cas` / `trellis-store` — content-addressed storage and SQLite
  metadata.
- `trellis-oracle` — Benchmark A ground-truth integrity. Test
  infrastructure, not product; it must never depend on the engine crates,
  so oracle labels stay independent of the implementation under test.
- `trellis-cli` — thin JSON command surface over the runtime. Glue only:
  subcommands delegate to the existing crates.

## Invariants

These are load-bearing. Violating one is a correctness bug, not a style
question.

- Attestations are append-only. No mutable epistemic state on artifacts
  or attestations; attestations never transition (§10, §12).
- No daemon, no filesystem watcher, no continuous dirty tracking.
  Reconcile-at-query is the source of truth (§5).
- No LLM as a validity oracle (§30).
- The reuse-policy floor does not bend: Valid ∧ Authoritative ∧
  ¬Unobserved (§15).
- Oracle fixture labels outrank implementation output. If a label
  conflicts with what Trellis produces, the label wins until a human
  reviews the discrepancy; labels are never edited to match output.
- Benchmark conditions must have equal tool capability (§25,
  same-logical-tool-API). Conditions may differ in state, never in the
  tools they can call.

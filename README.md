# Trellis

A provenance-aware incremental-computation runtime for coding agents:
it maintains agent-derived artifacts against an evolving repository
state so agents reuse still-valid work and recompute only what changed.

The core mechanism is typed **dependency projections** +
**append-only validation attestations** + **red/green reevaluation**
with a value-equality cutoff (Salsa-style red/green, extended to
non-deterministic computations and completeness-sensitive queries).

## Build & test

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Requires Rust 1.85+ (see `Cargo.toml`). No network needed: all test
fixtures are vendored.

## Layout

- `crates/` — the Rust workspace:
  - `trellis-core` (domain invariants, epistemic model, std-only)
  - `trellis-source` (BLAKE3 manifests, lazy reconciliation)
  - `trellis-program` (tree-sitter Python syntax projections)
  - `trellis-scip` (authoritative SCIP freeze adapter)
  - `trellis-engine` (candidate discovery + red/green transition)
  - `trellis-cas` / `trellis-store` (content-addressed storage + SQLite metadata)
  - `trellis-oracle` (Benchmark A ground-truth integrity)
- `fixtures/` — the synthetic oracle fixture (`python_auth/`, `oracle/`)
  and the pinned OSS snapshot (`oss_pip/`, content-digest locked)
- `benchmarks/` — committed results for Benchmarks A (oracle validity),
  B (systems economics), C (controlled agent), and the OSS coverage report
- `docs/architecture/TRELLIS_V0_1_FROZEN.md` — the frozen v0.1
  architecture specification (authoritative)
- `.agent/` — version-controlled engineering control plane (roadmap,
  quality gates, reviewer protocol, decisions, review records)

## Benchmarks (v0.1 evidence)

| Benchmark | Result |
|---|---|
| A — oracle validity | 54/54 rows match labels; false-valid = 0; recall/precision 1.0 |
| B — systems economics | C_validate ≈ 1.0ms vs C_recompute ≈ 9.3ms (≈8.5× headroom; premise HOLDS) |
| C — controlled agent | 4 ladder rungs, 24 paired trials, task success 100%, stale reuse 0 |
| OSS validation | pip 26.2.1 pinned snapshot: 156/156 files indexed; honesty floor holds |

See `benchmarks/*_results.txt` for the committed numbers and
`.agent/runs/` for the full run logs.

## Status

v0.1 complete: all 12 milestone nodes (M0–M12) green under
fresh-context adversarial review; the frozen Definition of Done
(spec §32) is audited in `.agent/reviews/2026-09-14-RELEASE-audit.md`.

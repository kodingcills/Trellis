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
  B (systems economics), C (controlled agent), and the OSS coverage
  report; `v02/` holds the real-agent experiment (runner, analyzer,
  MCP/OpenCode adapter, raw results, `REPORT.md`, `ABLATION.md`)
- `docs/architecture/TRELLIS_V0_1_FROZEN.md` — the frozen v0.1
  architecture specification (authoritative)
- `CONTRIBUTING.md` — build/test commands, crate boundaries, invariants

## Benchmarks (v0.1 evidence)

| Benchmark | Result |
|---|---|
| A — oracle validity | 54/54 rows match labels; false-valid = 0; recall/precision 1.0 |
| B — systems economics | C_validate ≈ 1.0ms vs C_recompute ≈ 9.3ms (≈8.5× headroom; premise HOLDS) |
| C — controlled agent | 4 ladder rungs, 24 paired trials, task success 100%, stale reuse 0 |
| OSS validation | pip 26.2.1 pinned snapshot: 156/156 files indexed; honesty floor holds |

See `benchmarks/*_results.txt` for the committed v0.1 numbers, and
`benchmarks/v02/REPORT.md` + `benchmarks/v02/ABLATION.md` for the
real-agent reuse experiment (protocol, raw results, and limitations).

## Status

v0.1 is complete against the frozen Definition of Done (spec §32): the
runtime, the oracle fixture, and Benchmarks A/B/C plus OSS validation
are committed as evidence.

v0.2 is in progress — a real coding agent reusing validated prior
computation through Trellis across related tasks. The reuse mechanism
is demonstrated (zero false-valid reuse, ceremony-free integration);
the cost comparison is not yet settled, because the 50-run ablation was
polluted by provider latency degradation. See `benchmarks/v02/` for the
protocol, the raw results, and the honest limitations.

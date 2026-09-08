# python_auth fixture

A synthetic but real-shaped Python service used as the **authoritative
oracle fixture** for Trellis Benchmark A (spec §26: synthetic = oracle;
pinned OSS = realism, never an oracle).

- `base/` — the pristine fixture (34 Python modules + pyproject).
  Standard library only, ordinary readable code, no Trellis-specific
  annotations anywhere in the source.
- `../oracle/catalog.json` — the mutation catalog and hand-authored
  expected-projection labels that Benchmark A will test against.
- `../oracle/README.md` — labeling methodology and provenance rules.

Domains: `auth` (tokens, passwords, stateless sessions, provider
interface + two implementations), `users` (model, repository, service,
profiles), `payments` (gateway interface + two implementations, pricing,
ledger), `api` (routes, middleware, responses), `config` (declared
settings), `tests` (stdlib unittest suite).

The fixture is authored **before** any invalidation machinery exists
(oracle-first, spec §33). Labels live only in the oracle catalog — never
in fixture comments — and are derived from fixture construction, Python
import semantics, and cheap independent textual cross-checks.

# Trellis Architecture & Technical Design Report

**Status:** Frozen implementation specification — **architecture frozen, implementation authorized**
**Version:** Draft 1.0 — supersedes Draft 0.1
**Frozen:** 2026-09-08, architecture review complete (review clarifications applied, Appendix A.12)
**Primary objective:** Prove that dependency-aware incremental maintenance of agent-derived computation produces substantial reuse without stale-result failures.

This document is the implementation spec. Where Draft 0.1 and this document differ, this document is authoritative. The amendment record relative to Draft 0.1 is listed in Appendix A.

---

# 1. Executive Summary

Trellis is a **provenance-aware incremental-computation runtime for coding agents**.

The project thesis:

> Coding agents should not repeatedly rediscover or recompute work that remains valid after the underlying codebase changes.

Trellis incrementally maintains agent-derived artifacts against an evolving, versioned repository state.

The central object is not a cached prompt or conversation. It is a typed, immutable **Artifact** whose value was derived under explicit **Dependency Projections** of a repository state, whose **Derivation** records how it was produced, and whose standing in the world is expressed by an append-only series of **Validation Attestations**.

Formally, let `W_r` represent the world state at repository revision `r`. A dependency is a projection:

```
d_i : W → V_i
```

such as `callers(TokenValidator.verify)` or `signature(AuthService.authenticate)`.

An artifact `A` records the observed dependency values:

```
D_A(W_r) = { d_1(W_r), d_2(W_r), ..., d_n(W_r) }
```

When the world changes, Trellis does not ask whether the whole repository changed. It determines which dependency projections may have changed and reevaluates only those. If `d_i(W_r) = d_i(W_{r+1})`, the dependency is green. If its value changed, the artifact becomes subject to verification or recomputation.

This is the fundamental mechanism.

---

# 2. Architectural Principles

1. **Correctness dominates reuse.** The worst failure is a stale artifact marked VALID and trusted. `C_false-valid ≫ C_unnecessary-recompute`. Early versions intentionally over-invalidate.
2. **Validation must be cheaper than recomputation.** If `C_validate ≉ C_recompute`, Trellis provides no value. This premise is measured, not assumed (Benchmark B).
3. **Dependencies represent semantic operations, not transport events.** `callers(AuthService.verify)` is strong information; `agent opened auth.py` is not.
4. **Deterministic analysis is authoritative.** ML is an optimization layer around uncertainty, never the correctness oracle.
5. **Local-first.** v0.1 is a modular monolith: one binary, one process, no distributed services.
6. **Artifacts are immutable.** New knowledge creates new artifacts; changed validity creates new attestations. Nothing epistemic is ever mutated in place.
7. **Every decision must be explainable.** For every artifact: why reusable, why invalidated, which dependency, what value before and after, what was affected.
8. **Authority is established by evidence, recorded immutably.** (New in 1.0.) Authority never mutates or downgrades; it is re-established by appending stronger evidence.

---

# 3. Explicit Scope of v0.1

The v0.1 world:

```
W = (repository, program index, selected configuration, toolchain fingerprint)
```

Concretely:

```
WorldState
├── git revision / working tree
├── tracked file contents
├── syntax trees
├── symbol definitions / signatures / references
├── caller / implementation relationships
├── imports
├── repository queries
├── dependency lockfiles
└── selected tool/runtime versions
```

Not included initially: arbitrary internet state, production databases, live APIs, distributed infrastructure, organizational knowledge.

Explicitly **not on the v0.1 critical path** (architecturally anticipated, not built): `trellisd`, gRPC, background workers, PostgreSQL, Docker/OCI execution, vLLM, PyTorch.

---

# 4. Core Domain Model

Primary domain concepts:

```
Snapshot
Projection
ProjectionObservation
Artifact
Derivation
Verifier
ValidationAttestation
CoverageCertificate
```

The epistemic lifecycle is:

```
Derivation          how the artifact was produced (provenance)
     ↓ produces
Artifact            immutable value + kind + dependencies
     ↓ evaluated against
Snapshot            a coherent frozen world state
     ↓ yields
ValidationAttestation  validity/authority of the artifact at that world
```

---

# 5. World-State Model: Lazy Reconciliation

**Frozen decision.** Dirtiness is **not continuously tracked**. There is no watcher, no daemon, no continuously maintained epoch journal in v0.1.

> Dirtiness is discovered lazily by reconciling the current working tree against the relevant stored source snapshot at query/freeze time. Expensive projection reevaluation is then also lazy.

```
stored SourceSnapshot S
        │
agent may make arbitrary edits
while Trellis is not running
        │
        ▼
artifact / query request
        │
        ▼
reconcile S → current working tree
        │
        ▼
exact changed-file set
        │
        ▼
tree-sitter change classification
        │
        ▼
only potentially affected projections reevaluated
```

- Reconciliation is cheap (file digests / mtimes against the snapshot manifest).
- Projection reevaluation happens only for the affected set, synchronously, before serving.
- A file watcher may later maintain *speculative* dirty hints as a latency optimization. A watcher is **never authoritative**; the reconcile-at-query step remains the source of truth.

---

# 6. Snapshot

A `Snapshot` identifies one version of the world Trellis has indexed. Snapshots are **explicit, on-demand coherent freezes** — not periodic events, not git-commit boundaries per se.

```rust
Snapshot {
    id: SnapshotId,
    repository_id: RepositoryId,
    git_commit: Option<GitOid>,      // informational
    working_tree_manifest: ManifestId,  // authoritative for v0.1
    index_id: ProgramIndexId,          // semantic index this freeze carries
    environment_id: EnvironmentFingerprint,
    parent: Option<SnapshotId>,
    created_at: Timestamp,
}
```

- A snapshot does not copy the repository; it references indexed state and a file manifest.
- **Freeze rule (tightened):** a freeze is required only when the requested authoritative decision depends on semantic projections whose current SCIP snapshot is **not provably compatible** with the current source state. An authoritative artifact lookup does *not* by itself imply a freeze.
  - Example: `SCIP @ S17`, current tree `W23`, change = `README.md` → the change classifier proves README cannot affect the Python semantic universe → S17 semantic observations remain applicable → **no SCIP rerun**.
  - Example: change = `auth/service.py` → relevant semantic state potentially changed → authoritative `callers(AuthService.verify)` requires a freeze.
- Scheduled anchors — task boundaries and explicit `trellis freeze` — always freeze.
- The parent relation gives `S₀ → S₁ → S₂ → …` for incremental change analysis.

---

# 7. Dependency Projection

The most important type in the architecture. A dependency is a function over world state whose value matters to an artifact:

```rust
Projection {
    kind: ProjectionKind,
    subject: Subject,
    property: Property,
    scope: Scope,
}
```

Examples: `FileContent(src/auth.py)`, `Signature(AuthService.authenticate)`, `Callers(TokenValidator.verify, scope=repository)`, `Implementations(AuthProvider)`, `RepositorySearch("refresh_token")`, `ConfigValue("security.session_timeout")`, `ToolVersion("python")`.

Evaluation and observation:

```
evaluate(projection, snapshot) → ProjectionValue

ProjectionObservation {
    projection_id
    snapshot_id
    value_digest
    canonical_value
}
```

Design note retained from 0.1: different properties of the same symbol invalidate different conclusions. Changing logging inside `authenticate` does not invalidate an artifact depending only on `Signature(AuthService.authenticate)`.

---

# 8. Completeness-Sensitive Queries: Universe and Coverage Certificates

Absence and completeness claims are only as sound as the universe they were evaluated over. `callers(refresh_token) = {}` is a claim about a query **and** the universe it covered **and** the evidence that coverage held.

## 8.1 Binding

Every completeness-sensitive observation binds to:

```
(Q, U, C)
```

- `Q` — the query (projection + parameters)
- `U` — the explicit eligible universe (set of eligible units: files/semantic units)
- `C` — a coverage certificate (below)

Persisted per observation:

```
query
universe descriptor
universe digest
coverage digest
semantic snapshot
indexer identity / version / config
result digest
```

Universe or coverage change **dirties the query and causes reevaluation** — it does not immediately destroy downstream artifacts. Downstream effects follow the normal red/green evaluation of the reevaluated value.

## 8.2 Requirements / capabilities split (Pin 2)

Backend-independence is structural, not aspirational:

- **ProjectionKind declares completeness *requirements*** — backend-independent proof obligations.

  Example — `Callers` requirements:
  ```
  eligible universe fully enumerated
  every relevant semantic unit participated
  resolved-call-reference capability available
  no unresolved coverage failures affecting query scope
  ```

- **CoverageCertificate declares established *capabilities*** — produced by the semantic provider adapter for a particular snapshot and universe.

  Example:
  ```
  CoverageCertificate {
      universe_complete = true
      all_units_participated = true
      declaration_inventory_complete = true
      resolved_reference_coverage = unproven
      inheritance_relationships = complete
      failures = [ ... ]
  }
  ```

- **CompletenessEvaluator** checks:

  ```
  Requirements(Q) ⊆ Capabilities(C)  →  COMPLETE
  otherwise                          →  UNKNOWN
  ```

Rules:

- Empty result + satisfied requirements ⇒ potentially **authoritative absence**.
- Empty result + unsatisfied requirements ⇒ `UNKNOWN`. Never an authoritative absence.
- SCIP-specific knowledge (e.g., "scip-python does not resolve X") lives **only** in the SCIP adapter/certificate. Projection semantics remain indexer-independent so that LSP, native resolver, or remote index backends can be substituted later without changing projection contracts.

Tree-sitter cross-validation (declaration inventory vs SCIP inventory per unit) is **evidence of coverage, not proof** of complete semantic reference resolution.

---

# 9. Semantic Indexing: Authoritative Batches + Conservative Live Overlay

## 9.1 SCIP at freeze points

SCIP is a batch indexer. v0.1 economics therefore separate the layers:

- **SCIP runs at freeze points** and produces *authoritative semantic snapshots*.
- **Tree-sitter + source delta** provides the current syntactic layer.

## 9.2 Provisional live semantic view

Between freezes:

```
frozen SCIP snapshot
+ current source delta
+ tree-sitter
= provisional live semantic view
```

Consistency semantics:

- Best-effort semantic navigation may consume the overlay.
- **Completeness-sensitive queries cannot treat the overlay as proof.**
- A fresh freeze is required for authoritative semantics where intervening changes may matter.
- Artifacts established mid-task against the overlay are evaluated under **provisional authority** (§12). They cannot be auto-reused on provisional authority alone; a freeze/revalidation must append an authoritative attestation.

## 9.3 Why tree-sitter + SCIP

They solve different problems and are complementary:

| Requirement | Tree-sitter | SCIP |
|---|---:|---:|
| Fast syntax parsing / changed regions | Excellent | Not purpose |
| Symbol definitions | Syntactic | Precise |
| Cross-file references, implementations | Weak alone | Strong |
| Works without full build | Often | Sometimes requires environment |

Pipeline: `git/file diff → tree-sitter changed syntax regions → semantic index refresh at freeze → SCIP-normalized graph`.

Trellis ingests SCIP into its own normalized representation (`ProgramSymbol`, `Definition`, `Reference`, `Implementation`, `Relationship`). SCIP is an input adapter; Trellis owns the internal program graph. The `ProgramIndex` trait insulates consumers:

```rust
trait ProgramIndex {
    fn definition(...)
    fn signature(...)
    fn references(...)
    fn callers(...)
    fn implementations(...)
    fn imports(...)
    fn changed_symbols(...)
}
```

## 9.4 Python first

One language, done well: Python. Sufficient semantic complexity, mature tree-sitter support, `scip-python` precise cross-file indexing, abundant benchmark repositories. Polyglot support is deferred until the invalidation engine is proven.

---

# 10. Artifact Model

An artifact is an externally meaningful result of computation — not hidden reasoning.

```rust
ArtifactEnvelope {
    artifact_id: ArtifactId,        // content-addressed, immutable
    schema_version: u32,
    kind: ArtifactKind,
    payload_ref: BlobId,
    proposition: Option<Proposition>,
    producer: ProducerInfo,
    derivation: DerivationId,
    dependencies: Vec<ProjectionId>,
    created_snapshot: SnapshotId,
    cost: CostRecord,
}
```

**Note what is absent:** no validity, authority, or verification fields on the artifact. Those belong to attestations (§12). An artifact is an immutable value with a provenance trail — never a mutable truth container.

Artifact kinds initially: `STRUCTURAL_SET, OBSERVATION, FACT, DERIVED_FACT, SUMMARY, HYPOTHESIS, PLAN, EXECUTION_RESULT, PATCH`.

---

# 11. Epistemic Dimensions (Four, Orthogonal)

Validity, authority, capture trust, and verification strength are independent questions and are modeled independently.

| Dimension | Values | Lives on |
|---|---|---|
| **Validity** | `VALID / STALE / UNKNOWN` | ValidationAttestation |
| **Authority** | `AUTHORITATIVE / PROVISIONAL` | ValidationAttestation |
| **CaptureTrust** | `COMPLETE / PARTIAL / DECLARED_ONLY / UNOBSERVED` | Derivation (propagates into attestations) |
| **VerificationLevel** | `UNVERIFIED / EVIDENCE_BACKED / STRUCTURAL / TEST / STATIC / DETERMINISTIC` | ValidationAttestation |

Conceptually:

```text
Artifact
├── immutable value
├── kind
└── derivation
      └── CaptureTrust

ValidationAttestation
├── target world (source + semantic snapshot)
├── Validity
├── Authority
├── VerificationLevel
├── verifier
└── evidence
```

Internal engine states (`GREEN / DIRTY / VERIFY / RED`) are **evaluation states** of the invalidation machinery, not part of the public truth model. They never leak into the artifact/attestation API.

This dimensional separation replaces Draft 0.1's single `verification_level` field and its flat validity enum. It also forbids the Draft 0.1 review temptation of adding `PROVISIONAL` as a validity state: an artifact can be `Validity: UNKNOWN, Authority: PROVISIONAL` and later `Validity: VALID, Authority: AUTHORITATIVE` — two independent axes.

---

# 12. Validation Attestations (Pin 1)

## 12.1 Model

Authority belongs to **immutable validation attestations scoped to a particular world/semantic snapshot** — not to mutable fields on the artifact.

```rust
ValidationAttestation {
    attestation_id: AttestationId,
    artifact_id: ArtifactId,
    target_source_snapshot: SnapshotId,
    target_semantic_snapshot: Option<SemanticSnapshotId>,
    validity: Validity,               // VALID / STALE / UNKNOWN
    authority: Authority,             // AUTHORITATIVE / PROVISIONAL
    verification_level: VerificationLevel,
    verifier: Option<VerifierId>,
    evidence: EvidenceRefs,
    capture_trust: CaptureTrust,      // propagated from derivation
    created_at: Timestamp,
}
```

## 12.2 Lifecycle

```
Artifact A
    │
    ├── Attestation @ S17
    │      validity = VALID
    │      authority = PROVISIONAL
    │
    └── Attestation @ S23
           validity = VALID
           authority = AUTHORITATIVE
```

- Provisional evidence (mid-task overlay evaluation) produces a **provisional attestation**.
- Freeze/revalidation creates a **new authoritative attestation**. It never mutates the provisional one.
- Existing attestations are never downgraded or mutated — the S17 attestation remains exactly what it always was.
- Working-tree drift does not retroactively weaken evidence. It makes that evidence **not yet applicable to the new world state**:

```
latest attestation = S23 (AUTHORITATIVE)
current world      = W24

→ S23's authority: still AUTHORITATIVE
→ A's current validity at W24: UNKNOWN until reconciled
```

- Individual attestations never transition. Authority strengthening in an artifact's validation history occurs only by **appending a stronger attestation**.

## 12.3 Effective reuse state

When somebody asks for `A` at world `W`:

```
artifact
    ↓
find strongest applicable attestation
    ↓
is it current for target world W?
    ├── yes → use its validity/authority
    └── no  → reconcile / revalidate → append new attestation
```

"Strongest applicable" is resolved by the reuse policy (§15), which is a function of all four epistemic dimensions — not of a single status enum.

---

# 13. Artifact Reuse Classes × Capture Trust

Reuse classes (from 0.1) and capture trust are **orthogonal**: a class describes what kind of claim the artifact makes; capture trust describes how well its derivation was observed. Reuse eligibility is the product of both.

## Classes

- **Class A — deterministic structural artifacts** (`CallerSet`, `ReferenceSet`, `ImplementationSet`, `ImportSet`, `SymbolSignature`): automatically reusable when dependencies are green **and** the projection's completeness policy is satisfied.
- **Class B — deterministically verified facts:** reusable while the attached verifier remains valid.
- **Class C — model-derived facts:** candidates only; require dependency validation plus sufficient authority/verification or explicit consumer policy.
- **Class D — hypotheses:** surfaced as "previously considered," never trusted reuse.
- **Class E — plans:** parts may serve as reference material.
- **Class F — patches:** never replayed after repository changes in v0.1.

## Capture trust (derivation-level)

- `COMPLETE` — Trellis has evidence that **every dependency-relevant observation channel participating in the derivation was captured with sufficient semantics** for that artifact's reuse policy.
- `PARTIAL` — mixed channels; some observations unrecorded.
- `DECLARED_ONLY` — dependencies agent-declared, not captured.
- `UNOBSERVED` — derivation relied on observations Trellis never captured.

**Trust is inherited from the capture channel, never claimed by the artifact.** An artifact cannot assert its own capture quality. Policy keys off this: `UNOBSERVED`-capture artifacts default to no auto-reuse regardless of class; `COMPLETE`-capture Class A artifacts receive full green treatment.

Level-2 trellis-aware structural tools (§21) are the **primary practical mechanism** for achieving `COMPLETE` in v0.1 — the mechanism, not the definition. Any fully-intercepted channel can in principle reach `COMPLETE`: e.g., a wrapped shell tool capturing cwd, command, environment dependencies, stdout/stderr, and the source snapshot satisfies the same evidence bar as a `trellis.*` tool.

---

# 14. Granularity and Materialization Policy

Retained from 0.1. Materialize when:

```
P(reuse) · C_recompute > C_capture + C_validation + C_storage + C_retrieval
```

- Always: `CallerSet`, `ReferenceSet`, `ImplementationSet`, `RepositorySlice`, `SymbolSignature`.
- Usually: `ArchitectureFact`, `RuntimeFact`, `Constraint`.
- Selectively: `Summary`, `Hypothesis`, `Plan`.
- Never: every model token, hidden reasoning, every line read. Individual reads become provenance/dependencies, not artifacts.

Artifact vs computation identity remain distinct:

```
ID_A = H(schema, kind, canonical_payload)          // artifact identity
ID_C = H(operator, intent, dependency identities,
         environment, producer configuration)      // computation identity
```

Computation identity indexes candidate previous outputs, not proof of replayability for model-derived computations.

---

# 15. Reuse Policy: Multidimensional

Reuse is **not** a function of one status. It is a function of the full epistemic tuple plus task context:

```
reuse(A, W, task) =
    f(
        strongest applicable attestation:
            validity, authority, verification_level,
        derivation.capture_trust,
        artifact class,
        dependency currency at W,
        completeness satisfaction for dependency observations,
        relevance(A, task),
        consumer trust policy
    )
```

Defaults:

- No applicable attestation, or attestation not current ⇒ reconcile/revalidate before any reuse.
- `Validity: UNKNOWN` ⇒ no auto-reuse.
- Authority `PROVISIONAL` ⇒ no auto-reuse unless policy explicitly permits (default: denied).
- `CaptureTrust: UNOBSERVED` ⇒ no auto-reuse (default).
- Class C/D/E/F ⇒ per-class rules of §13.

---

# 16. Validity Contracts: Kind / Verifier / Derivation Split

**Frozen decision.** Three responsibilities, three owners:

| Owner | Responsibility |
|---|---|
| **Artifact kind** | Defines the *semantic validity interface* — what "still valid" means for this shape of claim |
| **Verifier** | *Implements* that interface for a concrete artifact — establishes validity |
| **Derivation** | *Records* how the artifact was produced — provenance, never correctness |

Example:

```text
Artifact:  "normalize_email is idempotent"
Kind:      Proposition        (interface: attached predicate must verify)
Derivation: produced by an LLM after reading foo.py   (provenance)
Verifier:  property test f(f(x)) == f(x)              (establishes validity)
```

The verifier may be **attached after derivation** — which is precisely why derivation and verification cannot be the same concept. Multiple verifiers may attach over time; each attachment strengthens subsequent attestations.

- No applicable verifier ⇒ `Validity: UNKNOWN` ⇒ deny auto-reuse.
- Examples of kind-level interfaces: `CallerSet` → canonical set equality; `Signature` → normalized signature equality; `ArchitectureFact` → attached predicate still verifies; `Summary` → no automatic contract (conservative default).

---

# 17. Verification Subsystem

```rust
trait Verifier {
    fn verify(
        artifact,
        old_snapshot,
        new_snapshot
    ) -> VerificationResult;
}
```

Verifier families: `STRUCTURAL_QUERY, STATIC_CHECK, PROPERTY_TEST, UNIT_TEST, SCHEMA_CHECK, MODEL_REVIEW`.

Key mechanism retained from 0.1 and strengthened by the contract split:

```
model-derived claim
       ↓
generated verifier attached (post-hoc)
       ↓
verified claim
       ↓
cheap future revalidation via that verifier
```

---

# 18. Invalidation Engine: Red/Green

Borrowed philosophy from Salsa's red/green algorithm, extended to non-deterministic computations.

Given snapshot transition `S₀ → S₁`:

```
1. Reconcile S₀ → current tree: exact changed-file set.
2. Map changed inputs to potentially affected projections
   (reverse indexes: symbol → projections, file → projections).
3. Reevaluate those projections.
4. Old value == new value  → projection GREEN.
5. Value changed          → dependent artifacts DIRTY.
6. For each dirty artifact: evaluate its validity contract.
7. Cheap verifier succeeds → GREEN (append attestation).
8. Verifier fails          → STALE.
9. No verifier             → apply reuse policy (§15).
10. Recompute STALE artifact if requested.
11. Compare old semantic value with new value.
12. Propagate STALE downstream only if semantic value changed.
```

Step 12 prevents recursive over-invalidation: an internal implementation change that leaves the public architectural property unchanged leaves descendants green.

Universe/coverage changes (§8) enter at step 2 as dirtiers of completeness-sensitive queries; they trigger reevaluation, not immediate downstream destruction.

Validity states:

```
public:   VALID / STALE / UNKNOWN          (on attestations)
internal: GREEN / DIRTY / VERIFY / RED     (engine evaluation states)
```

---

# 19. Canonical Serialization, Hashing, Storage

Retained from 0.1, unchanged in substance:

- **Canonical serialization** before hashing: schema version, ordered fields, sorted sets/maps, normalized paths and symbol identifiers. Trellis owns the canonical hashing layer; protobuf wire format never defines identity.
- **BLAKE3** for content identities, algorithm encoded into IDs (`blake3:…`).
- **Content-addressed storage**, metadata separated from payloads:

```
.trellis/
├── metadata.db
├── cas/blake3/…
└── tmp/
```

- CAS write protocol: serialize → hash → temp file → fsync → atomic rename → commit metadata reference.
- **SQLite (WAL mode)** for metadata in v0.1; PostgreSQL deferred to the distributed phase (`FOR UPDATE … SKIP LOCKED` for queue claiming when multi-worker arrives).
- **Adjacency tables, not a graph database**, for artifact edges and dependencies.
- **Crash consistency:** blob first, metadata second — never the reverse; orphaned blobs collected by GC.
- **CAS GC:** mark-reachable from roots (active snapshots, pinned artifacts, recent runs, benchmark datasets, user pins); sweep after grace period.
- **Schema versioning:** every persistent object carries `schema_version`; incomprehensible old schemas ⇒ `UNKNOWN`, never reuse; implementation versions of validators/projections hash into identity.

Environment fingerprints: explicitly declared inputs only; environment variables deny-by-default; secrets never hashed, logged, or stored.

---

# 20. Query DSL

```text
file(path)
definition(symbol)
signature(symbol)
references(symbol, scope)
callers(symbol, scope)
implementations(symbol, scope)
imports(module, scope)
subclasses(symbol, scope)
repository_search(pattern, scope)
config_value(key)
tool_version(tool)
```

Every query produces `ProjectionKey`, canonical value, digest — and, for completeness-sensitive queries, a universe descriptor + coverage certificate (§8).

---

# 21. Dependency Capture and Agent Integration

Manual dependency annotation is never the primary interface. Capture levels:

- **Level 1 — observation:** wrap tools (reads, search, git, shell, tests). Semantically weak; produces `PARTIAL` capture trust at best.
- **Level 2 — trellis-aware structural tools** (preferred): `trellis.definition/references/callers/implementations/search/read` create explicit projections. Required for `COMPLETE` capture trust.
- **Level 3 — explicit publication:** agent publishes artifacts with evidence references. Model-declared dependencies are additional evidence, never the sole correctness mechanism.

---

# 22. Retrieval Is Separate From Validity

Validity asks: *is this still true?* Relevance asks: *would this help now?* Trellis's novelty is the first; practical integration needs both.

```
new task → candidate retrieval → validity evaluation
        → reuse-policy filtering → ranking → context-budget selection → agent
```

v0.1 retrieval: artifact type, symbol overlap, repository scope, task history, SQLite FTS, lexical matching. No vector database. Embeddings may join later as a relevance layer — never as a validity layer (`similarity ≠ validity`).

Context budgeting (constrained selection under a token budget) is downstream of the validity engine and not part of v0.1's core.

---

# 23. Execution Classification and Side Effects

Computation classes: `DETERMINISTIC, REPLAYABLE, MODEL_DERIVED, TEMPORAL, SIDE_EFFECTING` — each with separate reuse semantics.

Side effects (`git push`, deploy, PR creation, mutations): Trellis records that they occurred; it never automatically replays them. Automatic replay requires explicit `idempotent = true`, `idempotency_key`, `safe_to_retry = true`; default `safe_to_replay = false`, enforced at the type level where possible.

---

# 24. Process Architecture (v0.1)

```text
┌───────────────────────────────────┐
│             trellis (CLI)         │
│                                   │
│ Git/file reconciliation adapter   │
│ Program index (tree-sitter + SCIP)│
│ Dependency engine                 │
│ Validity engine + attestations    │
│ Completeness evaluator            │
│ SQLite metadata                   │
│ Local CAS                         │
└───────────────────────────────────┘
```

One binary. One process. No daemon, no watcher, no background workers, no gRPC, no PostgreSQL, no Docker.

Explicitly deferred with their entry points:

| Technology | Enters when |
|---|---|
| `trellisd` + gRPC (tonic, Tokio) | A real second-process consumer exists (post-benchmark-harness era) |
| PostgreSQL | Multi-agent/multi-worker coordination |
| Docker/OCI | Executable artifacts needing hermetic environments |
| vLLM | Controlled open-model benchmarking (Phase 3+) |
| PyTorch | Semantic validity training (Phase 3+, data first) |

Rust core (correctness-critical domain model, type-constrained illegal states, BLAKE3/tree-sitter/CAS fit); Python layer for agent adapters, benchmark harnesses, and later ML. Rayon/Tokio only where profiling or I/O concurrency justifies them; async is for waiting, not for CPU-bound analysis.

---

# 25. Benchmark Protocol (Four Layers)

Replaces the Draft 0.1 benchmark section entirely.

### Benchmark A — Oracle validity (synthetic fixture)

Measures: false-valid rate (target: zero on deterministic classes), stale recall, invalidation precision, abstention behavior (UNKNOWN issuance). Oracle labels from the mutation catalog with hand-verified expected validity.

### Benchmark B — Systems economics

Measures the economic premise directly: capture cost, reconciliation cost, projection validation cost, SCIP freeze cost, CAS/storage cost; p50/p95/p99 latencies; scaling with affected-frontier size. Validates `C_validate ≪ C_recompute` — the kill-criterion measurement.

### Benchmark C — Controlled agent (minimal loop)

**Frozen methodological constraint: both conditions expose the same logical tool API.**

```
                SAME LOGICAL TOOL API

                  callers(foo)
                    /      \
                   /        \
            baseline        Trellis
               │               │
        compute fresh      reuse if valid,
         every time        else compute
```

The independent variable is **incremental capture / validity / reuse — not tool capability**. Trellis may capture under the hood in both conditions; from the model's perspective, semantic capabilities are equivalent between conditions. Giving Trellis SCIP precision while baseline gets grep would invalidate the experiment: it would measure better program intelligence, not incremental maintenance.

Baseline ladder (each rung isolates one hypothesis):

```text
fresh computation
exact cache
file-level invalidation
memory / RAG
Trellis deterministic
eventually Trellis semantic
```

**Fairness contract.** Every rung of the ladder receives the **same semantic-query implementation** — `callers(foo)` executes the identical underlying caller query in every condition; only the reuse/invalidation strategy differs:

```text
fresh baseline    → same caller query, executed fresh
exact cache       → same caller query, exact-key caching
file invalidation → same caller query, file-dependency invalidation
Trellis           → same caller query, typed projection validity
```

Frozen controls (identical across all conditions and rungs):

```text
same context budget · same maximum steps · same timeout
same model configuration · same prompt/tool descriptions
same repository starting state · same semantic-query implementation
```

The memory/RAG rung receives the **same context budget** Trellis receives — RAG must not lose merely because it was allowed to over-stuff stale context, nor win under different budget semantics.

Paired repeated trials (same model, same harness, same repository state, same task); single runs are not evidence. Metrics: task correctness, wall-clock, input/output tokens, tool calls, repository searches, reuse/recompute counts, validation overhead, false-valid count. Prompt-cache control: record cached vs uncached tokens; attribute only fewer *logical repository/tool computations* to Trellis, never provider prefix-cache effects. North-star: **avoided recomputation at constant task correctness**; compounding metric `R_n` rising / discovery `D_n` falling across related-task sequences.

### Benchmark D — External validity

Pinned real OSS repositories (evolution replay), then an independent open agent harness. Never treated as an oracle unless an oracle is explicitly constructed.

---

# 26. Fixtures (Asymmetric From Day One)

- **Synthetic real-shaped Python repo** (~30–50 files: auth, payments, tests) — the **authoritative oracle fixture**. Exhaustive hand-labeled mutation catalog:

```text
R₀: refresh_token has no external callers
R₁: unrelated payment constant changes
R₂: new external caller added
R₃: caller implementation changes, caller set unchanged
R₄: authentication architecture changes
```

Carries Benchmark A ground truth, invalidation precision/recall, negative dependencies, adversarial mutations.

- **Pinned real OSS Python repo** — the **lightweight realism fixture**: index coverage, real package structure, scaling/integration smoke, external validity. Explicitly *not* a ground-truth oracle. Kept lightweight; the synthetic fixture remains the authoritative Phase 0 benchmark.

---

# 27. Milestones and Clock (Solo, Intensive)

**Frozen schedule:**

| Window | Deliverable |
|---|---|
| **Week 1** | Deterministic executable kernel: reconcile → index → projection → artifact → mutate → precise invalidation, with negative dependencies |
| **Weeks 2–3** | Credible Phase 0 benchmark (Benchmarks A + B on the synthetic fixture) |
| **Weeks 4–6** | Recognition-grade public release with real OSS evidence (lightweight OSS fixture results, reproducible benchmark) |

> If Phase 0 itself takes three months, Phase 0 was scoped too broadly.

Discipline: schemas frozen early; expected artifact behavior written **before** implementation of each mutation series.

**Phase ordering:**

```text
Phase 0    deterministic kernel (W1)
Phase 0.5  red/green propagation with value-unchanged cutoff (W2)
Phase 1    benchmark harness + minimal agent, same-tool-API Benchmark C (W2–6)
Phase 2    verifiers, structured facts, trust policies
Phase 3    semantic validity ML (mutation generator, labels, baselines, then models)
Phase 4    distributed runtime (trellisd/gRPC, PostgreSQL, remote CAS, workers)
Phase 5    cross-agent shared computation
```

gRPC/`trellisd` moved **after** the benchmark-harness era: the harness is the first consumer, driven through the CLI/SDK in-process. The boundary protocol enters when a genuine second-process consumer exists.

---

# 28. Success Gates and Kill Criteria

v0.1 success requires, in one reproducible benchmark:

1. Python repository indexed; symbol/reference/caller/implementation queries as typed projections.
2. Immutable artifacts with provenance, dependencies, and append-only attestations.
3. Precise change detection; unchanged projections stay green.
4. Negative-dependency invalidation (empty caller set becomes non-empty ⇒ STALE).
5. Red/green propagation with the value-unchanged cutoff.
6. Authoritative absence only under satisfied completeness requirements (§8).
7. `trellis explain <artifact-id>` shows the exact causal trace.
8. A second related task consumes still-valid artifacts from the first.
9. Against the baseline ladder at the same tool capability: correctness unchanged, exploration decreased, validation materially cheaper than avoided recomputation, **false-valid = 0** on deterministic classes.

**Kill criteria** — reconsider the architecture if:

- capture + validation overhead ≈ recomputation cost (Benchmark B),
- real coding tasks rarely reuse artifacts,
- dependency capture is too incomplete to prevent stale reuse,
- correctness requires whole-repository reanalysis on almost every mutation.

A technically sophisticated system that does not save real work is not valuable.

---

# 29. The Central System Invariant

Every automatically reused artifact must satisfy:

```
∀ dᵢ ∈ D(A):  dᵢ(W_current) = dᵢ(W_validated)
```

or possess a verifier proving the changed dependency does not invalidate the artifact's semantic contract — **and** hold an applicable, current attestation whose authority and capture trust satisfy reuse policy. For uncertain semantic dependencies, automatic trusted reuse is forbidden unless policy explicitly permits it.

Optimization statement:

```
minimize C_total = C_capture + C_validation + C_recomputation
subject to  P(stale artifact reused) < ε        (ε ≈ 0 initially)
```

The product experience: **the agent remembers only what is still true.**

---

# 30. Rejected Alternatives (Condensed)

- **Bazel/Dagger:** excellent explicit-input caching; agent computation's inputs are latent until captured as projections — the unsolved part Trellis addresses.
- **Salsa:** borrow the red/green algorithm; reject the deterministic-only computation model.
- **Temporal:** durable execution ≠ validity maintenance.
- **Redis/Kubernetes:** operational tools at scale, never sources of truth or conceptual architecture.
- **Vector DB:** similarity ≠ validity; relevance layer at most.
- **LLM-decides-validity:** the exact failure mode Trellis exists to prevent; LLMs may propose dependencies, summarize changes, prioritize verification, score uncertainty — never override deterministic contradiction.
- **Session-level caching:** too coarse in both directions; Trellis operates below session granularity.
- **Every-thought-an-artifact:** bookkeeping explosion with unverifiable semantics.

---

# 31. Technology Stack (v0.1 Core)

```text
Rust · Git · Tree-sitter · SCIP (adapter) · SQLite (WAL) · BLAKE3 · local filesystem CAS
```

Agent integration: Python SDK + benchmark harness (no gRPC yet). Semantic phase: Python/PyTorch. Distributed phase: PostgreSQL, tonic gRPC, OCI, object-store CAS, vLLM.

---

# 32. Definition of Done (v0.1)

All of §28 items 1–9 demonstrated in one reproducible benchmark run, plus:

- Benchmark B confirms the economic premise with headroom.
- Benchmark C run at equal tool capability, paired trials, against at least the first three ladder rungs.
- Attestation behavior demonstrated: provisional → authoritative upgrade by append; drift-induced UNKNOWN without authority mutation.
- Coverage-certificate behavior demonstrated: unsatisfied requirements turn an empty result into UNKNOWN, and universe growth dirties the query.

---

# 33. Implementation Order

Crates (`trellis-core`, `trellis-program`, `trellis-store`, `trellis-cas`, `trellis-engine`) are built in this order. **Oracle-first discipline is enforced:** the synthetic fixture and its expected validity labels exist **before** the machinery that must satisfy them, so "correct" is never defined as whatever Trellis happens to implement.

```text
M0  — domain invariants: IDs, Artifact, Derivation, Projection,
      Snapshot, ValidationAttestation, epistemic enums
M1  — deterministic source state: repository manifest, BLAKE3,
      snapshot, reconcile(snapshot, working_tree)
M2  — synthetic fixture + oracle: create R₀–R₄ BEFORE
      sophisticated invalidation exists
M3  — program projections: tree-sitter, normalized symbols,
      file/definition/signature queries
M4  — storage: SQLite, CAS, append-only attestations
M5  — dependency engine: projection observations, reverse indexes,
      dirty candidate discovery
M6  — red/green: reevaluation, value-equality cutoff, causal explanation
M7  — negative dependencies: universe descriptor, CoverageCertificate,
      Requirements/Capabilities, completeness evaluator
M8  — SCIP freeze adapter: batch ingest, authoritative semantic snapshot
M9  — provisional overlay (only after the authoritative path works)
M10 — Benchmark A + B
```

## First acceptance test

```text
S0:
    callers(refresh_token) = ∅
    completeness = COMPLETE

Artifact A:
    proposition: "refresh_token has no external callers"
    attestation: VALID / AUTHORITATIVE

Mutation:
    add payments/retry.py calling refresh_token

resolve(A) — expected trace:
    source reconciliation detects retry.py
    universe digest changes
    callers projection DIRTY
    completeness still satisfied
    callers reevaluated: ∅ → {payments.retry}
    contract fails → append STALE authoritative attestation

trellis explain A → exact cause visible
```

Red/green counterexample:

```text
Mutation:
    change body of payments.retry, caller relation identical

Expected:
    projection reevaluated
    caller-set semantic value unchanged
    absence-dependent downstream state does not propagate further
```

If both cases hold, the nucleus of Trellis exists. Agent integration begins only after this.

---

# Appendix A — Amendments Relative to Draft 0.1

1. **Query-time snapshot reconciliation** replaces continuous dirty tracking (§5). No watcher, no daemon; watchers later are speculative hints only.
2. **Validity split from authority.** `PROVISIONAL` is *not* a validity state; Authority is an orthogonal dimension (§11).
3. **CaptureTrust** added as an orthogonal provenance dimension, originating on the derivation (§11, §13).
4. **Reuse policy is explicitly multidimensional** — a function of validity, authority, verification level, capture trust, class, currency, completeness, relevance, and consumer policy (§15).
5. **Semantic-snapshot + live-overlay consistency semantics:** authoritative SCIP batches at freeze points; conservative provisional overlay between them (§9).
6. **Universe + coverage certificates** for completeness-sensitive queries, with the requirements/capabilities split and `Requirements(Q) ⊆ Capabilities(C)` evaluation (§8).
7. **Kind / verifier / derivation responsibility split:** kind = semantic interface, verifier = implementation (attachable post-hoc), derivation = provenance (§16).
8. **Benchmark section replaced** with the four-layer protocol (A oracle validity, B systems economics, C controlled agent with same-tool-API constraint and baseline ladder, D external validity) and paired repeated trials (§25).
9. **Authority modeled as immutable, append-only validation attestations** scoped to source+semantic snapshots; world drift affects currency, never historical authority (§12).
10. **Phase reorder:** gRPC/`trellisd` moved after the benchmark-harness era; harness (CLI/SDK) is the first consumer; solo-intensive W1 / W2–3 / W4–6 clock with the three-month scope-failure rule (§24, §27).
11. **Fixtures asymmetric from day one:** synthetic oracle + lightweight pinned-OSS realism fixture (§26).
12. **Review clarifications (Draft 1.0 final review — spec-tightening, no architecture change):** tightened freeze rule — freeze only on proven semantic incompatibility, never on lookup alone (§6); evidence-based `COMPLETE` capture-trust definition, Level-2 tools demoted to primary *mechanism* (§13, §21); attestation-transition wording — individual attestations never transition, history strengthens only by append (§12); Benchmark C fairness contract — equal semantic-query implementation across all ladder rungs plus frozen control list, RAG rung budgeted equally (§25); implementation order M0–M10 with oracle-first discipline and the two acceptance traces (§33). **Implementation authorized.**

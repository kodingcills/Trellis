//! Benchmark B — systems economics (spec §25 B, §28): the
//! kill-criterion measurement. Per-operation latencies (p50/p95/p99)
//! for source reconciliation, candidate discovery, projection
//! reevaluation (validation), SCIP freeze, CAS put/get, and attestation
//! append; scaling across affected-frontier sizes; the economic premise
//! C_validate ≪ C_recompute measured directly.

use std::collections::BTreeMap;
use std::time::Instant;

use trellis_core::artifact::{
    ArtifactEnvelope, ArtifactKind, CostRecord, Derivation, ProducerInfo,
};
use trellis_core::attestation::ValidationAttestation;
use trellis_core::ids::SnapshotId;
use trellis_core::ids::{ArtifactId, AttestationId, DerivationId};
use trellis_core::observation_id::canonical_observation_id;
use trellis_engine::prelude::*;
use trellis_program::prelude::{ModuleId, ProgramIndex, PythonSyntaxIndex};
use trellis_source::manifest::{build_manifest, ManifestOptions};
use trellis_source::reconcile::reconcile_against_working_tree;
use trellis_source::reconcile::ChangedSet;
use trellis_store::prelude::Store;

#[path = "harness/mod.rs"]
mod harness;

/// Results anchor to the workspace-root benchmarks/ dir (cargo runs
/// tests with the crate dir as CWD; CARGO_MANIFEST_DIR is the crate).
fn benchmarks_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("benchmarks")
}

use harness::*;

/// One timed sample set with percentiles.
struct Timings {
    label: String,
    samples_us: Vec<u128>,
}

impl Timings {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            samples_us: Vec::new(),
        }
    }

    fn measure(&mut self, f: impl FnOnce()) {
        let start = Instant::now();
        f();
        self.samples_us.push(start.elapsed().as_micros());
    }

    fn percentile(&self, p: f64) -> u128 {
        let mut sorted = self.samples_us.clone();
        sorted.sort_unstable();
        let idx = (((sorted.len() as f64 - 1.0) * p) as usize).min(sorted.len() - 1);
        sorted[idx]
    }

    fn render(&self) -> String {
        format!(
            "{:<44} p50={:>6}us  p95={:>6}us  p99={:>6}us  n={}",
            self.label,
            self.percentile(0.50),
            self.percentile(0.95),
            self.percentile(0.99),
            self.samples_us.len()
        )
    }
}

/// Synthesize a changed set of the given size by adding N distinct
/// Python files to a copy of the fixture tree (scale axis for B).
fn tree_with_n_added_files(base: &BTreeMap<String, String>, n: usize) -> BTreeMap<String, String> {
    let mut tree = base.clone();
    for i in 0..n {
        tree.insert(
            format!("bench/gen/mod_{i}.py"),
            format!(
                "\"\"\"Generated module {i} (benchmark).\"\"\"\n\nfrom auth.tokens import refresh_token\n\n\ndef bench_call_{i}(x: str) -> str:\n    return refresh_token(x)\n"
            ),
        );
    }
    tree
}

/// THE Benchmark B measurement: systems economics + the economic
/// premise. Writes the committed result artifact.
#[test]
fn benchmark_b_systems_economics() {
    let base = pristine_tree();

    // ── Setup: a seeded store at the pristine snapshot. ─────────────
    let work = tempfile::tempdir().expect("tempdir");
    let tree_root = work.path().join("tree");
    std::fs::create_dir_all(&tree_root).expect("tree root");
    write_tree(&tree_root, &base);
    let mut store = Store::open(work.path().join("metadata.db")).expect("store opens");
    let snapshot = SnapshotId::from_hash(digest_str("bench.b.base"));
    put_snapshot_row(&mut store, snapshot, None, BASE_TIMESTAMP);

    // Seed one observation to validate (the fixture callers projection).
    let index = PythonSyntaxIndex::index(&base);
    let source = FixtureSource::new(&index, &base);
    let projection = parse_projection("Callers(auth.tokens.refresh_token, Repository)");
    let value = source
        .evaluate(&projection)
        .expect("source evaluates")
        .expect("base proves the dependency");
    let obs = trellis_core::projection::ProjectionObservation::new(
        canonical_observation_id(&projection.id(), &digest_str(value.value()), &snapshot),
        projection.id(),
        snapshot,
        digest_str(value.value()),
        None,
    );
    store
        .record_observation_record(&projection, &obs, "auth/tokens.py")
        .expect("observation recorded");

    // ── Reconciliation cost (manifest build + reconcile) ────────────
    let mut t_reconcile = Timings::new("source reconciliation");
    for _ in 0..20 {
        let manifest = build_manifest(&ManifestOptions::new(&tree_root)).expect("manifest");
        t_reconcile.measure(|| {
            let _ = reconcile_against_working_tree(&manifest, &ManifestOptions::new(&tree_root))
                .expect("reconciles");
        });
    }

    // ── Candidate discovery cost (full recorded state). ─────────────
    let mut t_discovery = Timings::new("candidate discovery");
    for _ in 0..20 {
        let recorded = recorded_from_store(&store).expect("recorded loads");
        t_discovery.measure(|| {
            let _ = reevaluation_candidates(
                &ChangedSet {
                    added: vec!["bench/gen/mod_0.py".to_string()],
                    modified: vec![],
                    removed: vec![],
                },
                &recorded,
            );
        });
    }

    // ── Validation (projection reevaluation) cost. ──────────────────
    let mut t_validate = Timings::new("projection reevaluation (validate)");
    for _ in 0..20 {
        let index = PythonSyntaxIndex::index(&base);
        let src = FixtureSource::new(&index, &base);
        t_validate.measure(|| {
            let _ = src.evaluate(&projection).expect("source evaluates");
        });
    }

    // ── Recompute cost: FULL fixture reindex (cold computation). ────
    let mut t_recompute = Timings::new("full fixture reindex (recompute)");
    for _ in 0..10 {
        t_recompute.measure(|| {
            let idx = PythonSyntaxIndex::index(&base);
            for (module, _) in base.iter().take(30) {
                if let Some(m) = ModuleId::from_path(module) {
                    let _ = idx.imports(&m).proven();
                    let _ = idx.parse_status(&m).proven();
                }
            }
        });
    }

    // ── SCIP freeze cost (ingest of a fixture-shaped index). ────────
    let mut t_freeze = Timings::new("SCIP freeze (ingest + identity)");
    for _ in 0..10 {
        t_freeze.measure(|| {
            let _ = frozen_graph_identity();
        });
    }

    // ── CAS put/get + attestation append. ───────────────────────────
    let mut t_cas_put = Timings::new("CAS put (payload)");
    for i in 0..20 {
        let payload = format!("benchmark payload {i}").into_bytes();
        t_cas_put.measure(|| {
            store.put_blob(&payload).expect("blob");
        });
    }
    let mut t_attest = Timings::new("attestation append");
    let art = bench_artifact(&mut store, snapshot);
    for i in 0..10 {
        let att = bench_attestation(&art, snapshot, i);
        t_attest.measure(|| {
            store.append_attestation(&att).expect("attestation");
        });
    }

    // ── Scaling: frontier size vs discovery+validation cost. ────────
    let mut scaling = Vec::new();
    for frontier in [1usize, 8, 32] {
        let tree = tree_with_n_added_files(&base, frontier);
        let work_root = work.path().join("tree");
        write_tree(&work_root, &tree);
        let manifest = build_manifest(&ManifestOptions::new(&work_root)).expect("manifest");
        let mut t = Timings::new(&format!("frontier={frontier} reconcile+discover"));
        for _ in 0..10 {
            t.measure(|| {
                let changed =
                    reconcile_against_working_tree(&manifest, &ManifestOptions::new(&work_root))
                        .expect("reconciles");
                let recorded = recorded_from_store(&store).expect("recorded");
                let _ = reevaluation_candidates(&changed, &recorded);
            });
        }
        t.label = format!("frontier={frontier:>2} (reconcile+discover)");
        scaling.push(t.render());
    }

    // ── Economic premise: C_validate << C_recompute. ────────────────
    let c_validate = t_discovery.percentile(0.95) + t_validate.percentile(0.95);
    let c_recompute = t_recompute.percentile(0.95);
    let ratio = c_recompute as f64 / c_validate.max(1) as f64;

    let mut report = String::from(
        "Benchmark B — systems economics (synthetic fixture)\n\
         ====================================================\n\
         per-operation latencies (microseconds):\n",
    );
    for t in [
        &t_reconcile,
        &t_discovery,
        &t_validate,
        &t_recompute,
        &t_freeze,
        &t_cas_put,
        &t_attest,
    ] {
        report.push_str(&format!("  {}\n", t.render()));
    }
    report.push_str(&format!(
        "\nscaling (affected-frontier size):\n{}\n",
        scaling.join("\n    ")
    ));
    report.push_str(&format!(
        "\neconomic premise (kill-criterion measurement, §28):\n  \
         C_validate (p95 discovery+reevaluation) = {c_validate}us\n  \
         C_recompute (p95 full reindex)          = {c_recompute}us\n  \
         ratio C_recompute / C_validate          = {ratio:.1}x\n  \
         premise C_validate << C_recompute: {}\n",
        if c_validate < c_recompute {
            "HOLDS"
        } else {
            "FAILS"
        }
    ));

    std::fs::create_dir_all(benchmarks_dir()).expect("benchmarks dir");
    std::fs::write(benchmarks_dir().join("benchmark_b_results.txt"), report)
        .expect("results written");

    // The economic premise gate: validation must be materially cheaper.
    assert!(
        c_validate < c_recompute,
        "kill criterion approached: C_validate ({c_validate}us) >= C_recompute ({c_recompute}us)"
    );
}

fn frozen_graph_identity() -> trellis_core::ids::SemanticSnapshotId {
    // The M8 freeze path over a fixture-shaped index (reduced): ingest
    // + deterministic identity — the freeze-cost unit.
    use scip::types::{Metadata, SymbolInformation, ToolInfo};
    let mut metadata = Metadata::new();
    let mut tool = ToolInfo::new();
    tool.name = "scip-python".to_string();
    tool.version = "9.9.9".to_string();
    metadata.tool_info = Some(tool).into();
    let mut doc = scip::types::Document::new();
    doc.language = "python".to_string();
    doc.relative_path = "auth/tokens.py".to_string();
    let mut si = SymbolInformation::new();
    si.symbol = "scip-python pip python_auth 1.0.0 `auth/tokens.py`/refresh_token.".to_string();
    doc.symbols = vec![si];
    let index = scip::types::Index {
        metadata: Some(metadata).into(),
        documents: vec![doc],
        ..Default::default()
    };
    let (graph, _) = trellis_scip::ingest::ingest(&index);
    graph.semantic_snapshot_id(&trellis_scip::ScipIdentity::new("scip-python", "9.9.9"))
}

fn bench_attestation(
    artifact: &trellis_core::ids::ArtifactId,
    snapshot: trellis_core::ids::SnapshotId,
    seq: u64,
) -> trellis_core::attestation::ValidationAttestation {
    use trellis_core::validity::{Authority, Validity, VerificationLevel};
    let derivation = Derivation::new(
        DerivationId::from_hash(digest_str("bench.derivation")),
        vec![trellis_core::validity::CaptureChannel::TrellisTool],
        ProducerInfo::new("trellis-bench", "0.1.0", None).expect("producer"),
        BASE_TIMESTAMP,
    );
    let _ = seq;
    ValidationAttestation::for_derivation(
        AttestationId::from_hash(digest_str(&format!(
            "bench.att:{}:{}",
            snapshot.hash(),
            seq
        ))),
        *artifact,
        &derivation,
        snapshot,
        None,
        Validity::Valid,
        Authority::Authoritative,
        VerificationLevel::Structural,
        None,
        vec![],
        BASE_TIMESTAMP + seq,
    )
}

fn bench_artifact(
    store: &mut Store,
    snapshot: trellis_core::ids::SnapshotId,
) -> trellis_core::ids::ArtifactId {
    let art_id = ArtifactId::from_hash(digest_str("bench.artifact"));
    let derivation = Derivation::new(
        DerivationId::from_hash(digest_str("bench.derivation")),
        vec![trellis_core::validity::CaptureChannel::TrellisTool],
        ProducerInfo::new("trellis-bench", "0.1.0", None).expect("producer"),
        BASE_TIMESTAMP,
    );
    let blob = store.put_blob(b"payload").expect("blob");
    let envelope = ArtifactEnvelope::new(
        art_id,
        1,
        ArtifactKind::StructuralSet,
        blob,
        None,
        ProducerInfo::new("trellis-bench", "0.1.0", None).expect("producer"),
        derivation,
        vec![],
        snapshot,
        CostRecord::default(),
    )
    .expect("envelope valid");
    store.put_artifact(&envelope).expect("artifact persisted");
    art_id
}

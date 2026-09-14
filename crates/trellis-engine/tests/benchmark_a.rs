//! Benchmark A — oracle validity over the FULL M2 mutation catalog
//! (spec §25 A): every mutation chain runs through the production
//! red/green transition; every artifact consequence row is measured
//! against the oracle label. The committed result artifact reports
//! false-valid (gate: zero), stale recall, invalidation precision, and
//! abstention behavior. Oracle labels are read-only ground truth.

use trellis_core::validity::Validity;
use trellis_oracle::Catalog;

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

/// Run one mutation chain and collect every consequence row's actual
/// standing. Coverage-aware mutations (X5: the oracle's
/// coverage-degradation class) run with the coverage context describing
/// the POST-mutation world — the production path where §8.2 X5
/// semantics live (M7); all other chains run the plain M6 path.
fn run_chain(catalog: &Catalog, chain: &[String]) -> Vec<(String, String, Validity, Validity)> {
    let mut seeded = seed_base();
    let mut now = BASE_TIMESTAMP;
    let mut rows = Vec::new();
    for mid in chain {
        now += TIMESTAMP_STEP;
        let mutation = catalog
            .mutations
            .iter()
            .find(|m| &m.id == mid)
            .unwrap_or_else(|| panic!("{mid} in catalog"))
            .clone();
        let report = if mid == "X5" {
            // Coverage-aware mutation: the §8.2 X5 semantics live in
            // the coverage-context path (M7) — the certificate must
            // describe the post-mutation world.
            run_mutation_with(&mut seeded, &mutation, now, None, x5_coverage_hook)
        } else {
            run_mutation_ctx(&mut seeded, &mutation, now, None)
        };
        let _ = report;
        // Recompute consequence rows against the final snapshot.
        for consequence in &mutation.artifact_consequences {
            let id = seeded.artifact_ids[&consequence.artifact];
            let actual = latest_validity(&seeded.store, &id)
                .unwrap_or_else(|| panic!("{} keeps a standing", consequence.artifact));
            let expected = parse_validity_label(&consequence.after);
            rows.push((mid.clone(), consequence.artifact.clone(), expected, actual));
        }
    }
    rows
}

/// THE Benchmark A measurement: full-catalog oracle validity with the
/// zero-false-valid gate. Writes the committed result artifact.
#[test]
fn benchmark_a_oracle_validity_full_catalog() {
    let catalog = Catalog::load();
    let chains: Vec<Vec<String>> = vec![
        vec!["R1".into()],
        vec!["R2".into(), "R3".into()],
        vec!["R4".into()],
        vec!["X1".into()],
        vec!["X2".into()],
        vec!["X3".into()],
        vec!["X4".into()],
        vec!["X5".into()],
    ];

    let mut rows_total = 0usize;
    let mut matching = 0usize;
    let mut false_valid = 0usize;
    let mut oracle_invalid = 0usize;
    let mut engine_invalid = 0usize;
    let mut engine_invalid_correct = 0usize;
    let mut abstention_matches = 0usize;
    let mut per_row: Vec<String> = Vec::new();

    for chain in &chains {
        for (mid, artifact, expected, actual) in run_chain(&catalog, chain) {
            rows_total += 1;
            let row_matches = expected == actual;
            if row_matches {
                matching += 1;
            }
            let oracle_demands_invalid = expected != Validity::Valid;
            if oracle_demands_invalid {
                oracle_invalid += 1;
                if actual == Validity::Valid {
                    false_valid += 1;
                } else {
                    engine_invalid_correct += 1;
                }
            }
            if actual != Validity::Valid {
                engine_invalid += 1;
            }
            if expected == Validity::Unknown && actual == Validity::Unknown {
                abstention_matches += 1;
            }
            per_row.push(format!(
                "{mid:>3} {:<24} expected={expected:?} actual={actual:?} {}",
                artifact,
                if row_matches { "OK" } else { "MISMATCH" }
            ));
            assert!(
                row_matches,
                "oracle mismatch: {artifact} after {mid}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    let stale_recall = if oracle_invalid == 0 {
        1.0
    } else {
        engine_invalid_correct as f64 / oracle_invalid as f64
    };
    let precision = if engine_invalid == 0 {
        1.0
    } else {
        engine_invalid_correct as f64 / engine_invalid as f64
    };

    let report = format!(
        "Benchmark A — oracle validity (full M2 mutation catalog)\n\
         =========================================================\n\
         rows measured:        {rows_total}\n\
         rows matching labels: {matching}\n\
         false-valid:          {false_valid}   (gate: 0)\n\
         oracle-invalid rows:  {oracle_invalid}\n\
         stale recall:         {stale_recall:.4}\n\
         invalidation precision: {precision:.4}\n\
         abstention (UNKNOWN) matches: {abstention_matches}\n\
         \nper-row:\n{}\n",
        per_row.join("\n")
    );
    std::fs::create_dir_all(benchmarks_dir()).expect("benchmarks dir");
    std::fs::write(benchmarks_dir().join("benchmark_a_results.txt"), report)
        .expect("results written");
}

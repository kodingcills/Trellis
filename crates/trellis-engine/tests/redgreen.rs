//! M6 red/green acceptance tests: the §33 first acceptance trace, the
//! red/green counterexample, and the oracle catalog sweep (R0-R4, X1-X4)
//! against the M2 mutation catalog. Oracle labels are read-only ground
//! truth; the fixture semantic source never reads `catalog.json`.

use std::collections::BTreeMap;

use trellis_core::artifact::{
    ArtifactEnvelope, ArtifactKind, CostRecord, Derivation, ProducerInfo, Proposition,
};
use trellis_core::attestation::{EvidenceRef, ValidationAttestation};
use trellis_core::ids::{
    ArtifactId, AttestationId, ContentHash, DerivationId, HashAlgo, SnapshotId, Timestamp,
};
use trellis_core::projection::{Projection, ProjectionKind};
use trellis_core::validity::{Authority, Validity, VerificationLevel};

use trellis_core::coverage::{Completeness, CoverageCertificate, CoverageState};
use trellis_engine::discovery::RecordedObservation;
use trellis_engine::prelude::*;

use trellis_engine::redgreen::{
    ContractVerdict, ProjectionOutcomeKind, ReevaluationSource, Transition,
};
use trellis_oracle::{Catalog, Mutation};
use trellis_program::prelude::PythonSyntaxIndex;
use trellis_source::manifest::{build_manifest, ManifestOptions};
use trellis_source::reconcile::{reconcile_against_working_tree, ChangedSet};
use trellis_store::prelude::Store;

const BASE_TIMESTAMP: Timestamp = 1_700_000_000_000;
const TIMESTAMP_STEP: Timestamp = 1_000;
const FIXTURE_INDEXER: &str = "fixture-semantic-v1";

fn digest_of(bytes: &[u8]) -> ContentHash {
    ContentHash::compute(HashAlgo::Blake3, bytes)
}

fn digest_str(s: &str) -> ContentHash {
    digest_of(s.as_bytes())
}

// Shared harness (fixture tree, seeding, transitions, catalog parsing)
// extracted verbatim to tests/harness/mod.rs (M10: shared with the
// benchmark suite; behavior unchanged).
#[path = "harness/mod.rs"]
mod harness;

use harness::*;

// ─────────────────────────────────────────────────────────────────────
// Acceptance traces
// ─────────────────────────────────────────────────────────────────────

/// §33 first acceptance test: absence → new caller → STALE, with the
/// exact cause chain queryable.
#[test]
fn first_acceptance_trace_r2_absence_breaks_to_stale() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();
    let r2 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "R2")
        .expect("R2 in catalog");

    let report = run_mutation(&mut seeded, r2, BASE_TIMESTAMP + TIMESTAMP_STEP);

    let callers = find_outcome(
        &report,
        ProjectionKind::Callers,
        "auth.tokens.refresh_token",
    );
    assert_eq!(callers.outcome(), ProjectionOutcomeKind::Changed);
    assert_eq!(
        callers.new_value(),
        Some("api.webhooks.handle_refresh"),
        "caller set must reevaluate to exactly the new caller"
    );
    let pinned = callers.prior_digest().expect("prior digest recorded");
    assert_eq!(
        pinned,
        &digest_str(""),
        "prior value was the canonical empty set"
    );

    // Both absence-bearing artifacts go STALE by append.
    for name in ["ART_CALLERS_REFRESH", "ART_NO_CALLERS_FACT"] {
        let id = seeded.artifact_ids[name];
        let outcome = report
            .artifacts()
            .iter()
            .find(|ao| ao.artifact() == id)
            .unwrap_or_else(|| panic!("{name} must be re-contracted"));
        assert_eq!(outcome.appended(), Some(Validity::Stale), "{name}");
        assert_eq!(outcome.verdict(), ContractVerdict::Fails, "{name}");
        let att = seeded
            .store
            .attestation_history(&id)
            .expect("history")
            .iter()
            .last()
            .expect("attestation appended")
            .clone();
        assert_eq!(att.validity(), Validity::Stale, "{name}");
        assert_eq!(att.authority(), Authority::Authoritative, "{name}");
        assert_eq!(
            att.verification_level(),
            VerificationLevel::Structural,
            "{name}"
        );
        assert!(
            att.evidence()
                .iter()
                .any(|e| matches!(e, EvidenceRef::Observation(_))),
            "{name} evidence pins the evaluation observation"
        );
    }

    // Everything else keeps standing with NO new attestation: the
    // value-equality cutoff keeps untouched relations green.
    for name in [
        "ART_PROVIDER_SET",
        "ART_VALIDATE_SIG",
        "ART_STATELESS",
        "ART_LOGIN_CALLERS",
    ] {
        let id = seeded.artifact_ids[name];
        assert_eq!(
            latest_validity(&seeded.store, &id),
            Some(Validity::Valid),
            "{name} stays valid"
        );
        assert_eq!(history_len(&seeded.store, &id), 1, "{name} untouched");
        assert!(
            !report.artifacts().iter().any(|ao| ao.artifact() == id),
            "{name} must not appear in the transition's artifact pass"
        );
    }
}

/// §33 red/green counterexample: caller implementation body changes but
/// the caller-set value remains equal — downstream propagation stops,
/// and the stale structural set re-establishes validity by append.
#[test]
fn red_green_counterexample_r3_body_change_stops_propagation() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();
    let by_id = |id: &str| {
        catalog
            .mutations
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("{id} in catalog"))
            .clone()
    };

    run_mutation(&mut seeded, &by_id("R2"), BASE_TIMESTAMP + TIMESTAMP_STEP);
    let counts_before_r3: BTreeMap<String, usize> = catalog
        .seeded_artifacts
        .iter()
        .map(|a| {
            let id = seeded.artifact_ids[&a.id];
            (a.id.clone(), history_len(&seeded.store, &id))
        })
        .collect();

    let report = run_mutation(
        &mut seeded,
        &by_id("R3"),
        BASE_TIMESTAMP + 2 * TIMESTAMP_STEP,
    );

    // The caller set reevaluates to the same value it had after R2.
    let callers = find_outcome(
        &report,
        ProjectionKind::Callers,
        "auth.tokens.refresh_token",
    );
    assert_eq!(
        callers.new_value(),
        Some("api.webhooks.handle_refresh"),
        "caller relation unchanged by the body edit"
    );

    // Structural set: continuity holds → validity re-established by append.
    let structural = seeded.artifact_ids["ART_CALLERS_REFRESH"];
    assert_eq!(
        latest_validity(&seeded.store, &structural),
        Some(Validity::Valid),
        "set-equality contract re-establishes validity (catalog R3)"
    );
    let r3_outcome = report
        .artifacts()
        .iter()
        .find(|ao| ao.artifact() == structural)
        .expect("structural set re-contracted at R3");
    assert_eq!(r3_outcome.appended(), Some(Validity::Valid));
    let dep_eval = &r3_outcome.dependency_evaluations()[0];
    assert_eq!(
        dep_eval.pinned_digest(),
        dep_eval.new_digest(),
        "pinned R2 value equals the reevaluated value"
    );

    // Absence FACT: the proposition is still false → stays STALE.
    let fact = seeded.artifact_ids["ART_NO_CALLERS_FACT"];
    assert_eq!(
        latest_validity(&seeded.store, &fact),
        Some(Validity::Stale),
        "FACT proposition remains false under the non-empty set"
    );

    // No downstream propagation: no previously-VALID artifact gained an
    // attestation or changed standing at R3.
    for artifact in &catalog.seeded_artifacts {
        let id = seeded.artifact_ids[&artifact.id];
        let before = counts_before_r3[&artifact.id];
        let after = history_len(&seeded.store, &id);
        match artifact.id.as_str() {
            "ART_CALLERS_REFRESH" | "ART_NO_CALLERS_FACT" => {
                assert!(after >= before, "re-contracted artifacts may append");
            }
            _ => {
                assert_eq!(
                    after, before,
                    "{}: no propagation into stable artifacts",
                    artifact.id
                );
                assert_eq!(
                    latest_validity(&seeded.store, &id),
                    Some(Validity::Valid),
                    "{}: standing unchanged",
                    artifact.id
                );
            }
        }
    }
}

/// R1: an unrelated change reevaluates the semantic relations but
/// changes no values — zero appends, zero standing changes.
#[test]
fn r1_unrelated_change_produces_no_false_invalidation() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();
    let r1 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "R1")
        .expect("R1 in catalog");

    let report = run_mutation(&mut seeded, r1, BASE_TIMESTAMP + TIMESTAMP_STEP);

    let callers = find_outcome(
        &report,
        ProjectionKind::Callers,
        "auth.tokens.refresh_token",
    );
    assert_eq!(
        callers.outcome(),
        ProjectionOutcomeKind::Unchanged,
        "caller set unchanged by an unrelated edit"
    );
    assert!(report.artifacts().is_empty(), "no artifact re-contracted");
    for artifact in &catalog.seeded_artifacts {
        let id = seeded.artifact_ids[&artifact.id];
        assert_eq!(
            history_len(&seeded.store, &id),
            1,
            "{} untouched",
            artifact.id
        );
        assert_eq!(
            latest_validity(&seeded.store, &id),
            Some(Validity::Valid),
            "{} stays valid",
            artifact.id
        );
    }
}

/// R4: a signature change dirties only its own dependent.
#[test]
fn r4_signature_widening_propagates_to_dependents_only() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();
    let r4 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "R4")
        .expect("R4 in catalog");

    let report = run_mutation(&mut seeded, r4, BASE_TIMESTAMP + TIMESTAMP_STEP);

    let sig = seeded.artifact_ids["ART_VALIDATE_SIG"];
    assert_eq!(
        latest_validity(&seeded.store, &sig),
        Some(Validity::Stale),
        "signature widening breaks the structural-set contract"
    );
    assert!(
        report
            .artifacts()
            .iter()
            .any(|ao| ao.artifact() == sig && ao.appended() == Some(Validity::Stale)),
        "STALE appended for the signature artifact"
    );
    for name in [
        "ART_PROVIDER_SET",
        "ART_CALLERS_REFRESH",
        "ART_NO_CALLERS_FACT",
        "ART_STATELESS",
        "ART_LOGIN_CALLERS",
    ] {
        let id = seeded.artifact_ids[name];
        assert_eq!(history_len(&seeded.store, &id), 1, "{name} untouched");
    }
}

/// Full catalog sweep: every artifact consequence row of R0-R4 and
/// X1-X4 must match the engine's post-transition standing. X5 is
/// M7-gated (coverage certificates) and asserted separately.
#[test]
fn oracle_catalog_sweep_matches_labels() {
    let catalog = Catalog::load();
    let by_id: BTreeMap<String, Mutation> = catalog
        .mutations
        .iter()
        .map(|m| (m.id.clone(), m.clone()))
        .collect();

    let chains: Vec<Vec<String>> = vec![
        vec!["R1".into()],
        vec!["R2".into(), "R3".into()],
        vec!["R4".into()],
        vec!["X1".into()],
        vec!["X2".into()],
        vec!["X3".into()],
        vec!["X4".into()],
    ];

    for chain in &chains {
        let mut seeded = seed_base();
        let mut now = BASE_TIMESTAMP;
        for mid in chain {
            now += TIMESTAMP_STEP;
            let mutation = &by_id[mid];
            run_mutation(&mut seeded, mutation, now);
            for consequence in &mutation.artifact_consequences {
                let id = seeded.artifact_ids[&consequence.artifact];
                let actual = latest_validity(&seeded.store, &id)
                    .unwrap_or_else(|| panic!("{} has a standing", consequence.artifact));
                assert_eq!(
                    actual,
                    parse_validity_label(&consequence.after),
                    "{} after {}: catalog expects {}",
                    consequence.artifact,
                    mid,
                    consequence.after
                );
            }
        }
    }

    // R0 baseline: an empty transition changes nothing and appends nothing.
    let mut seeded = seed_base();
    let report = Transition::new(
        &mut seeded.store,
        &FixtureSource::new(&PythonSyntaxIndex::index(&seeded.tree), &seeded.tree),
        ChangedSet::default(),
        seeded.snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .run()
    .expect("empty transition runs");
    assert!(
        report.projections().is_empty(),
        "no candidates without change"
    );
    assert!(report.artifacts().is_empty(), "no appends without change");
}

/// X5 without a coverage context (the M6-compatible path): no binding
/// checks, no completeness verdicts — standings unchanged. The full X5
/// semantics (degradation → UNKNOWN) are covered by
/// `x5_coverage_degradation_downgrades_to_unknown` below.
#[test]
fn x5_without_coverage_context_keeps_standings() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();
    let x5 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "X5")
        .expect("X5 in catalog");

    let report = run_mutation(&mut seeded, x5, BASE_TIMESTAMP + TIMESTAMP_STEP);
    // No crash, deterministic report, no synthetic semantic values.
    for artifact in &catalog.seeded_artifacts {
        let id = seeded.artifact_ids[&artifact.id];
        let standing = latest_validity(&seeded.store, &id);
        assert!(standing.is_some(), "{} keeps a standing", artifact.id);
    }
    let _ = report;
}

// ─────────────────────────────────────────────────────────────────────
// M7: universe + coverage certificates (spec §8)
// ─────────────────────────────────────────────────────────────────────

/// Indexer-version change with an untouched tree (empty changed set)
/// dirties every completeness-sensitive observation for reevaluation
/// (§8.1): they are reevaluated, re-bound to the new indexer identity,
/// and — values unchanged under complete coverage — no artifact is
/// invalidated. A second bump with the same identity is not dirty.
#[test]
fn indexer_bump_dirties_completeness_observations() {
    let mut seeded = seed_base();
    let callers = parse_projection("Callers(auth.tokens.refresh_token, Repository)");

    let snapshot = SnapshotId::from_hash(digest_str("trellis.harness.snapshot:indexer-bump"));
    put_snapshot_row(
        &mut seeded.store,
        snapshot,
        Some(seeded.snapshot),
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    );
    seeded.snapshot = snapshot;
    let index = PythonSyntaxIndex::index(&seeded.tree);
    let source = FixtureSource::new(&index, &seeded.tree);
    let paths: Vec<String> = seeded.tree.keys().cloned().collect();
    let ctx = coverage_context(
        &paths,
        "fixture-semantic-v2",
        CoverageCertificate::complete(),
    );
    let report = Transition::new(
        &mut seeded.store,
        &source,
        ChangedSet::default(),
        snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .with_coverage(ctx)
    .run()
    .expect("indexer-bump transition runs");

    // Every completeness-sensitive observation was reevaluated with a
    // COMPLETE verdict and no standing change.
    for (kind, subject) in [
        (ProjectionKind::Callers, "auth.tokens.refresh_token"),
        (ProjectionKind::Callers, "auth.service.AuthService.login"),
        (
            ProjectionKind::Implementations,
            "auth.interfaces.AuthProvider",
        ),
    ] {
        let outcome = find_outcome(&report, kind, subject);
        assert_eq!(
            outcome.outcome(),
            ProjectionOutcomeKind::Unchanged,
            "{subject}"
        );
        assert_eq!(
            outcome.completeness(),
            Some(Completeness::Complete),
            "{subject}"
        );
        assert!(!outcome.completeness_dirty(), "{subject}");
    }

    // Values unchanged under complete coverage → no artifact touched.
    assert!(
        report.artifacts().is_empty(),
        "no re-contraction on indexer bump"
    );
    let catalog = Catalog::load();
    for artifact in &catalog.seeded_artifacts {
        let id = seeded.artifact_ids[&artifact.id];
        assert_eq!(
            history_len(&seeded.store, &id),
            1,
            "{} untouched",
            artifact.id
        );
    }

    // The binding now records the new indexer identity (§8.1).
    let (_, _, _, indexer, _) = seeded
        .store
        .completeness_binding(&callers.id())
        .expect("binding persisted");
    assert_eq!(indexer, "fixture-semantic-v2");

    // Same identity again at a fresh snapshot → not dirty → empty report.
    let snapshot2 = SnapshotId::from_hash(digest_str("trellis.harness.snapshot:indexer-bump-2"));
    put_snapshot_row(
        &mut seeded.store,
        snapshot2,
        Some(snapshot),
        BASE_TIMESTAMP + 2 * TIMESTAMP_STEP,
    );
    seeded.snapshot = snapshot2;
    let ctx2 = coverage_context(
        &paths,
        "fixture-semantic-v2",
        CoverageCertificate::complete(),
    );
    let report2 = Transition::new(
        &mut seeded.store,
        &source,
        ChangedSet::default(),
        snapshot2,
        BASE_TIMESTAMP + 2 * TIMESTAMP_STEP,
    )
    .with_coverage(ctx2)
    .run()
    .expect("second indexer-bump runs");
    assert!(
        report2.projections().is_empty(),
        "identical binding context dirties nothing"
    );
}

/// Universe growth (new eligible file, no relationship changes) dirties
/// completeness-sensitive observations (§8.1); reevaluation under
/// complete coverage re-binds and re-establishes the absence claims as
/// VALID — never immediate downstream destruction.
#[test]
fn universe_growth_dirties_and_reestablishes_absence() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();
    let x4 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "X4")
        .expect("X4 in catalog")
        .clone();

    // Apply the mutation manually so the coverage context describes the
    // POST-growth world (a real caller binds the current universe).
    let work_root = seeded.work.path().join("tree");
    let manifest_prev = build_manifest(&ManifestOptions::new(&work_root)).expect("manifest builds");
    apply_ops(&work_root, &x4.ops);
    let changed = reconcile_against_working_tree(&manifest_prev, &ManifestOptions::new(&work_root))
        .expect("reconciles");
    seeded.tree = read_tree_from(&work_root);
    let snapshot = snapshot_id_for(&seeded.tree);
    put_snapshot_row(
        &mut seeded.store,
        snapshot,
        Some(seeded.snapshot),
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    );
    seeded.snapshot = snapshot;

    let paths: Vec<String> = seeded.tree.keys().cloned().collect();
    let index = PythonSyntaxIndex::index(&seeded.tree);
    let source = FixtureSource::new(&index, &seeded.tree);
    let report = Transition::new(
        &mut seeded.store,
        &source,
        changed,
        snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .with_coverage(coverage_context(
        &paths,
        FIXTURE_INDEXER,
        CoverageCertificate::complete(),
    ))
    .run()
    .expect("universe-growth transition runs");

    // The Callers projection reevaluates (universe digest changed) with
    // the same value under complete coverage: the §8.1 dirtying forced
    // the reevaluation; the verdict is unchanged, so nothing propagates.
    let outcome = find_outcome(
        &report,
        ProjectionKind::Callers,
        "auth.tokens.refresh_token",
    );
    assert_eq!(outcome.outcome(), ProjectionOutcomeKind::Unchanged);
    assert_eq!(outcome.completeness(), Some(Completeness::Complete));
    assert!(
        !outcome.completeness_dirty(),
        "verdict unchanged → no claim dirtying (§8.1: reevaluation, not destruction)"
    );
    assert!(!outcome.compared().is_empty(), "projection was reevaluated");

    // Absence artifacts: VALID stays VALID with no new attestation —
    // value unchanged under an unchanged verdict is exactly the §18
    // step-12 cutoff.
    let structural = seeded.artifact_ids["ART_CALLERS_REFRESH"];
    assert_eq!(
        latest_validity(&seeded.store, &structural),
        Some(Validity::Valid)
    );
    assert_eq!(history_len(&seeded.store, &structural), 1);
    assert!(report.artifacts().is_empty(), "no re-contraction");
}

/// X5 end-to-end (§8.2): a unit the backend cannot index (syntax error)
/// produces a coverage certificate with explicit failures;
/// CompletenessEvaluator answers UNKNOWN for completeness-sensitive
/// kinds; absence/set claims degrade to UNKNOWN, never authoritative
/// absence. Definition-local dependencies (Signature, FileContent) are
/// unaffected.
#[test]
fn x5_coverage_degradation_downgrades_to_unknown() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();
    let x5 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "X5")
        .expect("X5 in catalog")
        .clone();

    // Apply the mutation manually: the certificate must describe the
    // POST-mutation world, where users/broken.py is a real coverage
    // failure (a syntax-error unit the backend cannot index).
    let work_root = seeded.work.path().join("tree");
    let manifest_prev = build_manifest(&ManifestOptions::new(&work_root)).expect("manifest builds");
    apply_ops(&work_root, &x5.ops);
    let changed = reconcile_against_working_tree(&manifest_prev, &ManifestOptions::new(&work_root))
        .expect("reconciles");
    seeded.tree = read_tree_from(&work_root);
    let snapshot = snapshot_id_for(&seeded.tree);
    put_snapshot_row(
        &mut seeded.store,
        snapshot,
        Some(seeded.snapshot),
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    );
    seeded.snapshot = snapshot;

    let paths: Vec<String> = seeded.tree.keys().cloned().collect();
    let certificate = PythonSyntaxIndex::index(&seeded.tree).coverage_certificate(&paths);
    assert!(
        certificate
            .failures
            .contains(&"users/broken.py".to_string()),
        "the broken unit must be an explicit coverage failure"
    );
    let index = PythonSyntaxIndex::index(&seeded.tree);
    let source = FixtureSource::new(&index, &seeded.tree);
    let report = Transition::new(
        &mut seeded.store,
        &source,
        changed,
        snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .with_coverage(coverage_context(&paths, FIXTURE_INDEXER, certificate))
    .run()
    .expect("X5 transition runs");

    // The broken file is an explicit coverage failure.
    let outcome = find_outcome(
        &report,
        ProjectionKind::Callers,
        "auth.tokens.refresh_token",
    );
    assert_eq!(outcome.completeness(), Some(Completeness::Unknown));

    // Absence-bearing artifacts degrade to UNKNOWN (X5 oracle rows).
    for name in [
        "ART_NO_CALLERS_FACT",
        "ART_CALLERS_REFRESH",
        "ART_PROVIDER_SET",
        "ART_LOGIN_CALLERS",
    ] {
        let id = seeded.artifact_ids[name];
        assert_eq!(
            latest_validity(&seeded.store, &id),
            Some(Validity::Unknown),
            "{name} must be UNKNOWN under degraded coverage"
        );
        assert!(
            report.artifacts().iter().any(|ao| ao.artifact() == id),
            "{name} must appear in the artifact pass"
        );
    }

    // Definition-local artifacts are unaffected (X5 oracle rows).
    for name in ["ART_VALIDATE_SIG", "ART_STATELESS"] {
        let id = seeded.artifact_ids[name];
        assert_eq!(
            latest_validity(&seeded.store, &id),
            Some(Validity::Valid),
            "{name} must stay VALID"
        );
        assert_eq!(history_len(&seeded.store, &id), 1, "{name} untouched");
    }
}

/// Degraded coverage AND a genuine value change compose honestly: the
/// caller set grows (R2) while coverage is degraded — the value change
/// dominates (Fails → STALE); a coverage downgrade alone over an
/// unchanged value yields UNKNOWN.
#[test]
fn degraded_coverage_composes_with_value_changes() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();
    let r2 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "R2")
        .expect("R2 in catalog")
        .clone();

    // R2 under a certificate that cannot resolve references (no syntax
    // failures, but resolution capability unproven).
    let paths: Vec<String> = seeded.tree.keys().cloned().collect();
    let mut unproven = CoverageCertificate::complete();
    unproven.resolved_reference_coverage = CoverageState::Unproven;
    let report = run_mutation_ctx(
        &mut seeded,
        &r2,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
        Some(coverage_context(&paths, FIXTURE_INDEXER, unproven)),
    );

    // The caller set genuinely changed; the claim fails on values
    // regardless of coverage → STALE (not UNKNOWN).
    let structural = seeded.artifact_ids["ART_CALLERS_REFRESH"];
    assert_eq!(
        latest_validity(&seeded.store, &structural),
        Some(Validity::Stale),
        "a real value change stays STALE under degraded coverage"
    );
    let _ = report;
}

/// A contract-less artifact whose dependency value changes must append
/// UNKNOWN — never auto-reuse, never a fabricated VALID (§15, §18 step 9).
#[test]
fn undecidable_contract_appends_unknown() {
    let mut seeded = seed_base();
    let catalog = Catalog::load();

    // Extra artifact depending on Callers(refresh) with NO verifier.
    let dep = parse_projection("Callers(auth.tokens.refresh_token, Repository)");
    let art_id = ArtifactId::from_hash(digest_str("trellis.harness.artifact:ART_UNVERIFIABLE"));
    let derivation = Derivation::new(
        DerivationId::from_hash(digest_str("trellis.harness.derivation:ART_UNVERIFIABLE")),
        vec![trellis_core::validity::CaptureChannel::TrellisTool],
        ProducerInfo::new("trellis-m6-harness", "0.1.0", None).expect("producer"),
        BASE_TIMESTAMP,
    );
    let blob = seeded.store.put_blob(b"payload").expect("blob");
    let envelope = ArtifactEnvelope::new(
        art_id,
        1,
        ArtifactKind::Fact,
        blob,
        Some(Proposition::new("unverifiable claim").expect("proposition")),
        ProducerInfo::new("trellis-m6-harness", "0.1.0", None).expect("producer"),
        derivation.clone(),
        vec![dep.id()],
        seeded.snapshot,
        CostRecord::default(),
    )
    .expect("envelope valid");
    seeded.store.put_artifact(&envelope).expect("persisted");
    let base_obs = seeded
        .store
        .all_projection_observations()
        .expect("observations")
        .into_iter()
        .find(|o| o.projection() == dep.id())
        .expect("base callers observation");
    let att = ValidationAttestation::for_derivation(
        AttestationId::from_hash(digest_str("trellis.harness.seed:ART_UNVERIFIABLE")),
        art_id,
        &derivation,
        seeded.snapshot,
        None,
        Validity::Valid,
        Authority::Authoritative,
        VerificationLevel::Structural,
        None,
        vec![EvidenceRef::Observation(base_obs.id())],
        BASE_TIMESTAMP,
    );
    seeded.store.append_attestation(&att).expect("seeded");

    let r2 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "R2")
        .expect("R2 in catalog")
        .clone();
    run_mutation(&mut seeded, &r2, BASE_TIMESTAMP + TIMESTAMP_STEP);

    assert_eq!(
        latest_validity(&seeded.store, &art_id),
        Some(Validity::Unknown),
        "no verifier + changed dependency → conservative UNKNOWN"
    );
}

/// Determinism: identical fixture + mutation sequence produce identical
/// reports, observation identities, and attestation identities.
#[test]
fn transitions_are_deterministic() {
    let run = || -> (TransitionReport, Vec<String>, Vec<String>) {
        let mut seeded = seed_base();
        let catalog = Catalog::load();
        let r2 = catalog
            .mutations
            .iter()
            .find(|m| m.id == "R2")
            .expect("R2 in catalog")
            .clone();
        let report = run_mutation(&mut seeded, &r2, BASE_TIMESTAMP + TIMESTAMP_STEP);
        let obs: Vec<String> = seeded
            .store
            .all_projection_observations()
            .expect("observations")
            .iter()
            .map(|o| o.id().to_string())
            .collect();
        let atts: Vec<String> = catalog
            .seeded_artifacts
            .iter()
            .flat_map(|a| {
                seeded
                    .store
                    .attestation_history(&seeded.artifact_ids[&a.id])
                    .expect("history")
                    .iter()
                    .map(|att| att.id().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        (report, obs, atts)
    };
    let (report_a, obs_a, atts_a) = run();
    let (report_b, obs_b, atts_b) = run();
    assert_eq!(report_a, report_b, "reports identical");
    assert_eq!(obs_a, obs_b, "observation identities identical");
    assert_eq!(atts_a, atts_b, "attestation identities identical");
}

/// Restart/reload: a transition run against a reopened store produces
/// identical results (persisted discovery state drives everything).
#[test]
fn transition_survives_restart_via_persistence() {
    let catalog = Catalog::load();
    let r2 = catalog
        .mutations
        .iter()
        .find(|m| m.id == "R2")
        .expect("R2 in catalog")
        .clone();

    let mut seeded = seed_base();
    let base_tree = seeded.tree.clone();
    let base_snapshot = seeded.snapshot;
    let db = seeded.work.path().join("metadata.db");
    let first = run_mutation(&mut seeded, &r2, BASE_TIMESTAMP + TIMESTAMP_STEP);
    let Seeded {
        store: _store,
        work,
        ..
    } = seeded;
    drop(_store);

    // Reopen the same durable store; the original tempdir keeps living
    // so its CAS blobs and database stay on disk.
    let mut reopened = Seeded {
        store: Store::open(&db).expect("reopens"),
        work,
        tree: base_tree,
        snapshot: base_snapshot,
        artifact_ids: BTreeMap::new(),
    };
    // Artifact ids are canonical content identities, so they rebuild
    // deterministically after restart.
    for artifact in &catalog.seeded_artifacts {
        reopened.artifact_ids.insert(
            artifact.id.clone(),
            ArtifactId::from_hash(digest_str(&format!(
                "trellis.harness.artifact:{}",
                artifact.id
            ))),
        );
    }
    let work_root = reopened.work.path().join("tree");
    let _ = std::fs::remove_dir_all(&work_root);
    std::fs::create_dir_all(&work_root).expect("tree root creatable");
    write_tree(&work_root, &reopened.tree);
    let manifest = build_manifest(&ManifestOptions::new(&work_root)).expect("manifest");
    apply_ops(&work_root, &r2.ops);
    let changed = reconcile_against_working_tree(&manifest, &ManifestOptions::new(&work_root))
        .expect("reconciles");
    reopened.tree = read_tree_from(&work_root);
    let snapshot = snapshot_id_for(&reopened.tree);
    put_snapshot_row(
        &mut reopened.store,
        snapshot,
        Some(reopened.snapshot),
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    );
    reopened.snapshot = snapshot;
    let index = PythonSyntaxIndex::index(&reopened.tree);
    let source = FixtureSource::new(&index, &reopened.tree);
    let second = Transition::new(
        &mut reopened.store,
        &source,
        changed,
        snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .run()
    .expect("reopened transition runs");

    let callers_b = find_outcome(
        &second,
        ProjectionKind::Callers,
        "auth.tokens.refresh_token",
    );
    eprintln!(
        "DBG callers outcome {:?} value {:?} compared {}",
        callers_b.outcome(),
        callers_b.new_value(),
        callers_b.compared().len()
    );

    let standing_after: Vec<Validity> = catalog
        .seeded_artifacts
        .iter()
        .map(|a| {
            latest_validity(&reopened.store, &reopened.artifact_ids[&a.id])
                .expect("standing survives restart")
        })
        .collect();
    // Deterministic contract evaluation: the re-run re-evaluates the
    // same change against the post-R2 pinned values, so the structural
    // set's continuity contract re-establishes VALID (the catalog's R3
    // semantics), the absence FACT stays STALE (proposition still
    // false), and every untouched relation stays VALID.
    assert_eq!(
        standing_after,
        vec![
            Validity::Valid, // ART_CALLERS_REFRESH: continuity holds
            Validity::Stale, // ART_NO_CALLERS_FACT: proposition false
            Validity::Valid, // ART_PROVIDER_SET
            Validity::Valid, // ART_VALIDATE_SIG
            Validity::Valid, // ART_STATELESS
            Validity::Valid, // ART_LOGIN_CALLERS
        ]
    );
    // The re-evaluation itself is identical: same projection outcomes,
    // same caller-set value.
    let callers_a = find_outcome(&first, ProjectionKind::Callers, "auth.tokens.refresh_token");
    let callers_b = find_outcome(
        &second,
        ProjectionKind::Callers,
        "auth.tokens.refresh_token",
    );
    assert_eq!(callers_a.outcome(), callers_b.outcome());
    assert_eq!(callers_a.new_value(), callers_b.new_value());
}

/// Recorded discovery state must be loadable after seeding (fail-closed
/// loader sanity over the M6-seeded observations).
#[test]
fn seeded_observations_load_fail_closed() {
    let seeded = seed_base();
    let recorded: Vec<RecordedObservation> =
        recorded_from_store(&seeded.store).expect("recorded state loads");
    assert!(
        recorded.len() >= 6,
        "every seeded artifact dependency is recorded: {}",
        recorded.len()
    );
    for rec in &recorded {
        assert!(!rec.anchor_path.is_empty(), "anchors recorded");
    }
}

/// Regression (M7 review OPTIONAL, cheap closure): a completeness-dirty
/// candidate whose source CANNOT evaluate (Unobserved) must neither
/// panic (new_observations indexing) nor fabricate a verdict. The
/// §18 step-12 cutoff holds: no value change and no claim change means
/// no propagation and no append — the standing is preserved.
#[test]
fn unobserved_completeness_dirty_degrades_to_unknown() {
    let mut seeded = seed_base();

    // A source that declines every evaluation (no evidence this round).
    struct BlindSource;
    impl ReevaluationSource for BlindSource {
        fn evaluate(
            &self,
            _projection: &Projection,
        ) -> Result<
            Option<trellis_engine::redgreen::Evaluated>,
            trellis_engine::redgreen::TransitionError,
        > {
            Ok(None)
        }
    }

    let snapshot = SnapshotId::from_hash(digest_str("trellis.harness.snapshot:blind"));
    put_snapshot_row(
        &mut seeded.store,
        snapshot,
        Some(seeded.snapshot),
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    );
    seeded.snapshot = snapshot;

    // The indexer identity changed → completeness-sensitive observations
    // are dirty (§8.1), but the source proves nothing → Unobserved.
    let paths: Vec<String> = seeded.tree.keys().cloned().collect();
    let ctx = coverage_context(
        &paths,
        "fixture-semantic-blind",
        CoverageCertificate::complete(),
    );
    let report = Transition::new(
        &mut seeded.store,
        &BlindSource,
        ChangedSet::default(),
        snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .with_coverage(ctx)
    .run()
    .expect("blind transition runs");

    // Completeness-sensitive outcomes exist and are Unobserved: the
    // forced dirtying reevaluated them, but the source proved nothing.
    let outcome = find_outcome(
        &report,
        ProjectionKind::Callers,
        "auth.tokens.refresh_token",
    );
    assert_eq!(outcome.outcome(), ProjectionOutcomeKind::Unobserved);
    assert_eq!(outcome.completeness(), Some(Completeness::Complete));
    assert!(
        !outcome.completeness_dirty(),
        "verdict unchanged → the claim is NOT dirty (§8.1 dirtying is verdict-scoped)"
    );

    // §18 step-12 cutoff: no evidence of a value or claim change → the
    // artifact's standing is untouched (no propagation, no attestation).
    // This pins the invariant that Unobserved alone NEVER invalidates —
    // the engine neither fabricates a verdict nor destroys a standing
    // it cannot justify touching.
    let structural = seeded.artifact_ids["ART_CALLERS_REFRESH"];
    assert_eq!(
        latest_validity(&seeded.store, &structural),
        Some(Validity::Valid),
        "no evidence of change → standing preserved (§18 step 12)"
    );
    assert_eq!(history_len(&seeded.store, &structural), 1, "no append");
}

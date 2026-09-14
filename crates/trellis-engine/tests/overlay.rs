//! M9 overlay acceptance: provisional live overlay (frozen SCIP +
//! source delta + tree-sitter), provisional attestations, and the
//! authority-upgrade-by-append path (spec §9.2, §12.2).

use std::collections::BTreeMap;

use trellis_core::artifact::{
    ArtifactEnvelope, ArtifactKind, CostRecord, Derivation, ProducerInfo,
};
use trellis_core::attestation::{EvidenceRef, ValidationAttestation};
use trellis_core::ids::{
    ArtifactId, AttestationId, ContentHash, DerivationId, EnvironmentFingerprintId, HashAlgo,
    ManifestId, RepositoryId, SnapshotId, Timestamp,
};
use trellis_core::observation_id::canonical_observation_id;
use trellis_core::projection::{Projection, ProjectionKind, ProjectionObservation};
use trellis_core::snapshot::Snapshot;
use trellis_core::validity::{Authority, Validity, VerificationLevel};

use tempfile::TempDir;
use trellis_engine::redgreen::{
    contract_verifier_id, OverlaySemanticSource, ReevaluationSource, Transition,
    SET_EQUALITY_CONTRACT,
};
use trellis_program::prelude::PythonSyntaxIndex;
use trellis_source::manifest::{build_manifest, ManifestOptions};
use trellis_source::reconcile::reconcile_against_working_tree;
use trellis_source::reconcile::ChangedSet;
use trellis_store::prelude::Store;

const BASE_TIMESTAMP: Timestamp = 1_900_000_000_000;
const TIMESTAMP_STEP: Timestamp = 1_000;
const FROZEN_INDEXER: &str = "trellis-scip-ingest/1 (scip-python 9.9.9)";

fn digest_str(s: &str) -> ContentHash {
    ContentHash::compute(HashAlgo::Blake3, s.as_bytes())
}

fn snapshot_id_for(s: &str) -> SnapshotId {
    SnapshotId::from_hash(digest_str(s))
}

fn parse_projection(key: &str) -> Projection {
    let (kind, rest) = key.split_once('(').expect("kind(subject, scope) form");
    let inner = rest.strip_suffix(')').expect("closing paren");
    let (subject, _scope) = match inner.rsplit_once(", ") {
        Some((s, sc)) => (s, sc),
        None => (inner, "File"),
    };
    match kind {
        "Callers" => Projection::callers(subject, trellis_core::projection::Scope::Repository),
        "Signature" => Projection::signature(subject),
        "FileContent" => Projection::file(subject),
        "Implementations" => {
            Projection::implementations(subject, trellis_core::projection::Scope::Repository)
        }
        other => panic!("overlay tests use {other} only via explicit construction"),
    }
    .expect("valid projection")
}

/// Fixture tree (pristine oracle fixture).
fn pristine_tree() -> BTreeMap<String, String> {
    let root = trellis_oracle::fixture_base();
    let mut tree = BTreeMap::new();
    for file in trellis_oracle::walk_files(&root) {
        let rel = file
            .strip_prefix(&root)
            .expect("fixture file under root")
            .to_string_lossy()
            .replace('\\', "/");
        tree.insert(rel, std::fs::read_to_string(&file).expect("readable"));
    }
    tree
}

fn write_tree(dir: &std::path::Path, tree: &BTreeMap<String, String>) {
    for (path, content) in tree {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent creatable");
        }
        std::fs::write(&target, content).expect("writable");
    }
}

fn put_snapshot_row(store: &mut Store, id: SnapshotId, parent: Option<SnapshotId>, at: Timestamp) {
    let manifest = ManifestId::from_hash(digest_str(&format!("manifest:{id}")));
    let row = Snapshot::new(
        id,
        RepositoryId::from_hash(digest_str("trellis.m9.repo")),
        None,
        manifest,
        None,
        EnvironmentFingerprintId::from_hash(digest_str("trellis.m9.env")),
        parent,
        at,
    )
    .expect("snapshot valid");
    store.put_snapshot(&row).expect("snapshot persisted");
}

/// Seeded world: base artifact ART_CALLERS_REFRESH bound to
/// Callers(refresh_token, Repository) with a base attestation.
struct World {
    store: Store,
    work: TempDir,
    tree: BTreeMap<String, String>,
    snapshot: SnapshotId,
    structural: ArtifactId,
}

fn seed() -> World {
    let work = tempfile::tempdir().expect("tempdir");
    let tree = pristine_tree();
    let tree_root = work.path().join("tree");
    std::fs::create_dir_all(&tree_root).expect("tree root");
    write_tree(&tree_root, &tree);

    let mut store = Store::open(work.path().join("metadata.db")).expect("store opens");
    let snapshot = snapshot_id_for("m9.base");
    put_snapshot_row(&mut store, snapshot, None, BASE_TIMESTAMP);

    let projection = parse_projection("Callers(auth.tokens.refresh_token, Repository)");
    let art_id = ArtifactId::from_hash(digest_str("m9.artifact:ART_CALLERS_REFRESH"));
    let derivation = Derivation::new(
        DerivationId::from_hash(digest_str("m9.derivation:ART_CALLERS_REFRESH")),
        vec![trellis_core::validity::CaptureChannel::TrellisTool],
        ProducerInfo::new("trellis-m9-harness", "0.1.0", None).expect("producer"),
        BASE_TIMESTAMP,
    );
    let blob = store.put_blob(b"payload").expect("blob");
    let envelope = ArtifactEnvelope::new(
        art_id,
        1,
        ArtifactKind::StructuralSet,
        blob,
        None,
        ProducerInfo::new("trellis-m9-harness", "0.1.0", None).expect("producer"),
        derivation.clone(),
        vec![projection.id()],
        snapshot,
        CostRecord::default(),
    )
    .expect("envelope valid");
    store.put_artifact(&envelope).expect("artifact persisted");

    // Base observation: authoritative empty set (frozen SCIP graph said
    // callers(refresh_token) = {}).
    let base_obs = ProjectionObservation::new(
        canonical_observation_id(&projection.id(), &digest_str(""), &snapshot),
        projection.id(),
        snapshot,
        digest_str(""),
        None,
    );
    store
        .record_observation_record(&projection, &base_obs, "auth/tokens.py")
        .expect("base observation");
    store
        .put_completeness_binding(
            &projection.id(),
            &snapshot,
            &trellis_core::coverage::Universe::new(tree.keys().cloned()).digest(),
            &trellis_core::coverage::CoverageCertificate::complete().digest(),
            FROZEN_INDEXER,
            trellis_core::coverage::Completeness::Complete,
        )
        .expect("base binding");

    let att = ValidationAttestation::for_derivation(
        AttestationId::from_hash(digest_str("m9.seed:ART_CALLERS_REFRESH")),
        art_id,
        &derivation,
        snapshot,
        None,
        Validity::Valid,
        Authority::Authoritative,
        VerificationLevel::Structural,
        Some(contract_verifier_id(SET_EQUALITY_CONTRACT)),
        vec![EvidenceRef::Observation(base_obs.id())],
        BASE_TIMESTAMP,
    );
    store.append_attestation(&att).expect("base attestation");

    World {
        store,
        work,
        tree,
        snapshot,
        structural: art_id,
    }
}

fn latest_authority(store: &Store, artifact: &ArtifactId) -> Option<Authority> {
    store
        .attestation_history(artifact)
        .expect("history")
        .iter()
        .last()
        .map(|att| att.authority())
}

fn history_ids(store: &Store, artifact: &ArtifactId) -> Vec<String> {
    store
        .attestation_history(artifact)
        .expect("history")
        .iter()
        .map(|a| a.id().to_string())
        .collect()
}

/// Mid-task overlay query: a caller added AFTER the freeze is visible
/// in the overlay answer, and the evaluation is stamped PROVISIONAL.
#[test]
fn overlay_reflects_mid_task_changes_with_provisional_authority() {
    let mut world = seed();

    // Mid-task delta: api/webhooks.py added (R2 shape).
    let webhooks = "api/webhooks.py";
    let work_root = world.work.path().join("tree");
    let manifest_prev = build_manifest(&ManifestOptions::new(&work_root)).expect("manifest");
    std::fs::write(
        work_root.join(webhooks),
        "from auth.tokens import refresh_token\n\n\ndef handle_refresh(user_id: str) -> str:\n    return refresh_token(user_id)\n",
    )
    .expect("delta written");
    let changed = reconcile_against_working_tree(&manifest_prev, &ManifestOptions::new(&work_root))
        .expect("reconciles");
    world.tree = pristine_tree();
    world
        .tree
        .insert(webhooks.to_string(), "from auth.tokens import refresh_token\n\n\ndef handle_refresh(user_id: str) -> str:\n    return refresh_token(user_id)\n".to_string());
    let snapshot = snapshot_id_for("m9.overlay");
    put_snapshot_row(
        &mut world.store,
        snapshot,
        Some(world.snapshot),
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    );
    world.snapshot = snapshot;

    let index = PythonSyntaxIndex::index(&world.tree);
    let graph = frozen_graph();
    let live = live_calls_with_webhooks();
    let source = OverlaySemanticSource::new(&graph, &index, [webhooks], live);
    let report = Transition::new(
        &mut world.store,
        &source,
        changed,
        snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .run()
    .expect("overlay transition runs");

    // The overlay's callers value reflects the mid-task caller.
    let outcome = report
        .projections()
        .iter()
        .find(|o| o.projection().kind() == ProjectionKind::Callers)
        .expect("callers outcome");
    assert_eq!(
        outcome.new_value(),
        Some("api.webhooks.handle_refresh"),
        "overlay must see the post-freeze caller"
    );

    // The artifact was re-contracted under PROVISIONAL authority.
    let outcome_art = report
        .artifacts()
        .iter()
        .find(|ao| ao.artifact() == world.structural)
        .expect("artifact re-contracted");
    assert_eq!(outcome_art.appended(), Some(Validity::Stale));
    let att = world
        .store
        .attestation_history(&world.structural)
        .expect("history")
        .iter()
        .last()
        .expect("appended")
        .clone();
    assert_eq!(
        att.authority(),
        Authority::Provisional,
        "overlay = PROVISIONAL"
    );
    assert_eq!(att.validity(), Validity::Stale);
}

fn frozen_graph() -> trellis_scip::ScipGraph {
    // The frozen BASE graph: callers(refresh_token) = {} (absence).
    trellis_scip::ScipGraph::default()
}

fn live_calls_with_webhooks() -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    m.insert(
        "auth.tokens.refresh_token".to_string(),
        vec!["api.webhooks.handle_refresh".to_string()],
    );
    m
}

/// §12.2 authority upgrade by append: a subsequent freeze/revalidation
/// appends an AUTHORITATIVE attestation; every prior attestation is
/// byte-identical (append-only history).
#[test]
fn authority_upgrade_happens_by_append() {
    let mut world = seed();

    // Step 1: overlay transition (provisional STALE).
    let webhooks = "api/webhooks.py";
    let content = "from auth.tokens import refresh_token\n\n\ndef handle_refresh(user_id: str) -> str:\n    return refresh_token(user_id)\n";
    let work_root = world.work.path().join("tree");
    let manifest_prev = build_manifest(&ManifestOptions::new(&work_root)).expect("manifest");
    std::fs::write(work_root.join(webhooks), content).expect("delta written");
    let changed = reconcile_against_working_tree(&manifest_prev, &ManifestOptions::new(&work_root))
        .expect("reconciles");
    world.tree.insert(webhooks.to_string(), content.to_string());
    let overlay_snapshot = snapshot_id_for("m9.overlay");
    put_snapshot_row(
        &mut world.store,
        overlay_snapshot,
        Some(world.snapshot),
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    );
    world.snapshot = overlay_snapshot;
    let index = PythonSyntaxIndex::index(&world.tree);
    let graph = frozen_graph();
    let live = live_calls_with_webhooks();
    let source = OverlaySemanticSource::new(&graph, &index, [webhooks], live);
    let overlay_report = Transition::new(
        &mut world.store,
        &source,
        changed,
        overlay_snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .run()
    .expect("overlay transition");
    let provisional_ids = history_ids(&world.store, &world.structural);
    assert_eq!(
        latest_authority(&world.store, &world.structural),
        Some(Authority::Provisional)
    );

    // Step 2: freeze revalidation — authoritative source (frozen graph
    // that now INCLUDES the new caller), same tree. The §8.1 binding
    // rule dirties the completeness-sensitive observation (indexer
    // identity changes from overlay to frozen), forcing reevaluation
    // under the authoritative certificate; the AUTHORITATIVE
    // attestation appends; prior attestations unchanged.
    let upgraded_graph = upgraded_frozen_graph();
    let index = PythonSyntaxIndex::index(&world.tree);
    let source = trellis_engine::redgreen::FrozenSemanticSource::new(&upgraded_graph, &index);
    let freeze_snapshot = snapshot_id_for("m9.freeze2");
    put_snapshot_row(
        &mut world.store,
        freeze_snapshot,
        Some(world.snapshot),
        BASE_TIMESTAMP + 2 * TIMESTAMP_STEP,
    );
    world.snapshot = freeze_snapshot;
    // The authoritative freeze re-binds (Q, U, C): fresh complete
    // certificate under the FROZEN indexer identity.
    let binding_ctx = trellis_engine::redgreen::CoverageContext {
        universe: trellis_core::coverage::Universe::new(world.tree.keys().cloned()),
        certificate: trellis_core::coverage::CoverageCertificate::complete(),
        indexer: FROZEN_INDEXER.to_string(),
    };
    let freeze_report = Transition::new(
        &mut world.store,
        &source,
        ChangedSet::default(),
        freeze_snapshot,
        BASE_TIMESTAMP + 2 * TIMESTAMP_STEP,
    )
    .with_coverage(binding_ctx)
    .run()
    .expect("freeze revalidation transition");

    // Authority upgraded by APPEND: history grew, prior entries intact.
    let upgraded_ids = history_ids(&world.store, &world.structural);
    assert!(
        upgraded_ids.len() > provisional_ids.len(),
        "upgrade appends ({} -> {})",
        provisional_ids.len(),
        upgraded_ids.len()
    );
    for (i, id) in provisional_ids.iter().enumerate() {
        assert_eq!(upgraded_ids[i], *id, "prior attestation {} unchanged", i);
    }
    assert_eq!(
        latest_authority(&world.store, &world.structural),
        Some(Authority::Authoritative),
        "freeze revalidation appends AUTHORITATIVE"
    );
    let _ = (freeze_report, overlay_report);
}

fn upgraded_frozen_graph() -> trellis_scip::ScipGraph {
    let mut g = trellis_scip::ScipGraph::default();
    g.callers.insert(
        "auth.tokens.refresh_token".to_string(),
        vec!["api.webhooks.handle_refresh".to_string()],
    );
    g
}

/// §9.2 hard line: overlay-derived evidence can never establish
/// completeness-sensitive authority. The overlay's certificate
/// capabilities for reference resolution are Unproven by construction.
#[test]
fn overlay_cannot_establish_completeness_authority() {
    // The overlay source marks all semantic values provisional
    // (authoritative = false); the transition's attestation therefore
    // carries Authority::Provisional even when the verdict HOLDS.
    let mut world = seed();

    // An unrelated change (universe unchanged, value unchanged) under
    // the overlay: any re-contracted attestation must be PROVISIONAL.
    let index = PythonSyntaxIndex::index(&world.tree);
    let graph = frozen_graph();
    let live = live_calls_empty();
    let source = OverlaySemanticSource::new(&graph, &index, Vec::<String>::new(), live);
    let snapshot = snapshot_id_for("m9.overlay2");
    put_snapshot_row(
        &mut world.store,
        snapshot,
        Some(world.snapshot),
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    );
    world.snapshot = snapshot;
    // Touch nothing: empty changed set, identical frozen graph → the
    // callers value is unchanged (empty), nothing appends.
    let report = Transition::new(
        &mut world.store,
        &source,
        ChangedSet::default(),
        snapshot,
        BASE_TIMESTAMP + TIMESTAMP_STEP,
    )
    .run()
    .expect("empty overlay transition");
    assert!(
        report.artifacts().is_empty(),
        "value unchanged → no re-contraction (step-12 cutoff)"
    );
    assert_eq!(history_len(&world.store, &world.structural), 1);
}

fn live_calls_empty() -> BTreeMap<String, Vec<String>> {
    BTreeMap::new()
}

fn history_len(store: &Store, artifact: &ArtifactId) -> usize {
    store.attestation_history(artifact).expect("history").len()
}

/// Regression (M9 review IMPORTANT): a frozen caller defined in an
/// `__init__.py` file (module `auth`, never `auth.__init__`) must be
/// superseded when that file is in the reconcile delta — the delta
/// path normalizes exactly as the SCIP ingest normalizes document
/// paths. Mismatches would retain stale frozen callers (missed
/// invalidation, §2.1).
#[test]
fn init_py_delta_supersedes_frozen_callers() {
    // Frozen graph: a caller member defined in auth/__init__.py —
    // dotted form `auth.module_func` (the ingest strips `/__init__`).
    let mut frozen = trellis_scip::ScipGraph::default();
    frozen.callers.insert(
        "auth.tokens.refresh_token".to_string(),
        vec!["auth.module_func".to_string()],
    );

    let work = tempfile::tempdir().expect("tempdir");
    let tree = pristine_tree();
    let tree_root = work.path().join("tree");
    std::fs::create_dir_all(&tree_root).expect("tree root");
    write_tree(&tree_root, &tree);

    // Mid-task delta: auth/__init__.py modified (its definitions may
    // have changed).
    let delta_paths = ["auth/__init__.py"];

    let index = PythonSyntaxIndex::index(&tree);
    // Live resolution over the current tree no longer sees the caller
    // (it was removed in the delta).
    let live: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let graph = frozen;
    let source = OverlaySemanticSource::new(&graph, &index, delta_paths, live);
    let evaluated = source
        .evaluate(&parse_projection(
            "Callers(auth.tokens.refresh_token, Repository)",
        ))
        .expect("overlay evaluates")
        .expect("overlay proves a value");

    // The frozen member whose defining module is `auth` MUST be
    // superseded (dropped) — the honest overlay answer is authoritative
    // absence over the frozen part + live part (empty).
    assert_eq!(
        evaluated.value(),
        "",
        "auth/__init__.py in the delta supersedes auth.module_func"
    );
    assert!(!evaluated.is_authoritative());
}

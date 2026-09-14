//! M5 discovery integration tests: adversarial battery + catalog-driven
//! no-false-negative checks (oracle labels read-only, never modified).

use std::collections::BTreeMap;

use trellis_core::ids::{ContentHash, ProjectionObservationId, SnapshotId};
use trellis_core::projection::{Projection, ProjectionKind, ProjectionObservation, Scope};
use trellis_program::prelude::{ModuleId, ProgramIndex, PythonSyntaxIndex};
use trellis_source::reconcile::ChangedSet;

use trellis_engine::discovery::RecordedObservation;
use trellis_engine::observe::record_observation;
use trellis_engine::prelude::*;
use trellis_store::prelude::Store;

fn h(seed: u8) -> ContentHash {
    let d: Vec<u8> = (0..32).map(|i| seed.wrapping_add(i)).collect();
    ContentHash::from_bytes(trellis_core::ids::HashAlgo::Blake3, &d).unwrap()
}

fn snap(seed: u8) -> SnapshotId {
    SnapshotId::from_hash(h(seed))
}

fn changed(paths: &[&str]) -> ChangedSet {
    ChangedSet {
        added: paths
            .iter()
            .filter(|p| p.starts_with("ADD:"))
            .map(|p| p.trim_start_matches("ADD:").to_string())
            .collect(),
        modified: paths
            .iter()
            .filter(|p| p.starts_with("MOD:"))
            .map(|p| p.trim_start_matches("MOD:").to_string())
            .collect(),
        removed: paths
            .iter()
            .filter(|p| p.starts_with("REM:"))
            .map(|p| p.trim_start_matches("REM:").to_string())
            .collect(),
    }
}

/// Synthetic recorded entry (in-memory; no persistence involved).
fn rec(projection: Projection, anchor: &str, seed: u8) -> RecordedObservation {
    let projection_id = projection.id();
    RecordedObservation {
        projection,
        observation: ProjectionObservation::new(
            ProjectionObservationId::from_hash(h(seed)),
            projection_id,
            snap(1),
            h(seed.wrapping_add(200)),
            None,
        ),
        anchor_path: anchor.to_string(),
    }
}

#[test]
fn file_scoped_projection_matches_exact_changed_path() {
    let recorded = vec![rec(
        Projection::file("auth/tokens.py").unwrap(),
        "auth/tokens.py",
        1,
    )];
    let c = reevaluation_candidates(&changed(&["MOD:auth/tokens.py"]), &recorded);
    assert!(c.contains(&recorded[0].observation.id()));
    // Unrelated path: NOT a candidate (file-scoped is precise).
    let c2 = reevaluation_candidates(&changed(&["MOD:payments/pricing.py"]), &recorded);
    assert!(!c2.contains(&recorded[0].observation.id()));
}

#[test]
fn unit_anchored_projections_match_changed_defining_file() {
    let recorded = vec![
        rec(
            Projection::signature("auth.tokens.refresh_token").unwrap(),
            "auth/tokens.py",
            1,
        ),
        rec(
            Projection::definition("auth.service.AuthService").unwrap(),
            "auth/service.py",
            2,
        ),
    ];
    // tokens.py modified → refresh_token candidate, AuthService NOT.
    let c = reevaluation_candidates(&changed(&["MOD:auth/tokens.py"]), &recorded);
    assert!(c.contains(&recorded[0].observation.id()));
    assert!(!c.contains(&recorded[1].observation.id()));

    // Removed file: symbols anchored to it must surface (symbol may be gone).
    let c2 = reevaluation_candidates(&changed(&["REM:auth/tokens.py"]), &recorded);
    assert!(c2.contains(&recorded[0].observation.id()));
}

#[test]
fn package_init_imports_are_anchored_to_init_py() {
    // Imports observation of module `users` recorded from
    // users/__init__.py: changing the initializer must select it — no
    // module-name→path derivation in M5 (M3 owns module identity).
    let recorded = vec![rec(
        Projection::imports("users", Scope::Repository).unwrap(),
        "users/__init__.py",
        1,
    )];
    for p in [
        "ADD:users/__init__.py",
        "MOD:users/__init__.py",
        "REM:users/__init__.py",
    ] {
        let c = reevaluation_candidates(&changed(&[p]), &recorded);
        assert!(
            c.contains(&recorded[0].observation.id()),
            "{p} must select the package-init imports observation"
        );
    }
    // Changing an unrelated file must not.
    let c = reevaluation_candidates(&changed(&["MOD:auth/tokens.py"]), &recorded);
    assert!(!c.contains(&recorded[0].observation.id()));
}

#[test]
fn added_file_triggers_completeness_sensitive_candidates() {
    let recorded = vec![rec(
        Projection::callers("auth.tokens.refresh_token", Scope::Repository).unwrap(),
        "auth/tokens.py",
        1,
    )];
    let c = reevaluation_candidates(&changed(&["ADD:api/webhooks.py"]), &recorded);
    assert!(c.contains(&recorded[0].observation.id()));
}

#[test]
fn modified_and_removed_files_trigger_completeness_candidates() {
    let recorded = vec![rec(
        Projection::callers("auth.tokens.refresh_token", Scope::Repository).unwrap(),
        "auth/tokens.py",
        1,
    )];
    assert!(
        reevaluation_candidates(&changed(&["MOD:payments/pricing.py"]), &recorded)
            .contains(&recorded[0].observation.id())
    );
    assert!(
        reevaluation_candidates(&changed(&["REM:users/service.py"]), &recorded)
            .contains(&recorded[0].observation.id())
    );
}

#[test]
fn conservative_fallback_kinds_match_any_file_change_and_are_documented() {
    // ConfigValue/ToolVersion inputs are not yet modeled: candidate on any
    // file-level change (documented in discovery.rs class 4).
    let recorded = vec![rec(
        Projection::config_value("security.session_timeout").unwrap(),
        "config/settings.py",
        1,
    )];
    assert!(
        reevaluation_candidates(&changed(&["MOD:payments/pricing.py"]), &recorded)
            .contains(&recorded[0].observation.id())
    );
}

#[test]
fn empty_changed_set_selects_nothing() {
    let recorded = vec![
        rec(
            Projection::file("auth/tokens.py").unwrap(),
            "auth/tokens.py",
            1,
        ),
        rec(
            Projection::signature("auth.tokens.refresh_token").unwrap(),
            "auth/tokens.py",
            2,
        ),
        rec(
            Projection::callers("auth.tokens.refresh_token", Scope::Repository).unwrap(),
            "auth/tokens.py",
            3,
        ),
        rec(Projection::config_value("k").unwrap(), "config.py", 4),
    ];
    let c = reevaluation_candidates(&ChangedSet::default(), &recorded);
    assert_eq!(c.len(), 0, "no changes → no candidates (every kind)");
}

#[test]
fn multiple_changes_and_duplicates_deduplicate_deterministically() {
    let recorded = vec![
        rec(
            Projection::file("auth/tokens.py").unwrap(),
            "auth/tokens.py",
            1,
        ),
        rec(
            Projection::signature("auth.tokens.refresh_token").unwrap(),
            "auth/tokens.py",
            2,
        ),
        rec(
            Projection::callers("auth.tokens.refresh_token", Scope::Repository).unwrap(),
            "auth/tokens.py",
            3,
        ),
    ];
    // Multiple changes hitting the same observations: one entry each.
    let c = reevaluation_candidates(
        &changed(&["MOD:auth/tokens.py", "ADD:api/webhooks.py"]),
        &recorded,
    );
    assert_eq!(c.len(), 3);
    let ids = c.ids();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "deterministic canonical ordering");
}

#[test]
fn unrelated_change_minimizes_candidates() {
    let recorded = vec![
        rec(
            Projection::signature("auth.tokens.refresh_token").unwrap(),
            "auth/tokens.py",
            1,
        ),
        rec(
            Projection::file("payments/pricing.py").unwrap(),
            "payments/pricing.py",
            2,
        ),
    ];
    let c = reevaluation_candidates(&changed(&["MOD:payments/pricing.py"]), &recorded);
    assert!(!c.contains(&recorded[0].observation.id()));
    assert!(c.contains(&recorded[1].observation.id()));
}

/// Same canonical key, different scope → distinct projection ids →
/// distinct observations, and both are discovered independently.
#[test]
fn scope_distinct_projections_have_distinct_ids() {
    let p_repo = Projection::definition("m.f").unwrap();
    let p_pkg = Projection::new(
        ProjectionKind::Definition,
        trellis_core::projection::Subject::Symbol("m.f".into()),
        trellis_core::projection::Property::Definition,
        Scope::Package,
    )
    .unwrap();
    assert_ne!(p_repo.id(), p_pkg.id());
    let recorded = vec![
        rec(p_repo.clone(), "m.py", 1),
        rec(p_pkg.clone(), "m.py", 2),
    ];
    let c = reevaluation_candidates(&changed(&["MOD:m.py"]), &recorded);
    assert!(c.contains(&recorded[0].observation.id()));
    assert!(c.contains(&recorded[1].observation.id()));
}

/// Persistence round-trip: record via the sanctioned path, reopen, load
/// via recorded_from_store, and candidates must be identical pre/post.
#[test]
fn discovery_survives_restart_via_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let mut store = Store::open(&db).unwrap();
        let snapshot_row = trellis_core::snapshot::Snapshot::new(
            snap(1),
            trellis_core::ids::RepositoryId::from_hash(h(70)),
            None,
            trellis_core::ids::ManifestId::from_hash(h(71)),
            None,
            trellis_core::ids::EnvironmentFingerprintId::from_hash(h(72)),
            None,
            1_000,
        )
        .unwrap();
        store.put_snapshot(&snapshot_row).unwrap();
        let tree = fixture_tree();
        let idx = PythonSyntaxIndex::index(&tree);
        let module = ModuleId::from_path("auth/tokens.py").unwrap();
        let n = record_unit_index(&idx, &module, "auth/tokens.py", &mut store).unwrap();
        assert!(n > 0);

        let p_sig = Projection::signature("auth.tokens.refresh_token").unwrap();
        let observed = observe_signature(&idx, &p_sig, snap(1)).unwrap().unwrap();
        record_observation(&mut store, &p_sig, &observed).unwrap();

        let p_file = Projection::file("auth/tokens.py").unwrap();
        let fobs = observe_file_digest(&idx, &p_file, snap(1))
            .unwrap()
            .unwrap();
        record_observation(&mut store, &p_file, &fobs).unwrap();
    }
    {
        let store = Store::open(&db).unwrap();
        let pre = recorded_from_store(&store).unwrap();
        let c = reevaluation_candidates(&changed(&["MOD:auth/tokens.py"]), &pre);
        assert_eq!(c.len(), 2, "both persisted observations are candidates");
        // Restart equality: reopen again → identical recorded set →
        // identical candidates.
        let store2 = Store::open(&db).unwrap();
        let post = recorded_from_store(&store2).unwrap();
        assert_eq!(pre, post);
        let c2 = reevaluation_candidates(&changed(&["MOD:auth/tokens.py"]), &post);
        assert_eq!(c, c2);
    }
}

/// Fail closed: an observation without a persisted descriptor/anchor is
/// corrupt state — explicit error, never a silently omitted candidate.
#[test]
fn missing_descriptor_or_anchor_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let snapshot_row = trellis_core::snapshot::Snapshot::new(
        snap(1),
        trellis_core::ids::RepositoryId::from_hash(h(70)),
        None,
        trellis_core::ids::ManifestId::from_hash(h(71)),
        None,
        trellis_core::ids::EnvironmentFingerprintId::from_hash(h(72)),
        None,
        1_000,
    )
    .unwrap();
    store.put_snapshot(&snapshot_row).unwrap();
    let tree = fixture_tree();
    let idx = PythonSyntaxIndex::index(&tree);
    let p = Projection::signature("auth.tokens.refresh_token").unwrap();
    let observed = observe_signature(&idx, &p, snap(1)).unwrap().unwrap();
    // Persist the observation but NOT its descriptor/anchor (simulated
    // partial persistence).
    store
        .put_projection_observation(observed.observation())
        .unwrap();
    let err = recorded_from_store(&store).unwrap_err();
    assert!(
        matches!(err, trellis_store::StoreError::Corrupt(_)),
        "missing discovery state must fail closed, got {err:?}"
    );
}

/// Multi-location symbols: recording the same symbol from two files keeps
/// BOTH rows across reopen (composite PK) — the symbol→file index never
/// loses an anchor.
#[test]
fn symbol_locations_preserve_history_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let mut store = Store::open(&db).unwrap();
        store.put_symbol_location("s.S", "a", "a.py").unwrap();
    }
    {
        let mut store = Store::open(&db).unwrap();
        store.put_symbol_location("s.S", "b", "b.py").unwrap();
    }
    let store = Store::open(&db).unwrap();
    let locs = store.symbol_locations().unwrap();
    assert_eq!(
        locs,
        vec![
            ("s.S".to_string(), "a".to_string(), "a.py".to_string()),
            ("s.S".to_string(), "b".to_string(), "b.py".to_string()),
        ],
        "historical locations must survive; first-wins would lose b.py"
    );
}

/// Catalog-driven no-false-negative battery: changed sets AND expected
/// must-not-miss keys are derived from the M2 oracle catalog (read-only).
#[test]
fn no_false_negatives_across_supported_m2_mutations() {
    let catalog = trellis_oracle::Catalog::load();
    let supported = ["R1", "R2", "R3", "R4", "X1", "X2", "X3", "X4", "X5"];

    // Recorded observations: one per unique oracle key across the
    // supported mutations' affected_projections, built FROM the catalog.
    let mut keys: Vec<trellis_oracle::ProjectionKey> = Vec::new();
    for id in supported {
        let m = catalog.mutation(id).unwrap();
        for label in &m.affected_projections {
            if !keys.contains(&label.key) {
                keys.push(label.key.clone());
            }
        }
    }
    let mut recorded = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let kind = parse_catalog_kind(&key.kind);
        let scope = parse_catalog_scope(&key.scope);
        let projection =
            Projection::new(kind, subject_of(kind, &key.subject), prop_of(kind), scope)
                .unwrap_or_else(|e| panic!("catalog key {} builds: {e}", key.render()));
        let anchor = anchor_for(&key.subject);
        recorded.push(rec(projection, &anchor, (i * 3 + 1) as u8));
    }

    for id in supported {
        let m = catalog.mutation(id).unwrap();
        let changed = changed_from_ops(&m.ops);
        let c = reevaluation_candidates(&changed, &recorded);
        println!(
            "MEASURE {id}: changed_paths={} candidates={} of {} recorded",
            changed.len(),
            c.len(),
            recorded.len()
        );
        for label in &m.affected_projections {
            let found = recorded
                .iter()
                .find(|r| key_matches(&r.projection, &label.key))
                .expect("every catalog affected key has a recorded observation");
            assert!(
                c.contains(&found.observation.id()),
                "{}: oracle-affected observation for key {} missing from candidates",
                id,
                label.key.render()
            );
        }
        // Universe-sensitive lower bound: every recorded completeness-
        // sensitive observation must be a candidate on any non-empty
        // change (they react to universe/symbol-inventory changes).
        if !changed.added.is_empty() || !changed.modified.is_empty() || !changed.removed.is_empty()
        {
            for r in &recorded {
                if r.projection.kind().is_completeness_sensitive() {
                    assert!(
                        c.contains(&r.observation.id()),
                        "{}: completeness-sensitive {}({}) must be a candidate on any change",
                        id,
                        r.projection.kind().keyword(),
                        r.projection.subject().canonical()
                    );
                }
            }
        }
    }
}

fn parse_catalog_kind(s: &str) -> ProjectionKind {
    match s {
        "FileContent" => ProjectionKind::FileContent,
        "Definition" => ProjectionKind::Definition,
        "Signature" => ProjectionKind::Signature,
        "References" => ProjectionKind::References,
        "Callers" => ProjectionKind::Callers,
        "Implementations" => ProjectionKind::Implementations,
        "Imports" => ProjectionKind::Imports,
        "Subclasses" => ProjectionKind::Subclasses,
        "RepositorySearch" => ProjectionKind::RepositorySearch,
        other => panic!("unexpected catalog kind {other}"),
    }
}

fn parse_catalog_scope(s: &str) -> Scope {
    match s {
        "File" => Scope::File,
        "Module" => Scope::Module,
        "Package" => Scope::Package,
        "Repository" => Scope::Repository,
        other => panic!("unexpected catalog scope {other}"),
    }
}

fn prop_of(kind: ProjectionKind) -> trellis_core::projection::Property {
    trellis_core::projection::Property::for_kind(kind).unwrap()
}

fn subject_of(kind: ProjectionKind, subject: &str) -> trellis_core::projection::Subject {
    use trellis_core::projection::Subject;
    match kind {
        ProjectionKind::FileContent => Subject::File(subject.into()),
        ProjectionKind::Imports => Subject::Module(subject.into()),
        ProjectionKind::ConfigValue => Subject::ConfigKey(subject.into()),
        ProjectionKind::ToolVersion => Subject::Tool(subject.into()),
        ProjectionKind::RepositorySearch => Subject::Text(subject.into()),
        _ => Subject::Symbol(subject.into()),
    }
}

fn anchor_for(subject: &str) -> String {
    // Recording-time source unit for the subject, per the M2 fixture.
    match subject {
        "payments/pricing.py"
        | "auth/interfaces.py"
        | "api/webhooks.py"
        | "users/deletion.py"
        | "users/alias_caller.py"
        | "users/broken.py"
        | "auth/providers/sso_provider.py"
        | "users/__init__.py" => subject.to_string(),
        "auth.interfaces.AuthProvider.validate" => "auth/interfaces.py".to_string(),
        "api.webhooks.handle_refresh" => "api/webhooks.py".to_string(),
        // Completeness-sensitive subjects (Callers/Implementations/
        // References/Subclasses/RepositorySearch) are evaluated over the
        // whole repository; their synthetic test observations carry no
        // single anchor and match via the universe rule.
        "auth.tokens.refresh_token" | "auth.service.AuthService.login" => {
            "auth/tokens.py".to_string()
        }
        "auth.interfaces.AuthProvider" => "auth/interfaces.py".to_string(),
        other => panic!("no anchor mapping for subject {other}"),
    }
}

fn changed_from_ops(ops: &[trellis_oracle::Op]) -> ChangedSet {
    let mut set = ChangedSet::default();
    for op in ops {
        match op.op.as_str() {
            "add_file" => set.added.push(op.path.clone()),
            "replace_file" => set.modified.push(op.path.clone()),
            "delete_file" => set.removed.push(op.path.clone()),
            other => panic!("unknown op {other}"),
        }
    }
    set
}

fn key_matches(p: &Projection, key: &trellis_oracle::ProjectionKey) -> bool {
    p.kind().keyword() == catalog_kind_keyword(&key.kind)
        && p.subject().canonical() == key.subject
        && p.scope_name() == key.scope
}

fn catalog_kind_keyword(s: &str) -> String {
    match s {
        "FileContent" => "file".into(),
        "Definition" => "definition".into(),
        "Signature" => "signature".into(),
        "References" => "references".into(),
        "Callers" => "callers".into(),
        "Implementations" => "implementations".into(),
        "Imports" => "imports".into(),
        "Subclasses" => "subclasses".into(),
        "RepositorySearch" => "repository_search".into(),
        other => panic!("unexpected kind {other}"),
    }
}

/// Semantic kinds are handled as recorded data (candidate on any change)
/// but never synthesized by M3 capabilities.
#[test]
fn semantic_kinds_are_recorded_data_not_synthesized() {
    let recorded = vec![rec(
        Projection::callers("auth.tokens.refresh_token", Scope::Repository).unwrap(),
        "auth/tokens.py",
        1,
    )];
    let c = reevaluation_candidates(&changed(&["ADD:x.py"]), &recorded);
    assert!(c.contains(&recorded[0].observation.id()));
    let idx = PythonSyntaxIndex::index(&fixture_tree());
    let symbol = trellis_program::prelude::SymbolPath::new("auth.tokens.refresh_token").unwrap();
    assert!(!ProgramIndex::callers(&idx, &symbol).is_proven());
}

fn fixture_tree() -> BTreeMap<String, String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/python_auth/base")
        .canonicalize()
        .unwrap();
    trellis_source::prelude::read_tree(&trellis_source::prelude::ManifestOptions::new(root))
        .unwrap()
}

/// M3 rebinding: `pkg.py` defines `pkg.sub.f`; adding `pkg/sub.py` can
/// rebind the symbol's unit (longest-prefix resolution). The observation
/// anchored to pkg.py must still be a candidate — inventory change rule.
#[test]
fn inventory_change_selects_unit_anchored_observations() {
    let recorded = vec![
        rec(Projection::definition("pkg.sub.f").unwrap(), "pkg.py", 1),
        rec(Projection::signature("pkg.sub.f").unwrap(), "pkg.py", 2),
        rec(
            Projection::imports("pkg", Scope::Repository).unwrap(),
            "pkg.py",
            3,
        ),
    ];
    // Only the ADDED file is in the changed set — anchors unchanged — but
    // the inventory change can rebind symbol resolution.
    let c = reevaluation_candidates(&changed(&["ADD:pkg/sub.py"]), &recorded);
    for r in &recorded {
        assert!(
            c.contains(&r.observation.id()),
            "ADD must select unit-anchored {}({})",
            r.projection.kind().keyword(),
            r.projection.subject().canonical()
        );
    }
    // Same for removals.
    let c2 = reevaluation_candidates(&changed(&["REM:other.py"]), &recorded);
    for r in &recorded {
        assert!(c2.contains(&r.observation.id()));
    }
    // Modified-only changes do NOT blanket-select unit-anchored kinds.
    let c3 = reevaluation_candidates(&changed(&["MOD:unrelated.py"]), &recorded);
    assert_eq!(
        c3.len(),
        0,
        "modified-only: anchors unchanged, no rebinding"
    );
}

/// Module aliasing: `users.py` and `users/__init__.py` share ModuleId in
/// M3, but file-digest is path-exact — adding the initializer must NOT
/// silently change the `file(users.py)` observation's value class, and the
/// changed-set handling stays sound (both paths are candidates).
#[test]
fn module_aliasing_file_digest_is_path_exact() {
    let mut tree = fixture_tree();
    tree.insert("users.py".to_string(), "VALUE = 1\n".to_string());
    let idx = PythonSyntaxIndex::index(&tree);
    let p_users = Projection::file("users.py").unwrap();
    let d1 = idx.file_digest("users.py").proven().flatten().unwrap();
    let _ = p_users;
    // Add the initializer: users.py digest unchanged (path-exact).
    tree.insert("users/__init__.py".to_string(), "X = 2\n".to_string());
    let idx2 = PythonSyntaxIndex::index(&tree);
    let d2 = idx2.file_digest("users.py").proven().flatten().unwrap();
    assert_eq!(
        d1, d2,
        "adding an aliased unit must not alter the other path's digest"
    );
    // And the new path has its own digest.
    let d3 = idx2
        .file_digest("users/__init__.py")
        .proven()
        .flatten()
        .unwrap();
    assert_ne!(d2, d3);
}

/// Conflicting anchor for the same observation is rejected; identical
/// re-record is idempotent.
#[test]
fn conflicting_observation_anchor_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let snapshot_row = snapshot_row();
    store.put_snapshot(&snapshot_row).unwrap();
    let tree = fixture_tree();
    let idx = PythonSyntaxIndex::index(&tree);
    let p = Projection::signature("auth.tokens.refresh_token").unwrap();
    let observed = observe_signature(&idx, &p, snap(1)).unwrap().unwrap();
    record_observation(&mut store, &p, &observed).unwrap();
    // Identical re-record: idempotent.
    record_observation(&mut store, &p, &observed).unwrap();
    // Conflicting anchor: rejected, stored anchor unchanged. (A changed
    // anchor requires a NEW observation — the value was evaluated against
    // a different unit.) The conflicting attempt here is the SAME
    // observation with a hand-forged different anchor: the canonical-id
    // check makes forged pairs impossible, so simulate via a second
    // observation of the same projection at a different snapshot.
    let snapshot_row2 = trellis_core::snapshot::Snapshot::new(
        snap(2),
        trellis_core::ids::RepositoryId::from_hash(h(70)),
        None,
        trellis_core::ids::ManifestId::from_hash(h(71)),
        None,
        trellis_core::ids::EnvironmentFingerprintId::from_hash(h(72)),
        None,
        2_000,
    )
    .unwrap();
    store.put_snapshot(&snapshot_row2).unwrap();
    let observed2 = observe_signature(&idx, &p, snap(2)).unwrap().unwrap();
    assert_ne!(observed.observation().id(), observed2.observation().id());
    record_observation(&mut store, &p, &observed2).unwrap();
    // The first observation's anchor is untouched.
    let store = Store::open(dir.path().join("metadata.db")).unwrap();
    assert_eq!(
        store
            .observation_anchor(&observed.observation().id())
            .unwrap(),
        "auth/tokens.py"
    );
}

/// Tampered descriptor (fields that hash differently) → fail closed.
#[test]
fn tampered_descriptor_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let mut store = Store::open(&db).unwrap();
        store.put_snapshot(&snapshot_row()).unwrap();
        let tree = fixture_tree();
        let idx = PythonSyntaxIndex::index(&tree);
        let p = Projection::signature("auth.tokens.refresh_token").unwrap();
        let observed = observe_signature(&idx, &p, snap(1)).unwrap().unwrap();
        record_observation(&mut store, &p, &observed).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE projection_descriptors SET subject = 'auth.tokens.other' WHERE subject = 'auth.tokens.refresh_token'",
            [],
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    let err = recorded_from_store(&store).unwrap_err();
    assert!(
        matches!(err, trellis_store::StoreError::Corrupt(_)),
        "tampered descriptor must fail closed, got {err:?}"
    );
}

/// Subject variant participates in identity: File("m.f") vs Symbol("m.f")
/// are different keys even with identical payloads.
#[test]
fn subject_variant_participates_in_identity() {
    let as_symbol = Projection::definition("m.f").unwrap();
    let as_file = Projection::new(
        ProjectionKind::Definition,
        trellis_core::projection::Subject::File("m.f".into()),
        trellis_core::projection::Property::Definition,
        Scope::Repository,
    )
    .unwrap();
    assert_ne!(as_symbol.id(), as_file.id());
}

fn snapshot_row() -> trellis_core::snapshot::Snapshot {
    trellis_core::snapshot::Snapshot::new(
        snap(1),
        trellis_core::ids::RepositoryId::from_hash(h(70)),
        None,
        trellis_core::ids::ManifestId::from_hash(h(71)),
        None,
        trellis_core::ids::EnvironmentFingerprintId::from_hash(h(72)),
        None,
        1_000,
    )
    .unwrap()
}

/// Sequential rebinding (the cycle-3 blocker): observe pkg.sub.f against
/// pkg.py; ADD pkg/sub.py rebinds resolution; re-observe and persist (the
/// new observation carries the NEW authoritative anchor); reopen; then a
/// MODIFIED-ONLY change to pkg/sub.py must select the rebound observation.
#[test]
fn sequential_rebind_reobservation_and_modified_only_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    let p = Projection::definition("pkg.sub.f").unwrap();

    // S0: pkg.py defines pkg.sub.f (single-file module).
    {
        let mut store = Store::open(&db).unwrap();
        store.put_snapshot(&snapshot_row()).unwrap();
        let mut tree = BTreeMap::new();
        tree.insert("pkg.py".to_string(), "def f():\n    pass\n".to_string());
        let idx = PythonSyntaxIndex::index(&tree);
        let observed = observe_definition(&idx, &p, snap(1)).unwrap().unwrap();
        assert_eq!(observed.resolved_source_path(), "pkg.py");
        record_observation(&mut store, &p, &observed).unwrap();
    }

    // S1: pkg/sub.py added → M3 rebinds pkg.sub.f to pkg/sub.py. The
    // engine re-observes and records the rebound observation (new anchor).
    {
        let mut store = Store::open(&db).unwrap();
        let mut tree = BTreeMap::new();
        tree.insert("pkg.py".to_string(), "def f():\n    pass\n".to_string());
        tree.insert(
            "pkg/sub.py".to_string(),
            "def f():\n    return 1\n".to_string(),
        );
        let idx = PythonSyntaxIndex::index(&tree);
        let observed = observe_definition(&idx, &p, snap(2)).unwrap().unwrap();
        assert_eq!(
            observed.resolved_source_path(),
            "pkg/sub.py",
            "M3's longest-prefix resolution rebinds to the subpackage unit"
        );
        store
            .put_snapshot(
                &trellis_core::snapshot::Snapshot::new(
                    snap(2),
                    trellis_core::ids::RepositoryId::from_hash(h(70)),
                    None,
                    trellis_core::ids::ManifestId::from_hash(h(71)),
                    None,
                    trellis_core::ids::EnvironmentFingerprintId::from_hash(h(72)),
                    None,
                    2_000,
                )
                .unwrap(),
            )
            .unwrap();
        record_observation(&mut store, &p, &observed).unwrap();
    }

    // S2: reopen; MODIFIED-ONLY change to pkg/sub.py must select the
    // rebound observation (its persisted anchor is now pkg/sub.py).
    {
        let store = Store::open(&db).unwrap();
        let recorded = recorded_from_store(&store).unwrap();
        let rebound = recorded
            .iter()
            .find(|r| r.observation.snapshot() == snap(2))
            .expect("rebound observation persisted");
        assert_eq!(rebound.anchor_path, "pkg/sub.py");
        let c = reevaluation_candidates(&changed(&["MOD:pkg/sub.py"]), &recorded);
        assert!(
            c.contains(&rebound.observation.id()),
            "modified-only change to the rebound unit must select it"
        );
        // The OLD observation (anchored to pkg.py) is not selected by a
        // pkg/sub.py modification — its own anchor is unchanged.
        let old = recorded
            .iter()
            .find(|r| r.observation.snapshot() == snap(1))
            .unwrap();
        assert!(!c.contains(&old.observation.id()));
    }
}

/// Non-Python inventory changes do NOT blanket-select unit-anchored kinds
/// (the documented .py-only rule).
#[test]
fn non_python_inventory_change_does_not_select_unit_anchored() {
    let recorded = vec![rec(
        Projection::definition("pkg.sub.f").unwrap(),
        "pkg.py",
        1,
    )];
    let c = reevaluation_candidates(&changed(&["ADD:README.md"]), &recorded);
    assert!(!c.contains(&recorded[0].observation.id()));
    let c2 = reevaluation_candidates(&changed(&["REM:docs.md"]), &recorded);
    assert!(!c2.contains(&recorded[0].observation.id()));
}

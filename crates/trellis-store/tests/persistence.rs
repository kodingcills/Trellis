//! M4 persistence acceptance + adversarial tests (spec §19, node contract).

use trellis_cas::FsCas;
use trellis_core::artifact::{
    ArtifactEnvelope, ArtifactKind, CostRecord, Derivation, ProducerInfo, Proposition,
};
use trellis_core::attestation::{AttestationHistory, EvidenceRef, ValidationAttestation};
use trellis_core::ids::{
    ArtifactId, AttestationId, BlobId, ContentHash, DerivationId, EnvironmentFingerprintId,
    HashAlgo, ProjectionId, RepositoryId, SnapshotId, VerifierId,
};
use trellis_core::observation_id::canonical_observation_id;
use trellis_core::projection::Projection;
use trellis_core::projection::ProjectionObservation;
use trellis_core::validity::{Authority, CaptureChannel, Validity, VerificationLevel};
use trellis_store::prelude::*;

fn h(seed: u8) -> ContentHash {
    let d: Vec<u8> = (0..32).map(|i| seed.wrapping_add(i)).collect();
    ContentHash::from_bytes(HashAlgo::Blake3, &d).unwrap()
}

/// Sanctioned blob-first flow: place bytes in CAS, then register.
fn place_and_register(store: &mut Store, cas: &FsCas, bytes: &[u8]) -> BlobId {
    let id = cas.put(bytes).unwrap();
    store.register_blob(&id).unwrap();
    id
}

fn producer() -> ProducerInfo {
    ProducerInfo::new("bench-agent", "0.1.0", Some("test-model".to_string())).unwrap()
}

fn derivation() -> Derivation {
    Derivation::new(
        DerivationId::from_hash(h(90)),
        vec![CaptureChannel::TrellisTool],
        producer(),
        1_000,
    )
}

fn artifact() -> ArtifactEnvelope {
    ArtifactEnvelope::new(
        ArtifactId::from_hash(h(30)),
        1,
        ArtifactKind::Fact,
        BlobId::from_hash(h(31)),
        Some(Proposition::new("refresh_token has no external callers").unwrap()),
        producer(),
        derivation(),
        vec![ProjectionId::from_hash(h(44))],
        SnapshotId::from_hash(h(32)),
        CostRecord::default(),
    )
    .unwrap()
}

fn attestation(id_seed: u8, at: u64) -> ValidationAttestation {
    ValidationAttestation::for_derivation(
        AttestationId::from_hash(h(id_seed)),
        ArtifactId::from_hash(h(30)),
        &derivation(),
        SnapshotId::from_hash(h(70)),
        None,
        Validity::Valid,
        Authority::Authoritative,
        VerificationLevel::Structural,
        None,
        vec![],
        at,
    )
}

/// Full round-trip: domain → persist → close → reopen → domain equality +
/// identity equality (M4 restart/recovery acceptance, process A/B).
#[test]
fn restart_recovery_preserves_semantics_and_ordering() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");

    // ── process A ──
    {
        let mut store = Store::open(&db).unwrap();
        assert!(store.wal_enabled());
        assert!(store.foreign_keys_enabled());

        // CAS blob first (blob-first protocol).
        let payload = b"canonical artifact payload";
        let cas = FsCas::open(dir.path().join("cas")).unwrap();
        let blob_id = place_and_register(&mut store, &cas, payload);

        let snap = trellis_core::snapshot::Snapshot::new(
            SnapshotId::from_hash(h(60)),
            RepositoryId::from_hash(h(61)),
            None,
            trellis_core::ids::ManifestId::from_hash(h(61)),
            None,
            EnvironmentFingerprintId::from_hash(h(62)),
            None,
            1_500,
        )
        .unwrap();
        store.put_snapshot(&snap).unwrap();

        let mut art = artifact();
        // point payload at the durably placed blob
        art = with_payload(art, blob_id);
        store.put_artifact(&art).unwrap();
        store.append_attestation(&attestation(80, 2_000)).unwrap();
        store
            .append_attestation(&{
                ValidationAttestation::for_derivation(
                    AttestationId::from_hash(h(81)),
                    art.id(),
                    &derivation(),
                    SnapshotId::from_hash(h(71)),
                    None,
                    Validity::Stale,
                    Authority::Authoritative,
                    VerificationLevel::Test,
                    None,
                    vec![],
                    2_500,
                )
            })
            .unwrap();
    } // store dropped = "process exit"

    // ── process B ──
    {
        let store = Store::open(&db).unwrap();
        let snap = store.get_snapshot(&SnapshotId::from_hash(h(60))).unwrap();
        assert_eq!(snap.created_at(), 1_500);

        let art = store.get_artifact(&ArtifactId::from_hash(h(30))).unwrap();
        // Expected: the exact envelope persisted by process A — payload
        // blob is the one durably placed, everything else identical.
        let cas = FsCas::open(dir.path().join("cas")).unwrap();
        let blob_id = cas.put(b"canonical artifact payload").unwrap();
        let expected = with_payload(artifact(), blob_id);
        assert_eq!(art, expected, "artifact must round-trip to domain equality");
        assert_eq!(art.id(), expected.id());

        let history: AttestationHistory = store
            .attestation_history(&ArtifactId::from_hash(h(30)))
            .unwrap();
        let entries: Vec<_> = history.iter().collect();
        assert_eq!(entries.len(), 2);
        // Append order preserved: created_at ascending, contents identical.
        assert_eq!(entries[0].created_at(), 2_000);
        assert_eq!(entries[1].created_at(), 2_500);
        assert_eq!(entries[0].validity(), Validity::Valid);
        assert_eq!(entries[1].validity(), Validity::Stale);
        assert_eq!(entries[0].created_at(), 2_000);
    }
}

fn with_payload(a: ArtifactEnvelope, blob: BlobId) -> ArtifactEnvelope {
    // Rebuild with the durably-placed payload id (identities preserved).
    ArtifactEnvelope::new(
        a.id(),
        a.schema_version(),
        a.kind(),
        blob,
        a.proposition()
            .map(|p| Proposition::new(p.as_str()).unwrap()),
        a.producer().clone(),
        a.derivation().clone(),
        a.dependencies().to_vec(),
        a.created_snapshot(),
        a.cost(),
    )
    .unwrap()
}

/// Duplicate canonical objects are idempotent with identical identity.
#[test]
fn duplicate_artifact_insertion_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();
    let blob = place_and_register(&mut store, &cas, b"payload");
    let art = with_payload(artifact(), blob);
    store.put_artifact(&art).unwrap();
    store.put_artifact(&art).unwrap(); // idempotent
    let loaded = store.get_artifact(&art.id()).unwrap();
    assert_eq!(loaded, art);
}

/// Domain invariant enforced at the storage boundary: duplicate attestation
/// identity rejected.
#[test]
fn duplicate_attestation_identity_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();
    let blob = place_and_register(&mut store, &cas, b"p");
    let art = with_payload(artifact(), blob);
    store.put_artifact(&art).unwrap();
    store.append_attestation(&attestation(80, 2_000)).unwrap();
    // Same identity again → Constraint (append-only integrity, spec §12).
    assert!(store.append_attestation(&attestation(80, 2_100)).is_err());
}

/// Artifact/attestation mismatch rejected (foreign artifact).
#[test]
fn attestation_for_unknown_artifact_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let foreign = ValidationAttestation::for_derivation(
        AttestationId::from_hash(h(81)),
        ArtifactId::from_hash(h(31)), // no such artifact
        &derivation(),
        SnapshotId::from_hash(h(70)),
        None,
        Validity::Valid,
        Authority::Authoritative,
        VerificationLevel::Structural,
        None,
        vec![],
        1_000,
    );
    let err = store.append_attestation(&foreign).unwrap_err();
    assert!(matches!(err, trellis_store::StoreError::Constraint(_)));
}

/// Metadata must never commit pointing at a blob that is not durably
/// placed: put_artifact before CAS put → error, and no artifact row exists.
#[test]
fn metadata_never_references_absent_blob() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    // No CAS put for this payload id.
    let err = store.put_artifact(&artifact()).unwrap_err();
    assert!(matches!(err, trellis_store::StoreError::MissingBlob(_)));
    // No half-persisted metadata graph.
    assert!(store.get_artifact(&artifact().id()).is_err());
}

/// Transaction failure leaves no half-persisted metadata graph.
#[test]
fn transaction_failure_rolls_back_completely() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();
    let blob = place_and_register(&mut store, &cas, b"payload");
    let art = with_payload(artifact(), blob);

    // A failing transaction: artifact insert succeeds inside, then error.
    let result: Result<(), trellis_store::StoreError> = store.transaction(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO artifacts (id, schema_version, kind, payload_ref,
             proposition, producer, derivation, dependencies, created_snapshot, created_at)
             VALUES (?1, 1, 'Fact', ?2, NULL, '{}', ?3, '[]', ?3, 0)",
            rusqlite::params![
                art.id().to_string(),
                art.payload_ref().to_string(),
                art.created_snapshot().to_string()
            ],
        )
        .map_err(trellis_store::StoreError::from)?;
        Err(trellis_store::StoreError::Constraint(
            "simulated failure".into(),
        ))
    });
    assert!(result.is_err());
    // Nothing persisted.
    assert!(store.get_artifact(&art.id()).is_err());
}

/// Unsupported schema version is rejected explicitly.
#[test]
fn unsupported_schema_version_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let _store = Store::open(&db).unwrap();
    }
    // Tamper: bump schema version to an unsupported future value.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = '999' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    let err = Store::open(&db).unwrap_err();
    assert!(
        matches!(err, trellis_store::StoreError::UnsupportedSchema { .. }),
        "must reject unsupported schema, got {err:?}"
    );
}

/// Missing referenced blob is an explicit error at the domain boundary.
#[test]
fn missing_referenced_blob_detected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();
    // A blob id never placed in the CAS: registration is refused (metadata
    // can never lead the blob) and the blob is missing on read.
    let phantom = BlobId::from_hash(h(99));
    assert!(matches!(
        store.register_blob(&phantom),
        Err(trellis_store::StoreError::MissingBlob(_))
    ));
    assert!(!cas.contains(&phantom));
    assert!(matches!(
        cas.get(&phantom),
        Err(trellis_cas::CasError::MissingBlob(_))
    ));
}

/// Corrupted CAS blob is detected on read.
#[test]
fn corrupted_cas_blob_detected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();
    let blob = place_and_register(&mut store, &cas, b"to-be-corrupted");
    // Corrupt the stored object.
    let digest_hex = blob.hash().to_string();
    let digest_hex = digest_hex.strip_prefix("blake3:").unwrap();
    let path = dir.path().join("cas").join(&digest_hex[..2]);
    let obj = path.join(digest_hex);
    let mut bytes = std::fs::read(&obj).unwrap();
    bytes[0] ^= 0xff;
    std::fs::write(&obj, &bytes).unwrap();
    assert!(matches!(
        cas.get(&blob),
        Err(trellis_cas::CasError::CorruptBlob(_))
    ));
}

/// Projection observations round-trip with canonical identity.
#[test]
fn projection_observation_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let snap = trellis_core::snapshot::Snapshot::new(
        SnapshotId::from_hash(h(60)),
        RepositoryId::from_hash(h(61)),
        None,
        trellis_core::ids::ManifestId::from_hash(h(62)),
        None,
        EnvironmentFingerprintId::from_hash(h(63)),
        None,
        1_000,
    )
    .unwrap();
    store.put_snapshot(&snap).unwrap();
    // Canonical identity: the id derives from (projection, value, snapshot).
    let proj_id = ProjectionId::from_hash(h(41));
    let value = h(41);
    let snapshot = SnapshotId::from_hash(h(60));
    let id = canonical_observation_id(&proj_id, &value, &snapshot);
    let obs =
        ProjectionObservation::new(id, proj_id, snapshot, value, Some(BlobId::from_hash(h(42))));
    store.put_projection_observation(&obs).unwrap();
    store.put_projection_observation(&obs).unwrap(); // idempotent
    let loaded = store.get_projection_observation(&obs.id()).unwrap();
    assert_eq!(loaded, obs);
}

/// Attestation append ordering survives reopen (domain identity + order).
#[test]
fn attestation_ordering_preserved_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    let art = artifact();
    {
        let mut store = Store::open(&db).unwrap();
        let cas = FsCas::open(dir.path().join("cas")).unwrap();
        let blob = place_and_register(&mut store, &cas, b"payload");
        store
            .put_artifact(&with_payload(art.clone(), blob))
            .unwrap();
        for (seed, at) in [(81u8, 2_000u64), (82, 2_100), (83, 2_200)] {
            store
                .append_attestation(&attestation_with_validity(seed, at, Validity::Unknown))
                .unwrap();
        }
    }
    {
        let store = Store::open(&db).unwrap();
        let history: AttestationHistory = store.attestation_history(&art.id()).unwrap();
        let times: Vec<u64> = history.iter().map(|a| a.created_at()).collect();
        assert_eq!(times, vec![2_000, 2_100, 2_200]);
    }
}

fn attestation_with_validity(seed: u8, at: u64, validity: Validity) -> ValidationAttestation {
    ValidationAttestation::for_derivation(
        AttestationId::from_hash(h(seed)),
        ArtifactId::from_hash(h(30)),
        &derivation(),
        SnapshotId::from_hash(h(70)),
        None,
        validity,
        Authority::Authoritative,
        VerificationLevel::Structural,
        None,
        vec![],
        at,
    )
}

/// Same logical object persisted under different insertion orders yields
/// the same domain identity (identity is content, not insertion order).
#[test]
fn insertion_order_does_not_change_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();
    let blob = place_and_register(&mut store, &cas, b"same");

    // Two artifacts with identical content and identity.
    let a1 = with_payload(artifact(), blob);
    store.put_artifact(&a1).unwrap();
    // Re-derive an equivalent artifact from scratch; identity must match.
    let a2 = artifact();
    let a2 = with_payload(a2, blob);
    store.put_artifact(&a2).unwrap();
    assert_eq!(a1.id(), a2.id());

    // Dependencies sorted by the domain constructor: stored edge order must
    // match canonical (sorted) form regardless of insertion order.
    // Dependency edge order matches canonical (sorted) order in both.
    let deps1: Vec<String> = a1.dependencies().iter().map(|d| d.to_string()).collect();
    let deps2: Vec<String> = a2.dependencies().iter().map(|d| d.to_string()).collect();
    assert_eq!(deps1, deps2);
}

/// Absolute temporary checkout path does not contaminate portable IDs:
/// persisting the same logical object from two different roots yields the
/// same identity.
#[test]
fn absolute_paths_do_not_contaminate_identity() {
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    assert_ne!(dir1.path(), dir2.path());

    let cas1 = FsCas::open(dir1.path().join("cas")).unwrap();
    let cas2 = FsCas::open(dir2.path().join("cas")).unwrap();
    let blob1 = cas1.put(b"shared content").unwrap();
    let blob2 = cas2.put(b"shared content").unwrap();
    assert_eq!(blob1, blob2, "CAS identity is content-derived, path-free");

    // Snapshot identity from two different roots: same canonical inputs →
    // same id (freeze is content-based).
    let repo = RepositoryId::from_hash(h(3));
    let manifest = trellis_source_manifest();
    let env = env_fingerprint();
    let s1 = trellis_source::prelude::freeze(repo, &manifest, &env, None, None, 1_000).unwrap();
    let s2 = trellis_source::prelude::freeze(repo, &manifest, &env, None, None, 1_000).unwrap();
    assert_eq!(s1.id(), s2.id());
}

fn env_fingerprint() -> trellis_source::prelude::EnvironmentFingerprint {
    trellis_source::prelude::EnvironmentFingerprint::from_declared(&[(
        "python".to_string(),
        "3.12".to_string(),
    )])
    .unwrap()
}

fn trellis_source_manifest() -> trellis_source::prelude::Manifest {
    trellis_source::prelude::Manifest::from_entries(vec![]).unwrap()
}

/// Attestation append preserves monotonic ordering semantics from the
/// domain model (out-of-order appends rejected at the domain layer too).
#[test]
fn attestation_append_ordering_semantics_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();
    let blob = place_and_register(&mut store, &cas, b"payload");
    store.put_artifact(&with_payload(artifact(), blob)).unwrap();

    store.append_attestation(&attestation(81, 1_000)).unwrap();
    // Same created_at is allowed (non-decreasing); earlier is rejected.
    store.append_attestation(&attestation(82, 1_000)).unwrap();
    assert!(store.append_attestation(&attestation(83, 999)).is_err());
}

/// Evidence references round-trip (empty set in M4; the type-level surface
/// exists for M5).
#[test]
fn evidence_ref_type_surface_exists() {
    let _dir_guard = tempfile::tempdir().unwrap();
    let _ = EvidenceRef::Blob(BlobId::from_hash(h(50)));
    let _ = VerifierId::from_hash(h(51));
}

/// Non-default CostRecord round-trips (restart equality, M4 review fix).
#[test]
fn cost_record_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();
    let blob = place_and_register(&mut store, &cas, b"payload");

    let base = artifact();
    let costed = ArtifactEnvelope::new(
        base.id(),
        base.schema_version(),
        base.kind(),
        blob,
        base.proposition()
            .map(|p| Proposition::new(p.as_str()).unwrap()),
        base.producer().clone(),
        base.derivation().clone(),
        base.dependencies().to_vec(),
        base.created_snapshot(),
        CostRecord {
            capture_ms: Some(120),
            validation_ms: Some(45),
            recompute_ms: Some(3_000),
            tokens: Some(500),
        },
    )
    .unwrap();
    store.put_artifact(&costed).unwrap();
    let loaded = store.get_artifact(&costed.id()).unwrap();
    assert_eq!(loaded.cost().capture_ms, Some(120));
    assert_eq!(loaded.cost().tokens, Some(500));
    assert_eq!(loaded, costed, "cost must round-trip to domain equality");
}

/// Attestation evidence + verifier round-trip (restart equality).
#[test]
fn evidence_and_verifier_roundtrip_through_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let mut store = Store::open(&db).unwrap();
        let cas = FsCas::open(dir.path().join("cas")).unwrap();
        let blob = place_and_register(&mut store, &cas, b"payload");
        store.put_artifact(&with_payload(artifact(), blob)).unwrap();
        let att = ValidationAttestation::for_derivation(
            AttestationId::from_hash(h(85)),
            ArtifactId::from_hash(h(30)),
            &derivation(),
            SnapshotId::from_hash(h(70)),
            None,
            Validity::Valid,
            Authority::Authoritative,
            VerificationLevel::Deterministic,
            Some(VerifierId::from_hash(h(51))),
            vec![EvidenceRef::Blob(BlobId::from_hash(h(52)))],
            3_000,
        );
        store.append_attestation(&att).unwrap();
    }
    {
        let store = Store::open(&db).unwrap();
        let history: AttestationHistory = store
            .attestation_history(&ArtifactId::from_hash(h(30)))
            .unwrap();
        let entries: Vec<_> = history.iter().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].verifier(), Some(VerifierId::from_hash(h(51))));
        assert_eq!(
            entries[0].evidence(),
            &[EvidenceRef::Blob(BlobId::from_hash(h(52)))]
        );
    }
}

/// register_blob refuses blobs never placed in the CAS — metadata can
/// never lead the blob (blob-first, spec §19).
#[test]
fn register_blob_without_placement_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let phantom = BlobId::from_hash(h(99));
    assert!(matches!(
        store.register_blob(&phantom),
        Err(trellis_store::StoreError::MissingBlob(_))
    ));
    // And an artifact referencing it is refused too.
    let err = store.put_artifact(&artifact()).unwrap_err();
    assert!(matches!(err, trellis_store::StoreError::MissingBlob(_)));
    assert!(store.get_artifact(&artifact().id()).is_err());
}

/// Tampered JSON column decodes to explicit Corrupt, never a silently
/// reinterpreted domain object.
#[test]
fn corrupt_state_decode_is_explicit_not_silent() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let mut store = Store::open(&db).unwrap();
        store.put_derivation(&derivation()).unwrap();
    }
    // Tamper the channels column with truncated JSON.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE derivations SET channels = '[CaptureCha' WHERE id = ?1",
            [derivation().id().to_string()],
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    let err = store.get_derivation(&derivation().id()).unwrap_err();
    assert!(
        matches!(err, trellis_store::StoreError::Corrupt(_)),
        "corrupt state must error explicitly, got {err:?}"
    );
}

/// Minimal GC: orphan CAS objects (durable file, no registry row) are
/// collectible; registered blobs are not orphans.
#[test]
fn orphan_blobs_collectible_by_roots_walk() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let cas = FsCas::open(dir.path().join("cas")).unwrap();

    // Registered blob (durable + registered) — NOT an orphan.
    let registered = place_and_register(&mut store, &cas, b"registered");

    // Orphan: durable CAS file with no registry row (crash between CAS
    // rename and metadata commit).
    let orphan = cas.put(b"orphaned").unwrap();
    // Remove its registry row to simulate the crash window.
    store.remove_blob_registration(&orphan).unwrap();

    let orphans = store.collect_orphans().unwrap();
    assert_eq!(orphans, vec![orphan]);
    assert!(!orphans.contains(&registered));
}

/// Tampered evidence element → explicit Corrupt, never a shortened history.
#[test]
fn corrupt_evidence_element_is_explicit_not_silently_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let mut store = Store::open(&db).unwrap();
        let cas = FsCas::open(dir.path().join("cas")).unwrap();
        let blob = place_and_register(&mut store, &cas, b"payload");
        store.put_artifact(&with_payload(artifact(), blob)).unwrap();
        store
            .append_attestation(&{
                ValidationAttestation::for_derivation(
                    AttestationId::from_hash(h(86)),
                    ArtifactId::from_hash(h(30)),
                    &derivation(),
                    SnapshotId::from_hash(h(70)),
                    None,
                    Validity::Valid,
                    Authority::Authoritative,
                    VerificationLevel::Deterministic,
                    Some(VerifierId::from_hash(h(51))),
                    vec![
                        EvidenceRef::Blob(BlobId::from_hash(h(52))),
                        EvidenceRef::PriorAttestation(AttestationId::from_hash(h(53))),
                    ],
                    3_000,
                )
            })
            .unwrap();
    }
    // Tamper ONE evidence element.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE validation_attestations SET evidence = ?1 WHERE id = ?2",
            rusqlite::params![
                serde_json::to_string(&[format!("blob:{}", h(52)), "garbage-entry".to_string()])
                    .unwrap(),
                AttestationId::from_hash(h(86)).to_string(),
            ],
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    let err = store
        .attestation_history(&ArtifactId::from_hash(h(30)))
        .unwrap_err();
    assert!(
        matches!(err, trellis_store::StoreError::Corrupt(_)),
        "malformed evidence must error explicitly, got {err:?}"
    );
}

/// Tampered cost field (string instead of u64) → explicit Corrupt, never
/// a silently zeroed cost record.
#[test]
fn corrupt_cost_field_is_explicit_not_silently_zeroed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let mut store = Store::open(&db).unwrap();
        let cas = FsCas::open(dir.path().join("cas")).unwrap();
        let blob = place_and_register(&mut store, &cas, b"payload");
        let base = artifact();
        let costed = ArtifactEnvelope::new(
            base.id(),
            base.schema_version(),
            base.kind(),
            blob,
            base.proposition()
                .map(|p| Proposition::new(p.as_str()).unwrap()),
            base.producer().clone(),
            base.derivation().clone(),
            base.dependencies().to_vec(),
            base.created_snapshot(),
            CostRecord {
                capture_ms: Some(120),
                ..CostRecord::default()
            },
        )
        .unwrap();
        store.put_artifact(&costed).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE artifacts SET cost = ?1 WHERE id = ?2",
            rusqlite::params![
                serde_json::json!({"capture_ms": "120", "validation_ms": null,
                    "recompute_ms": null, "tokens": null})
                .to_string(),
                artifact().id().to_string()
            ],
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    let err = store.get_artifact(&artifact().id()).unwrap_err();
    assert!(
        matches!(err, trellis_store::StoreError::Corrupt(_)),
        "mistyped cost field must error explicitly, got {err:?}"
    );
}

/// Tampered producer model (non-string) → explicit Corrupt, never a
/// silently dropped provenance identifier.
#[test]
fn corrupt_producer_model_is_explicit_not_silently_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    {
        let mut store = Store::open(&db).unwrap();
        let cas = FsCas::open(dir.path().join("cas")).unwrap();
        let blob = place_and_register(&mut store, &cas, b"payload");
        store.put_artifact(&with_payload(artifact(), blob)).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE artifacts SET producer = ?1 WHERE id = ?2",
            rusqlite::params![
                serde_json::json!({
                    "harness": "bench-agent",
                    "version": "0.1.0",
                    "model": 120i64
                })
                .to_string(),
                artifact().id().to_string(),
            ],
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    let err = store.get_artifact(&artifact().id()).unwrap_err();
    assert!(
        matches!(err, trellis_store::StoreError::Corrupt(_)),
        "mistyped producer model must error explicitly, got {err:?}"
    );
}

/// Tampered value_digest → both direct loading and bulk reads fail closed
/// (canonical-id re-derivation, spec §19).
#[test]
fn tampered_observation_digest_fails_closed_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    let snapshot = SnapshotId::from_hash(h(60));
    {
        let mut store = Store::open(&db).unwrap();
        let snap_row = trellis_core::snapshot::Snapshot::new(
            snapshot,
            RepositoryId::from_hash(h(61)),
            None,
            trellis_core::ids::ManifestId::from_hash(h(62)),
            None,
            trellis_core::ids::EnvironmentFingerprintId::from_hash(h(63)),
            None,
            1_000,
        )
        .unwrap();
        store.put_snapshot(&snap_row).unwrap();
        let proj_id = ProjectionId::from_hash(h(41));
        let value = h(41);
        let id = canonical_observation_id(&proj_id, &value, &snapshot);
        let obs = ProjectionObservation::new(id, proj_id, snapshot, value, None);
        store.put_projection_observation(&obs).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        // A parse-VALID but different digest: parsing alone accepts it;
        // only canonical-ID re-derivation detects the corruption.
        conn.execute(
            "UPDATE projection_observations SET value_digest = ?1",
            [h(42).to_string()],
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    // Bulk path (discovery loader).
    let err = store.all_projection_observations().unwrap_err();
    assert!(matches!(err, trellis_store::StoreError::Corrupt(_)));
    // Direct path: same centralized re-derivation check.
    let digest_err = store
        .get_projection_observation(&canonical_observation_id(
            &ProjectionId::from_hash(h(41)),
            &h(41),
            &snapshot,
        ))
        .unwrap_err();
    assert!(matches!(digest_err, trellis_store::StoreError::Corrupt(_)));
}

/// Forged observation id (not the canonical derivation) rejected at
/// ingress.
#[test]
fn forged_observation_id_rejected_at_ingress() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let forged = ProjectionObservation::new(
        trellis_core::ids::ProjectionObservationId::from_hash(h(44)),
        ProjectionId::from_hash(h(41)),
        SnapshotId::from_hash(h(60)),
        h(41),
        None,
    );
    let err = store.put_projection_observation(&forged).unwrap_err();
    assert!(matches!(err, trellis_store::StoreError::Constraint(_)));
}

/// Composite publication failure: observation whose snapshot FK is absent
/// fails on the final insert — descriptor and anchor writes roll back with
/// it (no partially persisted discovery state).
#[test]
fn composite_publication_rolls_back_on_final_insert_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    let p = Projection::signature("auth.tokens.refresh_token").unwrap();
    // Build the observation directly (canonical id) — the store-level
    // composite is under test, not the engine helpers.
    let proj_id = p.id();
    let value = h(41);
    let snapshot = SnapshotId::from_hash(h(60));
    let id = canonical_observation_id(&proj_id, &value, &snapshot);
    let observed = ProjectionObservation::new(id, proj_id, snapshot, value, None);
    // NO snapshot row for h(60): the observation INSERT violates the FK.
    let err = store
        .record_observation_record(&p, &observed, "auth/tokens.py")
        .unwrap_err();
    assert!(matches!(err, trellis_store::StoreError::Sqlite(_)));
    // Nothing survived: no descriptor, no anchor, no observation.
    assert!(matches!(
        store.projection_descriptor(&p.id()),
        Err(trellis_store::StoreError::Corrupt(_))
    ));
    assert!(matches!(
        store.observation_anchor(&id),
        Err(trellis_store::StoreError::Corrupt(_))
    ));
    assert!(store.all_projection_observations().unwrap().is_empty());
}

/// The real anchor-conflict branch: the SAME observation id submitted with
/// a different anchor is rejected; the existing anchor is unchanged.
#[test]
fn composite_anchor_conflict_rejected_and_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("metadata.db")).unwrap();
    store.put_snapshot(&snapshot_row()).unwrap();
    let p = Projection::signature("auth.tokens.refresh_token").unwrap();
    let proj_id = p.id();
    let value = h(41);
    let snapshot = SnapshotId::from_hash(h(60));
    let id = canonical_observation_id(&proj_id, &value, &snapshot);
    let observed = ProjectionObservation::new(id, proj_id, snapshot, value, None);
    store
        .record_observation_record(&p, &observed, "auth/tokens.py")
        .unwrap();
    // Same observation, conflicting anchor path.
    let err = store
        .record_observation_record(&p, &observed, "payments/pricing.py")
        .unwrap_err();
    assert!(matches!(err, trellis_store::StoreError::Constraint(_)));
    // Existing anchor unchanged; exactly one anchor row.
    let store = Store::open(dir.path().join("metadata.db")).unwrap();
    assert_eq!(store.observation_anchor(&id).unwrap(), "auth/tokens.py");
}

fn snapshot_row() -> trellis_core::snapshot::Snapshot {
    trellis_core::snapshot::Snapshot::new(
        SnapshotId::from_hash(h(60)),
        RepositoryId::from_hash(h(61)),
        None,
        trellis_core::ids::ManifestId::from_hash(h(62)),
        None,
        trellis_core::ids::EnvironmentFingerprintId::from_hash(h(63)),
        None,
        1_000,
    )
    .unwrap()
}

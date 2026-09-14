//! Store root: connection management, schema versioning, WAL.

use std::path::{Path, PathBuf};

/// Persistence-layer errors. All explicit; corrupt/unsupported state is
/// never silently reinterpreted (spec §19).
#[derive(Debug)]
pub enum StoreError {
    /// SQLite error.
    Sqlite(rusqlite::Error),
    /// The persisted schema version is not supported by this build.
    UnsupportedSchema {
        /// The schema version found in the database.
        found: i64,
        /// The schema version supported by this build.
        supported: i64,
    },
    /// A referenced blob is missing from the CAS (dangling metadata).
    MissingBlob(String),
    /// A CAS operation failed.
    Cas(trellis_cas::CasError),
    /// The requested object does not exist.
    NotFound(String),
    /// A uniqueness/domain constraint was violated (e.g. duplicate
    /// attestation identity, artifact/attestation mismatch).
    Constraint(String),
    /// Persisted state is corrupt or incomplete.
    Corrupt(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "sqlite: {e}"),
            StoreError::UnsupportedSchema { found, supported } => {
                write!(
                    f,
                    "unsupported schema version {found} (supported: {supported})"
                )
            }
            StoreError::MissingBlob(id) => write!(f, "metadata references missing blob {id}"),
            StoreError::Cas(e) => write!(f, "cas: {e}"),
            StoreError::NotFound(what) => write!(f, "not found: {what}"),
            StoreError::Constraint(msg) => write!(f, "constraint violation: {msg}"),
            StoreError::Corrupt(msg) => write!(f, "corrupt persisted state: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<trellis_cas::CasError> for StoreError {
    fn from(e: trellis_cas::CasError) -> Self {
        StoreError::Cas(e)
    }
}

impl From<trellis_core::error::DomainError> for StoreError {
    fn from(e: trellis_core::error::DomainError) -> Self {
        StoreError::Corrupt(format!("domain invariant violated: {e}"))
    }
}

/// Schema version supported by this build. Databases with a different
/// version are rejected explicitly (spec §19).
pub const SCHEMA_VERSION: i64 = 1;

/// The durable store: SQLite metadata + CAS blob placement.
pub struct Store {
    conn: rusqlite::Connection,
    path: PathBuf,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

impl Store {
    /// Open (or initialize) a store at `.trellis/metadata.db`.
    ///
    /// # Errors
    /// [`StoreError::Sqlite`] on open/pragma failures;
    /// [`StoreError::UnsupportedSchema`] when an existing database carries
    /// a schema version this build does not support.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path_buf = path.as_ref().to_path_buf();
        let path = path_buf.as_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        // WAL mode for concurrent readers + single writer (spec §19).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// The database file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn init_schema(conn: &rusqlite::Connection) -> Result<(), StoreError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
        let version: Option<i64> = conn_schema_version(conn)?;
        match version {
            None => {
                conn.execute(
                    "INSERT INTO schema_meta (key, value) VALUES ('schema_version', ?1)",
                    [SCHEMA_VERSION.to_string()],
                )?;
                conn.execute_batch(SCHEMA)?;
                conn.execute_batch(ENGINE_SCHEMA)?;
                Ok(())
            }
            Some(v) if v == SCHEMA_VERSION => {
                // Additive engine schema (M5): idempotent, compatible with
                // v1 rows — no reinterpretation of persisted objects
                // (recorded in DECISIONS).
                conn.execute_batch(ENGINE_SCHEMA)?;
                Ok(())
            }
            Some(v) => Err(StoreError::UnsupportedSchema {
                found: v,
                supported: SCHEMA_VERSION,
            }),
        }
    }

    /// Whether WAL mode is active on this connection.
    #[must_use]
    pub fn wal_enabled(&self) -> bool {
        self.conn
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map(|m| m.eq_ignore_ascii_case("wal"))
            .unwrap_or(false)
    }

    /// Whether foreign-key enforcement is on.
    #[must_use]
    pub fn foreign_keys_enabled(&self) -> bool {
        self.conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
            .map(|v| v == 1)
            .unwrap_or(false)
    }

    /// Run `f` inside a transaction; on `Err` the transaction rolls back,
    /// leaving no half-persisted metadata graph.
    ///
    /// # Errors
    /// Whatever `f` returns, or a SQLite error on commit.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&rusqlite::Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let tx = self.conn.transaction()?;
        let outcome = f(&tx);
        match outcome {
            Ok(v) => {
                tx.commit()?;
                Ok(v)
            }
            Err(e) => {
                let _ = tx.rollback();
                Err(e)
            }
        }
    }
}

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS snapshots (
    id TEXT PRIMARY KEY,
    repository_id TEXT NOT NULL,
    git_commit TEXT,
    manifest_id TEXT NOT NULL,
    semantic_snapshot TEXT,
    environment_id TEXT NOT NULL,
    parent_id TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY,
    schema_version INTEGER NOT NULL,
    kind TEXT NOT NULL,
    payload_ref TEXT NOT NULL,
    proposition TEXT,
    producer TEXT NOT NULL,            -- canonical producer encoding
    derivation TEXT NOT NULL,
    dependencies TEXT NOT NULL,        -- canonical JSON array of projection ids
    created_snapshot TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    cost TEXT NOT NULL,                -- canonical JSON cost record
    FOREIGN KEY (payload_ref) REFERENCES cas_objects(id)
);

CREATE TABLE IF NOT EXISTS derivations (
    id TEXT PRIMARY KEY,
    capture_trust TEXT NOT NULL,
    channels TEXT NOT NULL,            -- canonical JSON array
    producer TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS artifact_dependencies (
    artifact_id TEXT NOT NULL,
    projection_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (artifact_id, position),
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS validation_attestations (
    id TEXT PRIMARY KEY,
    artifact_id TEXT NOT NULL,
    target_source_snapshot TEXT NOT NULL,
    target_semantic_snapshot TEXT,
    validity TEXT NOT NULL,
    authority TEXT NOT NULL,
    verification_level TEXT NOT NULL,
    verifier TEXT,
    evidence TEXT NOT NULL,            -- canonical JSON array
    capture_trust TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    seq INTEGER,                       -- append order within history; NULL until assigned
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);

CREATE TABLE IF NOT EXISTS projection_observations (
    id TEXT PRIMARY KEY,
    projection TEXT NOT NULL,          -- canonical projection key
    snapshot TEXT NOT NULL,
    value_digest TEXT NOT NULL,
    canonical_value TEXT,
    FOREIGN KEY (snapshot) REFERENCES snapshots(id)
);

CREATE TABLE IF NOT EXISTS cas_objects (
    id TEXT PRIMARY KEY                -- blake3:<hex>; rows are written only after durable CAS placement
);
"#;

/// Additive M5 engine schema: the discovery reverse indexes. Idempotent;
/// safe to run on any v1 database; reinterprets no persisted rows
/// (recorded in DECISIONS).
/// - `symbol_locations`: symbol → defining source unit(s). Composite PK
///   preserves ALL historical locations (a symbol recorded in two files
///   keeps both rows — over-approximation is sound, omission is not).
/// - `projection_descriptors`: canonical projection key per ProjectionId
///   (kind/subject/scope), recorded at observation time so discovery
///   survives restart without caller-supplied key maps.
/// - `observation_anchors`: the source unit an observation was evaluated
///   against — the file→observations reverse index.
pub(crate) const ENGINE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS symbol_locations (
    symbol TEXT NOT NULL,
    module TEXT NOT NULL,
    source_path TEXT NOT NULL,
    PRIMARY KEY (symbol, source_path)
);
CREATE TABLE IF NOT EXISTS projection_descriptors (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    subject_variant TEXT NOT NULL,
    subject TEXT NOT NULL,
    scope TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS observation_anchors (
    observation_id TEXT PRIMARY KEY,
    source_path TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS completeness_bindings (
    projection_id TEXT PRIMARY KEY,
    snapshot TEXT NOT NULL,
    universe_digest TEXT NOT NULL,
    coverage_digest TEXT NOT NULL,
    indexer TEXT NOT NULL,
    completeness TEXT NOT NULL
);
"#;

pub(crate) fn conn_schema_version(conn: &rusqlite::Connection) -> rusqlite::Result<Option<i64>> {
    match conn.query_row(
        "SELECT value FROM schema_meta WHERE key = 'schema_version'",
        [],
        |row| row.get::<_, String>(0),
    ) {
        Ok(s) => Ok(s.parse::<i64>().ok()),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Centralized observation decode with canonical-id verification: a
/// persisted observation id that does not re-derive from its fields is
/// corrupt state, explicitly failed closed (spec §8, §19).
fn decode_observation_row(
    oid: &str,
    proj: &str,
    snap: &str,
    digest: &str,
    canonical: Option<&str>,
) -> Result<trellis_core::projection::ProjectionObservation, StoreError> {
    let proj_id = parse_typed(proj, "projection")?;
    let snap_id = parse_typed(snap, "snapshot")?;
    let value = parse_typed(digest, "digest")?;
    let id = parse_typed(oid, "observation")?;
    let derived =
        trellis_core::observation_id::canonical_observation_id(&proj_id, &value, &snap_id);
    if derived != id {
        return Err(StoreError::Corrupt(format!(
            "observation id {oid} does not re-derive from its fields"
        )));
    }
    Ok(trellis_core::projection::ProjectionObservation::new(
        id,
        proj_id,
        snap_id,
        value,
        canonical
            .map(|c| c.parse())
            .transpose()
            .map_err(|_| StoreError::Corrupt("bad blob id".into()))?,
    ))
}

pub(crate) fn ensure_blob(conn: &rusqlite::Connection, blob_id: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR IGNORE INTO cas_objects (id) VALUES (?1)",
        [blob_id],
    )?;
    Ok(())
}

pub(crate) fn require_blob(conn: &rusqlite::Connection, blob_id: &str) -> Result<(), StoreError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cas_objects WHERE id = ?1",
        [blob_id],
        |row| row.get(0),
    )?;
    if n == 0 {
        return Err(StoreError::MissingBlob(blob_id.to_string()));
    }
    Ok(())
}

// ─── canonical encodings ────────────────────────────────────────────

/// Parse a typed content id from its canonical rendering.
fn parse_typed<T>(s: &str, what: &str) -> Result<T, StoreError>
where
    T: std::str::FromStr,
{
    s.parse()
        .map_err(|_| StoreError::Corrupt(format!("bad {what} id: {s}")))
}

// Storage is not a second identity source: domain objects are persisted
// with their canonical IDs and round-trip to domain equality. JSON here is
// a durable *carrier*, not a competing canonical form.

fn enc_list(items: &[String]) -> String {
    serde_json::to_string(items).expect("list encoding cannot fail")
}

fn dec_list(json: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(json)
        .map_err(|_| StoreError::Corrupt(format!("malformed list encoding: {json}")))
}

fn evidence_enc(e: &trellis_core::attestation::EvidenceRef) -> String {
    use trellis_core::attestation::EvidenceRef;
    match e {
        EvidenceRef::Blob(b) => format!("blob:{}", b),
        EvidenceRef::Observation(o) => format!("obs:{}", o),
        EvidenceRef::PriorAttestation(a) => format!("att:{}", a),
    }
}

fn evidence_dec(s: &str) -> Result<trellis_core::attestation::EvidenceRef, StoreError> {
    use trellis_core::attestation::EvidenceRef;
    let (kind, id) = s
        .split_once(':')
        .ok_or_else(|| StoreError::Corrupt(format!("malformed evidence ref: {s}")))?;
    Ok(match kind {
        "blob" => EvidenceRef::Blob(
            id.parse()
                .map_err(|_| StoreError::Corrupt(format!("bad evidence blob id: {s}")))?,
        ),
        "obs" => EvidenceRef::Observation(
            id.parse()
                .map_err(|_| StoreError::Corrupt(format!("bad evidence obs id: {s}")))?,
        ),
        "att" => EvidenceRef::PriorAttestation(
            id.parse()
                .map_err(|_| StoreError::Corrupt(format!("bad evidence att id: {s}")))?,
        ),
        _ => return Err(StoreError::Corrupt(format!("unknown evidence kind: {s}"))),
    })
}

fn cost_enc(c: &trellis_core::artifact::CostRecord) -> String {
    serde_json::json!({
        "capture_ms": c.capture_ms,
        "validation_ms": c.validation_ms,
        "recompute_ms": c.recompute_ms,
        "tokens": c.tokens,
    })
    .to_string()
}

fn cost_field(v: &serde_json::Value, key: &str) -> Result<Option<u64>, StoreError> {
    match v.get(key) {
        None => Err(StoreError::Corrupt(format!(
            "cost record missing key `{key}`"
        ))),
        Some(serde_json::Value::Null) => Ok(None),
        Some(x) => x
            .as_u64()
            .map(Some)
            .ok_or_else(|| StoreError::Corrupt(format!("cost field `{key}` is not u64/null"))),
    }
}

fn cost_dec(s: &str) -> Result<trellis_core::artifact::CostRecord, StoreError> {
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|_| StoreError::Corrupt("bad cost record".into()))?;
    Ok(trellis_core::artifact::CostRecord {
        capture_ms: cost_field(&v, "capture_ms")?,
        validation_ms: cost_field(&v, "validation_ms")?,
        recompute_ms: cost_field(&v, "recompute_ms")?,
        tokens: cost_field(&v, "tokens")?,
    })
}

fn producer_enc(p: &trellis_core::artifact::ProducerInfo) -> String {
    serde_json::json!({
        "harness": p.harness(),
        "version": p.version(),
        "model": p.model(),
    })
    .to_string()
}

fn producer_dec(s: &str) -> Result<trellis_core::artifact::ProducerInfo, StoreError> {
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|_| StoreError::Corrupt("bad producer".into()))?;
    // Strict field decode: producer provenance is evidentiary — a mistyped
    // value is corrupt state, never a silently dropped identifier (§19).
    let harness = v
        .get("harness")
        .and_then(|x| x.as_str())
        .ok_or_else(|| StoreError::Corrupt("producer harness missing/non-string".into()))?;
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .ok_or_else(|| StoreError::Corrupt("producer version missing/non-string".into()))?;
    let model = match v.get("model") {
        None => return Err(StoreError::Corrupt("producer model key missing".into())),
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(other) => {
            return Err(StoreError::Corrupt(format!(
                "producer model is neither null nor string: {other}"
            )))
        }
    };
    trellis_core::artifact::ProducerInfo::new(harness, version, model)
        .map_err(|_| StoreError::Corrupt("producer fields empty".into()))
}

fn parse_capture_trust(s: &str) -> Result<trellis_core::validity::CaptureTrust, StoreError> {
    use trellis_core::validity::CaptureTrust;
    match s {
        "Unobserved" => Ok(CaptureTrust::Unobserved),
        "DeclaredOnly" => Ok(CaptureTrust::DeclaredOnly),
        "Partial" => Ok(CaptureTrust::Partial),
        "Complete" => Ok(CaptureTrust::Complete),
        _ => Err(StoreError::Corrupt(format!("bad capture trust {s}"))),
    }
}

fn parse_validity(s: &str) -> Result<trellis_core::validity::Validity, StoreError> {
    use trellis_core::validity::Validity;
    match s {
        "Valid" => Ok(Validity::Valid),
        "Stale" => Ok(Validity::Stale),
        "Unknown" => Ok(Validity::Unknown),
        _ => Err(StoreError::Corrupt(format!("bad validity {s}"))),
    }
}

fn parse_authority(s: &str) -> Result<trellis_core::validity::Authority, StoreError> {
    use trellis_core::validity::Authority;
    match s {
        "Authoritative" => Ok(Authority::Authoritative),
        "Provisional" => Ok(Authority::Provisional),
        _ => Err(StoreError::Corrupt(format!("bad authority {s}"))),
    }
}

fn parse_verification(s: &str) -> Result<trellis_core::validity::VerificationLevel, StoreError> {
    use trellis_core::validity::VerificationLevel;
    match s {
        "Unverified" => Ok(VerificationLevel::Unverified),
        "EvidenceBacked" => Ok(VerificationLevel::EvidenceBacked),
        "Structural" => Ok(VerificationLevel::Structural),
        "Test" => Ok(VerificationLevel::Test),
        "Static" => Ok(VerificationLevel::Static),
        "Deterministic" => Ok(VerificationLevel::Deterministic),
        _ => Err(StoreError::Corrupt(format!("bad verification level {s}"))),
    }
}

fn parse_kind(s: &str) -> Result<trellis_core::artifact::ArtifactKind, StoreError> {
    match s {
        "StructuralSet" => Ok(ArtifactKind::StructuralSet),
        "Observation" => Ok(ArtifactKind::Observation),
        "Fact" => Ok(ArtifactKind::Fact),
        "DerivedFact" => Ok(ArtifactKind::DerivedFact),
        "Summary" => Ok(ArtifactKind::Summary),
        "Hypothesis" => Ok(ArtifactKind::Hypothesis),
        "Plan" => Ok(ArtifactKind::Plan),
        "ExecutionResult" => Ok(ArtifactKind::ExecutionResult),
        "Patch" => Ok(ArtifactKind::Patch),
        _ => Err(StoreError::Corrupt(format!("bad artifact kind {s}"))),
    }
}

use trellis_core::artifact::{ArtifactKind, ArtifactKind as AK};

// silence unused alias
#[allow(unused_imports)]
use AK as _AK;

// ─── snapshot repository ────────────────────────────────────────────

impl Store {
    /// Register a blob id in metadata. **Durability gate**: the blob must
    /// already exist in the CAS (`FsCas::put` completed); otherwise the
    /// registration is refused — metadata can never lead the blob (§19).
    ///
    /// # Errors
    /// [`StoreError::MissingBlob`] when the blob is absent from the CAS;
    /// SQLite failures.
    pub fn register_blob(&mut self, id: &trellis_core::ids::BlobId) -> Result<(), StoreError> {
        if !self.open_cas()?.contains(id) {
            return Err(StoreError::MissingBlob(id.to_string()));
        }
        ensure_blob(&self.conn, &id.to_string())?;
        Ok(())
    }

    /// Persist a snapshot. Idempotent by canonical identity.
    ///
    /// # Errors
    /// SQLite failures.
    pub fn put_snapshot(&mut self, s: &trellis_core::snapshot::Snapshot) -> Result<(), StoreError> {
        self.transaction(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO snapshots
                 (id, repository_id, git_commit, manifest_id, semantic_snapshot,
                  environment_id, parent_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    s.id().to_string(),
                    s.repository_id().to_string(),
                    s.git_commit().map(|c| c.to_string()),
                    s.working_tree_manifest().to_string(),
                    s.semantic_snapshot().map(|m| m.to_string()),
                    s.environment().to_string(),
                    s.parent().map(|p| p.to_string()),
                    s.created_at() as i64,
                ],
            )?;
            Ok(())
        })
    }

    /// Load a snapshot by canonical id.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when absent; SQLite failures.
    pub fn get_snapshot(
        &self,
        id: &trellis_core::ids::SnapshotId,
    ) -> Result<trellis_core::snapshot::Snapshot, StoreError> {
        let row = self.conn.query_row(
            "SELECT id, repository_id, git_commit, manifest_id, semantic_snapshot,
                    environment_id, parent_id, created_at
             FROM snapshots WHERE id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            },
        );
        match row {
            Ok((sid, repo, commit, manifest, semantic, env, parent, created)) => {
                Ok(trellis_core::snapshot::Snapshot::new(
                    parse_typed(&sid, "snapshot")?,
                    parse_typed(&repo, "repository")?,
                    commit
                        .map(|c| c.parse())
                        .transpose()
                        .map_err(|_| StoreError::Corrupt("bad git commit".into()))?,
                    parse_typed(&manifest, "manifest")?,
                    semantic
                        .map(|m| parse_typed(&m, "semantic snapshot"))
                        .transpose()?,
                    parse_typed(&env, "environment")?,
                    parent
                        .map(|x| parse_typed(&x, "parent snapshot"))
                        .transpose()?,
                    created as u64,
                )?)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(StoreError::NotFound(format!("snapshot {id}")))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Persist a derivation (idempotent by canonical identity).
    ///
    /// # Errors
    /// SQLite failures.
    pub fn put_derivation(
        &mut self,
        d: &trellis_core::artifact::Derivation,
    ) -> Result<(), StoreError> {
        self.transaction(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO derivations
                 (id, capture_trust, channels, producer, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    d.id().to_string(),
                    format!("{:?}", d.capture_trust()),
                    enc_list(
                        &d.channels()
                            .iter()
                            .map(|c| format!("{c:?}"))
                            .collect::<Vec<_>>()
                    ),
                    producer_enc(d.producer()),
                    d.created_at() as i64,
                ],
            )?;
            Ok(())
        })
    }

    /// Load a derivation by canonical id.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when absent; SQLite failures.
    pub fn get_derivation(
        &self,
        id: &trellis_core::ids::DerivationId,
    ) -> Result<trellis_core::artifact::Derivation, StoreError> {
        let row = self.conn.query_row(
            "SELECT id, capture_trust, channels, producer, created_at
             FROM derivations WHERE id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        );
        match row {
            Ok((did, _trust, channels, producer, created)) => {
                // capture_trust is derivable from channels (the derivation
                // constructor recomputes it identically); the persisted
                // column is checked by attestation round-trip tests.
                let channels: Vec<String> = dec_list(&channels)?;
                let channels = channels
                    .iter()
                    .map(|c| parse_channel(c))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| StoreError::Corrupt("bad capture channel".into()))?;
                Ok(trellis_core::artifact::Derivation::new(
                    parse_typed(&did, "derivation")?,
                    channels,
                    producer_dec(&producer)?,
                    created as u64,
                ))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(StoreError::NotFound(format!("derivation {id}")))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Persist an artifact envelope. **Blob first, metadata second**: the
    /// payload blob must already be durably placed in the CAS
    /// ([`Store::register_blob`]); otherwise the metadata commit is
    /// refused (spec §19). Idempotent by canonical identity.
    ///
    /// # Errors
    /// [`StoreError::MissingBlob`] when the payload was never durably
    /// placed; SQLite failures.
    pub fn put_artifact(
        &mut self,
        a: &trellis_core::artifact::ArtifactEnvelope,
    ) -> Result<(), StoreError> {
        // Blob-first gate: reject metadata that would dangle.
        require_blob(&self.conn, &a.payload_ref().to_string())?;
        self.transaction(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO derivations
                 (id, capture_trust, channels, producer, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    a.derivation().id().to_string(),
                    format!("{:?}", a.derivation().capture_trust()),
                    enc_list(
                        &a.derivation()
                            .channels()
                            .iter()
                            .map(|c| format!("{c:?}"))
                            .collect::<Vec<_>>()
                    ),
                    producer_enc(a.derivation().producer()),
                    a.derivation().created_at() as i64,
                ],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO artifacts
                 (id, schema_version, kind, payload_ref, proposition, producer,
                  derivation, dependencies, created_snapshot, created_at, cost)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    a.id().to_string(),
                    a.schema_version() as i64,
                    format!("{:?}", a.kind()),
                    a.payload_ref().to_string(),
                    a.proposition().map(|p| p.to_string()),
                    producer_enc(a.producer()),
                    a.derivation().id().to_string(),
                    enc_list(
                        &a.dependencies()
                            .iter()
                            .map(|d| d.to_string())
                            .collect::<Vec<_>>()
                    ),
                    a.created_snapshot().to_string(),
                    a.derivation().created_at() as i64,
                    cost_enc(&a.cost()),
                ],
            )?;
            for (pos, dep) in a.dependencies().iter().enumerate() {
                conn.execute(
                    "INSERT OR IGNORE INTO artifact_dependencies
                     (artifact_id, projection_id, position) VALUES (?1, ?2, ?3)",
                    rusqlite::params![a.id().to_string(), dep.to_string(), pos as i64],
                )?;
            }
            Ok(())
        })
    }

    /// Load an artifact envelope by canonical id.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when absent; SQLite failures.
    pub fn get_artifact(
        &self,
        id: &trellis_core::ids::ArtifactId,
    ) -> Result<trellis_core::artifact::ArtifactEnvelope, StoreError> {
        let row = self.conn.query_row(
            "SELECT a.id, a.schema_version, a.kind, a.payload_ref, a.proposition,
                    a.producer, d.id, d.created_at, a.created_snapshot, a.cost
             FROM artifacts a JOIN derivations d ON d.id = a.derivation
             WHERE a.id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                ))
            },
        );
        let (aid, schema, kind, payload, proposition, producer, deriv_id, _created, snapshot, cost) =
            match row {
                Ok(v) => v,
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    return Err(StoreError::NotFound(format!("artifact {id}")))
                }
                Err(e) => return Err(e.into()),
            };
        let deps: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT projection_id FROM artifact_dependencies
                 WHERE artifact_id = ?1 ORDER BY position",
            )?;
            let rows = stmt.query_map([aid.as_str()], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let derivation = self.get_derivation(
            &deriv_id
                .parse()
                .map_err(|_| StoreError::Corrupt("bad derivation id".into()))?,
        )?;
        Ok(trellis_core::artifact::ArtifactEnvelope::new(
            parse_typed(&aid, "artifact")?,
            schema as u32,
            parse_kind(&kind)?,
            parse_typed(&payload, "payload blob")?,
            proposition
                .map(trellis_core::artifact::Proposition::new)
                .transpose()?,
            producer_dec(&producer)?,
            derivation,
            deps.iter()
                .map(|d| parse_typed(d, "projection"))
                .collect::<Result<Vec<_>, StoreError>>()?,
            parse_typed(&snapshot, "snapshot")?,
            cost_dec(&cost)?,
        )?)
    }

    /// Append an attestation. **Append-only**: no API mutates or removes
    /// existing attestations. Rejects duplicate identities and foreign
    /// artifacts (spec §12 domain invariants).
    ///
    /// # Errors
    /// [`StoreError::Constraint`] on duplicate identity/foreign artifact;
    /// SQLite failures.
    pub fn append_attestation(
        &mut self,
        att: &trellis_core::attestation::ValidationAttestation,
    ) -> Result<(), StoreError> {
        self.transaction(|conn| {
            let dup: i64 = conn.query_row(
                "SELECT COUNT(*) FROM validation_attestations WHERE id = ?1",
                [att.id().to_string()],
                |r| r.get(0),
            )?;
            if dup > 0 {
                return Err(StoreError::Constraint(format!(
                    "attestation {} already recorded",
                    att.id()
                )));
            }
            let known: i64 = conn.query_row(
                "SELECT COUNT(*) FROM artifacts WHERE id = ?1",
                [att.artifact_id().to_string()],
                |r| r.get(0),
            )?;
            if known == 0 {
                return Err(StoreError::Constraint(format!(
                    "attestation {} references unknown artifact {}",
                    att.id(),
                    att.artifact_id()
                )));
            }
            // Preserve the domain's non-decreasing created_at append rule
            // at the storage boundary (spec §12): a new attestation may not
            // precede the latest recorded one.
            let latest: Option<i64> = conn.query_row(
                "SELECT MAX(created_at) FROM validation_attestations WHERE artifact_id = ?1",
                [att.artifact_id().to_string()],
                |r| r.get::<_, Option<i64>>(0),
            )?;
            if let Some(latest) = latest {
                if (att.created_at() as i64) < latest {
                    return Err(StoreError::Constraint(format!(
                        "attestation created_at {} precedes latest {} (append-only history)",
                        att.created_at(),
                        latest
                    )));
                }
            }
            conn.execute(
                "INSERT INTO validation_attestations
                 (id, artifact_id, target_source_snapshot, target_semantic_snapshot,
                  validity, authority, verification_level, verifier, evidence,
                  capture_trust, created_at, seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                         (SELECT COALESCE(MAX(seq), 0) + 1 FROM validation_attestations
                          WHERE artifact_id = ?2))",
                rusqlite::params![
                    att.id().to_string(),
                    att.artifact_id().to_string(),
                    att.target_source_snapshot().to_string(),
                    att.target_semantic_snapshot().map(|s| s.to_string()),
                    format!("{:?}", att.validity()),
                    format!("{:?}", att.authority()),
                    format!("{:?}", att.verification_level()),
                    att.verifier().map(|v| v.to_string()),
                    enc_list(&att.evidence().iter().map(evidence_enc).collect::<Vec<_>>()),
                    format!("{:?}", att.capture_trust()),
                    att.created_at() as i64,
                ],
            )?;
            Ok(())
        })
    }

    /// Load an artifact's attestation history in append order.
    ///
    /// # Errors
    /// SQLite failures.
    pub fn attestation_history(
        &self,
        artifact: &trellis_core::ids::ArtifactId,
    ) -> Result<trellis_core::attestation::AttestationHistory, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, target_source_snapshot, target_semantic_snapshot, validity,
                    authority, verification_level, verifier, evidence, capture_trust,
                    created_at, seq
             FROM validation_attestations WHERE artifact_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([artifact.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, Option<i64>>(10)?,
            ))
        })?;
        let mut history = trellis_core::attestation::AttestationHistory::new(*artifact);
        for row in rows {
            let (
                aid,
                src_snap,
                sem_snap,
                validity,
                authority,
                verif,
                verifier,
                evidence,
                trust,
                created,
                _seq,
            ) = row?;
            let att = trellis_core::attestation::ValidationAttestation::from_persisted(
                parse_typed(&aid, "attestation")?,
                *artifact,
                parse_typed(&src_snap, "snapshot")?,
                sem_snap
                    .map(|s| parse_typed(&s, "semantic snapshot"))
                    .transpose()?,
                parse_validity(&validity)?,
                parse_authority(&authority)?,
                parse_verification(&verif)?,
                verifier.map(|v| parse_typed(&v, "verifier")).transpose()?,
                dec_list(&evidence)?
                    .iter()
                    .map(|e| evidence_dec(e))
                    .collect::<Result<Vec<_>, StoreError>>()?,
                parse_capture_trust(&trust)?,
                created as u64,
            );
            history.append(att)?;
        }
        Ok(history)
    }

    /// Persist a projection observation (idempotent by canonical identity).
    ///
    /// # Errors
    /// SQLite failures.
    pub fn put_projection_observation(
        &mut self,
        obs: &trellis_core::projection::ProjectionObservation,
    ) -> Result<(), StoreError> {
        // Ingress gate: only canonical observation identities are durable
        // (a forged id could never re-derive on load — refuse now, §19).
        let derived = trellis_core::observation_id::canonical_observation_id(
            &obs.projection(),
            obs.value_digest(),
            &obs.snapshot(),
        );
        if derived != obs.id() {
            return Err(StoreError::Constraint(format!(
                "observation id {} is not the canonical derivation of its fields",
                obs.id()
            )));
        }
        self.transaction(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO projection_observations
                 (id, projection, snapshot, value_digest, canonical_value)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    obs.id().to_string(),
                    obs.projection().to_string(),
                    obs.snapshot().to_string(),
                    obs.value_digest().to_string(),
                    obs.canonical_value().map(|b| b.to_string()),
                ],
            )?;
            Ok(())
        })
    }

    /// Load a projection observation by canonical id.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when absent; SQLite failures.
    pub fn get_projection_observation(
        &self,
        id: &trellis_core::ids::ProjectionObservationId,
    ) -> Result<trellis_core::projection::ProjectionObservation, StoreError> {
        let row = self.conn.query_row(
            "SELECT id, projection, snapshot, value_digest, canonical_value
             FROM projection_observations WHERE id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            },
        );
        match row {
            Ok((oid, proj, snap, digest, canonical)) => {
                decode_observation_row(&oid, &proj, &snap, &digest, canonical.as_deref())
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(StoreError::NotFound(format!("observation {id}")))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Durably place canonical bytes in the CAS and register the blob id.
    /// The only sanctioned path for metadata to reference a blob: blob
    /// first, metadata second (spec §19).
    ///
    /// # Errors
    /// CAS failures; SQLite failures.
    pub fn put_blob(&mut self, bytes: &[u8]) -> Result<trellis_core::ids::BlobId, StoreError> {
        let cas = self.open_cas()?;
        let id = cas.put(bytes)?;
        self.transaction(|conn| ensure_blob(conn, &id.to_string()))?;
        Ok(id)
    }

    /// Remove a blob's registry row (test support for the crash window
    /// between CAS rename and metadata commit; not part of normal flow).
    ///
    /// # Errors
    /// SQLite failures.
    #[doc(hidden)]
    pub fn remove_blob_registration(
        &mut self,
        id: &trellis_core::ids::BlobId,
    ) -> Result<(), StoreError> {
        self.transaction(|conn| {
            conn.execute("DELETE FROM cas_objects WHERE id = ?1", [id.to_string()])?;
            Ok(())
        })
    }

    /// Collect orphan blobs: durable CAS objects with no metadata
    /// registry row (e.g. from a crash between CAS rename and metadata
    /// commit). Returns their ids; nothing is deleted implicitly.
    ///
    /// # Errors
    /// CAS or SQLite failures.
    pub fn collect_orphans(&self) -> Result<Vec<trellis_core::ids::BlobId>, StoreError> {
        let cas = self.open_cas()?;
        let mut orphans = Vec::new();
        let mut stack = vec![cas.root_path().to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .map_err(|e| trellis_cas::CasError::Io(dir.clone(), e))?
                .flatten()
            {
                let p = entry.path();
                if p.is_dir() {
                    if p.file_name().map(|n| n != "tmp").unwrap_or(true) {
                        stack.push(p);
                    }
                    continue;
                }
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    let id_str = format!("blake3:{name}");
                    if let Ok(id) = id_str.parse::<trellis_core::ids::BlobId>() {
                        let registered: i64 = self.conn.query_row(
                            "SELECT COUNT(*) FROM cas_objects WHERE id = ?1",
                            [&id_str],
                            |r| r.get(0),
                        )?;
                        if registered == 0 {
                            orphans.push(id);
                        }
                    }
                }
            }
        }
        orphans.sort();
        orphans.dedup();
        Ok(orphans)
    }

    /// Persist a snapshot. Idempotent by canonical identity.
    ///
    /// # Errors
    /// [`StoreError::MissingBlob`] if not registered; CAS errors.
    pub fn get_blob(&self, id: &trellis_core::ids::BlobId) -> Result<Vec<u8>, StoreError> {
        let cas = self.open_cas()?;
        Ok(cas.get(id)?)
    }

    fn open_cas(&self) -> Result<trellis_cas::FsCas, StoreError> {
        // CAS lives next to metadata.db: .trellis/cas
        let cas_root = self
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("cas");
        Ok(trellis_cas::FsCas::open(cas_root)?)
    }
}

fn parse_channel(s: &str) -> Option<trellis_core::validity::CaptureChannel> {
    use trellis_core::validity::CaptureChannel;
    // format!("{c:?}") of the enum variants.
    let v = s.trim_start_matches("CaptureChannel::");
    Some(match v {
        "TrellisTool" => CaptureChannel::TrellisTool,
        "FullyIntercepted" => CaptureChannel::FullyIntercepted,
        "PartiallyCaptured" => CaptureChannel::PartiallyCaptured,
        "DeclaredOnly" => CaptureChannel::DeclaredOnly,
        "Unobserved" => CaptureChannel::Unobserved,
        _ => return None,
    })
}

// ─── engine support: discovery reverse indexes ──────────────────────

impl Store {
    /// Record a symbol's defining source unit (symbol→file reverse index).
    /// Idempotent per (symbol, source_path); ALL historical locations are
    /// preserved — discovery must never lose an anchor.
    ///
    /// # Errors
    /// SQLite failures.
    pub fn put_symbol_location(
        &mut self,
        symbol: &str,
        module: &str,
        source_path: &str,
    ) -> Result<(), StoreError> {
        self.transaction(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO symbol_locations (symbol, module, source_path)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![symbol, module, source_path],
            )?;
            Ok(())
        })
    }

    /// All recorded symbol locations, sorted by (symbol, source_path).
    ///
    /// # Errors
    /// SQLite failures.
    pub fn symbol_locations(&self) -> Result<Vec<(String, String, String)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT symbol, module, source_path FROM symbol_locations
             ORDER BY symbol, source_path",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Atomically persist one observation's full discovery state: the
    /// observation row, its projection descriptor, and its source anchor —
    /// one transaction, never partially persisted (M4 gate; engine
    /// `record_observation` is the sanctioned caller).
    ///
    /// # Errors
    /// [`StoreError::Constraint`] on identity-pairing, canonical-id, or
    /// anchor-conflict violations; SQLite failures.
    pub fn record_observation_record(
        &mut self,
        projection: &trellis_core::projection::Projection,
        observation: &trellis_core::projection::ProjectionObservation,
        anchor_path: &str,
    ) -> Result<(), StoreError> {
        // Ingress gates (fail before any write).
        if projection.id() != observation.projection() {
            return Err(StoreError::Constraint(format!(
                "observation {} does not carry projection {}'s id",
                observation.id(),
                projection.id()
            )));
        }
        let derived = trellis_core::observation_id::canonical_observation_id(
            &observation.projection(),
            observation.value_digest(),
            &observation.snapshot(),
        );
        if derived != observation.id() {
            return Err(StoreError::Constraint(format!(
                "observation id {} is not the canonical derivation of its fields",
                observation.id()
            )));
        }
        self.transaction(|conn| {
            let variant = match projection.subject() {
                trellis_core::projection::Subject::File(_) => "File",
                trellis_core::projection::Subject::Symbol(_) => "Symbol",
                trellis_core::projection::Subject::Module(_) => "Module",
                trellis_core::projection::Subject::Text(_) => "Text",
                trellis_core::projection::Subject::ConfigKey(_) => "ConfigKey",
                trellis_core::projection::Subject::Tool(_) => "Tool",
            };
            conn.execute(
                "INSERT OR IGNORE INTO projection_descriptors
                 (id, kind, subject_variant, subject, scope)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    projection.id().to_string(),
                    projection.kind().keyword(),
                    variant,
                    projection.subject().canonical(),
                    projection.scope_name(),
                ],
            )?;
            let existing: Option<String> = conn
                .query_row(
                    "SELECT source_path FROM observation_anchors WHERE observation_id = ?1",
                    [observation.id().to_string()],
                    |r| r.get(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            match existing {
                Some(p) if p == anchor_path => {}
                Some(p) => {
                    return Err(StoreError::Constraint(format!(
                        "observation {} already anchored to {p}, refusing conflicting anchor {anchor_path}",
                        observation.id()
                    )))
                }
                None => {
                    conn.execute(
                        "INSERT INTO observation_anchors (observation_id, source_path)
                         VALUES (?1, ?2)",
                        rusqlite::params![observation.id().to_string(), anchor_path],
                    )?;
                }
            }
            conn.execute(
                "INSERT OR IGNORE INTO projection_observations
                 (id, projection, snapshot, value_digest, canonical_value)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    observation.id().to_string(),
                    observation.projection().to_string(),
                    observation.snapshot().to_string(),
                    observation.value_digest().to_string(),
                    observation.canonical_value().map(|b| b.to_string()),
                ],
            )?;
            Ok(())
        })
    }

    /// Artifacts that declare a dependency on the given projection
    /// (artifact→projection adjacency, spec §19). Deterministic order.
    ///
    /// # Errors
    /// SQLite failures.
    pub fn artifacts_by_dependency(
        &self,
        projection: &trellis_core::ids::ProjectionId,
    ) -> Result<Vec<trellis_core::ids::ArtifactId>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT artifact_id FROM artifact_dependencies
             WHERE projection_id = ?1 ORDER BY artifact_id",
        )?;
        let rows = stmt.query_map([projection.to_string()], |r| r.get::<_, String>(0))?;
        rows.map(|row| {
            let s = row?;
            parse_typed(&s, "artifact")
        })
        .collect()
    }

    /// Record a projection's canonical key descriptor (kind/subject/scope)
    /// for its content id. Idempotent.
    ///
    /// # Errors
    /// SQLite failures.
    pub fn put_projection_descriptor(
        &mut self,
        projection: &trellis_core::projection::Projection,
    ) -> Result<(), StoreError> {
        self.transaction(|conn| {
            let variant = match projection.subject() {
                trellis_core::projection::Subject::File(_) => "File",
                trellis_core::projection::Subject::Symbol(_) => "Symbol",
                trellis_core::projection::Subject::Module(_) => "Module",
                trellis_core::projection::Subject::Text(_) => "Text",
                trellis_core::projection::Subject::ConfigKey(_) => "ConfigKey",
                trellis_core::projection::Subject::Tool(_) => "Tool",
            };
            conn.execute(
                "INSERT OR IGNORE INTO projection_descriptors
                 (id, kind, subject_variant, subject, scope)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    projection.id().to_string(),
                    projection.kind().keyword(),
                    variant,
                    projection.subject().canonical(),
                    projection.scope_name(),
                ],
            )?;
            Ok(())
        })
    }

    /// Record the source unit an observation was evaluated against
    /// (file→observation reverse index). Idempotent.
    ///
    /// # Errors
    /// SQLite failures.
    pub fn put_observation_anchor(
        &mut self,
        observation: &trellis_core::ids::ProjectionObservationId,
        source_path: &str,
    ) -> Result<(), StoreError> {
        self.transaction(|conn| {
            // Idempotent for the identical (observation, path) pair; a
            // conflicting path for the same observation is corrupt intent —
            // reject explicitly rather than silently keeping a plausible
            // wrong anchor (the file→observation index is
            // correctness-critical).
            let existing: Option<String> = conn
                .query_row(
                    "SELECT source_path FROM observation_anchors WHERE observation_id = ?1",
                    [observation.to_string()],
                    |r| r.get(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            match existing {
                Some(p) if p == source_path => Ok(()),
                Some(p) => Err(StoreError::Constraint(format!(
                    "observation {} already anchored to {p}, refusing conflicting anchor {source_path}",
                    observation
                ))),
                None => {
                    conn.execute(
                        "INSERT INTO observation_anchors (observation_id, source_path)
                         VALUES (?1, ?2)",
                        rusqlite::params![observation.to_string(), source_path],
                    )?;
                    Ok(())
                }
            }
        })
    }

    /// All persisted projection observations, sorted by id (deterministic).
    ///
    /// # Errors
    /// SQLite failures.
    pub fn all_projection_observations(
        &self,
    ) -> Result<Vec<trellis_core::projection::ProjectionObservation>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, projection, snapshot, value_digest, canonical_value
             FROM projection_observations ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (oid, proj, snap, digest, canonical) = row?;
            out.push(decode_observation_row(
                &oid,
                &proj,
                &snap,
                &digest,
                canonical.as_deref(),
            )?);
        }
        Ok(out)
    }

    /// Load one observation's descriptor. Missing row = corrupt persisted
    /// state: an observation without its key cannot be discovered and must
    /// never be silently omitted (fail closed, spec §8).
    ///
    /// # Errors
    /// [`StoreError::Corrupt`] when the descriptor row is absent;
    /// SQLite failures.
    pub fn projection_descriptor(
        &self,
        projection: &trellis_core::ids::ProjectionId,
    ) -> Result<(String, String, String, String), StoreError> {
        self.conn
            .query_row(
                "SELECT kind, subject_variant, subject, scope
                 FROM projection_descriptors WHERE id = ?1",
                [projection.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => StoreError::Corrupt(format!(
                    "observation references projection {} without a recorded descriptor",
                    projection
                )),
                other => other.into(),
            })
    }

    /// Load an observation's anchor. Missing row = corrupt persisted state
    /// (fail closed).
    ///
    /// # Errors
    /// [`StoreError::Corrupt`] when the anchor row is absent; SQLite failures.
    pub fn observation_anchor(
        &self,
        observation: &trellis_core::ids::ProjectionObservationId,
    ) -> Result<String, StoreError> {
        self.conn
            .query_row(
                "SELECT source_path FROM observation_anchors WHERE observation_id = ?1",
                [observation.to_string()],
                |r| r.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => StoreError::Corrupt(format!(
                    "observation {} has no recorded source anchor",
                    observation
                )),
                other => other.into(),
            })
    }

    /// Bind a completeness-sensitive projection's current observation
    /// context `(Q, U, C)` (spec §8.1): the snapshot it was evaluated
    /// at, universe digest, coverage digest, indexer identity, and the
    /// completeness verdict the certificate supported. One current
    /// binding per projection: re-binding at a NEW snapshot supersedes
    /// the previous one (a universe/coverage change re-binds after
    /// reevaluation); re-binding at the SAME snapshot with a different
    /// tuple is corrupt intent and rejected.
    ///
    /// # Errors
    /// [`StoreError::Constraint`] on a conflicting same-snapshot rebind;
    /// SQLite failures.
    pub fn put_completeness_binding(
        &mut self,
        projection: &trellis_core::ids::ProjectionId,
        snapshot: &trellis_core::ids::SnapshotId,
        universe_digest: &trellis_core::ids::ContentHash,
        coverage_digest: &trellis_core::ids::ContentHash,
        indexer: &str,
        completeness: trellis_core::coverage::Completeness,
    ) -> Result<(), StoreError> {
        let completeness = match completeness {
            trellis_core::coverage::Completeness::Complete => "complete",
            trellis_core::coverage::Completeness::Unknown => "unknown",
        };
        self.transaction(|conn| {
            let existing: Option<(String, String, String, String, String)> = conn
                .query_row(
                    "SELECT snapshot, universe_digest, coverage_digest, indexer, completeness
                     FROM completeness_bindings WHERE projection_id = ?1",
                    [projection.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            let new_universe = universe_digest.to_string();
            let new_coverage = coverage_digest.to_string();
            let new_snapshot = snapshot.to_string();
            match existing {
                Some((s, u, c, i, k))
                    if s == new_snapshot && u == new_universe && c == new_coverage
                        && i == indexer && k == completeness =>
                {
                    Ok(())
                }
                Some((s, _, _, _, _)) if s == new_snapshot => Err(StoreError::Constraint(
                    format!(
                        "projection {projection} already bound at snapshot {s} with a different (universe, coverage, indexer, completeness); refusing conflicting rebind"
                    ),
                )),
                _ => {
                    conn.execute(
                        "INSERT INTO completeness_bindings
                         (projection_id, snapshot, universe_digest, coverage_digest, indexer, completeness)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(projection_id) DO UPDATE SET
                           snapshot = excluded.snapshot,
                           universe_digest = excluded.universe_digest,
                           coverage_digest = excluded.coverage_digest,
                           indexer = excluded.indexer,
                           completeness = excluded.completeness",
                        rusqlite::params![
                            projection.to_string(),
                            new_snapshot,
                            new_universe,
                            new_coverage,
                            indexer,
                            completeness
                        ],
                    )?;
                    Ok(())
                }
            }
        })
    }

    /// The persisted current `(Q, U, C)` binding for a projection.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when unbound; SQLite failures.
    pub fn completeness_binding(
        &self,
        projection: &trellis_core::ids::ProjectionId,
    ) -> Result<
        (
            trellis_core::ids::SnapshotId,
            trellis_core::ids::ContentHash,
            trellis_core::ids::ContentHash,
            String,
            trellis_core::coverage::Completeness,
        ),
        StoreError,
    > {
        let (s, u, c, i, k) = self.conn.query_row(
            "SELECT snapshot, universe_digest, coverage_digest, indexer, completeness
             FROM completeness_bindings WHERE projection_id = ?1",
            [projection.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )?;
        let completeness = match k.as_str() {
            "complete" => trellis_core::coverage::Completeness::Complete,
            "unknown" => trellis_core::coverage::Completeness::Unknown,
            other => {
                return Err(StoreError::Corrupt(format!(
                    "unknown completeness verdict {other:?}"
                )))
            }
        };
        Ok((
            parse_typed(&s, "snapshot")?,
            parse_typed(&u, "universe digest")?,
            parse_typed(&c, "coverage digest")?,
            i,
            completeness,
        ))
    }
}

//! trellis-cli — thin JSON command surface over the v0.1 runtime for
//! coding-agent adapters (v0.2).
//!
//! The CLI is glue, not a new runtime: every subcommand delegates to the
//! existing engine/store/program crates. Fresh semantic queries use a
//! clearly-labeled approximate textual resolver implemented here (the
//! production SCIP path is deferred until v0.2 measurement justifies it);
//! syntactic kinds come from `trellis-program` and are proven.

#![forbid(unsafe_code)]

mod semantic;
mod state;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use trellis_core::artifact::{
    ArtifactEnvelope, ArtifactKind, CostRecord, Derivation, ProducerInfo, Proposition,
};
use trellis_core::attestation::{EvidenceRef, ValidationAttestation};
use trellis_core::coverage::{CoverageCertificate, Universe};
use trellis_core::ids::{
    ArtifactId, AttestationId, ContentHash, DerivationId, EnvironmentFingerprintId, HashAlgo,
    ManifestId, RepositoryId, SnapshotId,
};
use trellis_core::observation_id::canonical_observation_id;
use trellis_core::projection::{Projection, ProjectionKind, ProjectionObservation, Scope};
use trellis_core::snapshot::Snapshot;
use trellis_core::validity::{Authority, Validity, VerificationLevel};
use trellis_engine::prelude::*;
use trellis_engine::redgreen::{contract_verifier_id, ProjectionOutcomeKind, Transition};
use trellis_program::prelude::PythonSyntaxIndex;
use trellis_source::manifest::{build_manifest, Manifest, ManifestEntry, ManifestOptions};
use trellis_source::reconcile::{reconcile_against_working_tree, ChangedSet};
use trellis_store::prelude::Store;

use crate::semantic::CliSemanticSource;
use crate::state::{now_ms, snapshot_id_for, CliState};

const INDEXER: &str = "trellis-cli-textual-v1";
const PRODUCER: &str = "trellis-cli";
const SET_EQUALITY: &str = trellis_engine::redgreen::SET_EQUALITY_CONTRACT;

type CliResult<T> = Result<T, String>;

fn digest_str(s: &str) -> ContentHash {
    ContentHash::compute(HashAlgo::Blake3, s.as_bytes())
}

fn artifact_key_id(key: &str, created: u64) -> ArtifactId {
    // Per-publication identity: the store is append-only (no artifact
    // mutation path), so each republish of a key writes a new artifact;
    // the CLI sidecar resolves a key to its latest artifact id.
    ArtifactId::from_hash(digest_str(&format!("trellis.cli.artifact:{key}:{created}")))
}

fn pin_kind(projection: &Projection) -> &'static str {
    match projection.kind() {
        ProjectionKind::Callers => "callers",
        ProjectionKind::FileContent => "file",
        ProjectionKind::Imports => "imports",
        ProjectionKind::Definition => "definition",
        ProjectionKind::Signature => "signature",
        _ => "other",
    }
}

fn pin_scope(projection: &Projection) -> String {
    match projection.scope() {
        Scope::File => "File".to_string(),
        Scope::Module => "Module".to_string(),
        Scope::Package => "Package".to_string(),
        Scope::Repository => "Repository".to_string(),
    }
}

fn pin_projection(kind: &str, subject: &str, scope: &str) -> CliResult<Projection> {
    match (kind, scope) {
        ("callers", "Repository") => {
            Projection::callers(subject, Scope::Repository).map_err(|e| e.to_string())
        }
        ("file", _) => Projection::file(subject).map_err(|e| e.to_string()),
        ("imports", "Module") => {
            Projection::imports(subject, Scope::Module).map_err(|e| e.to_string())
        }
        ("definition", _) => Projection::definition(subject).map_err(|e| e.to_string()),
        ("signature", _) => Projection::signature(subject).map_err(|e| e.to_string()),
        (other, _) => Err(format!("unknown pin kind {other}")),
    }
}

fn put_snapshot_row(store: &mut Store, id: SnapshotId, parent: Option<SnapshotId>, at: u64) {
    let manifest = ManifestId::from_hash(digest_str(&format!("manifest:{id}")));
    let row = Snapshot::new(
        id,
        RepositoryId::from_hash(digest_str("trellis.cli.repo")),
        None,
        manifest,
        None,
        EnvironmentFingerprintId::from_hash(digest_str("trellis.cli.env")),
        parent,
        at,
    )
    .expect("snapshot valid");
    store.put_snapshot(&row).expect("snapshot persisted");
}

/// Coverage context for the CLI's own textual backend. The resolver
/// scans every tracked .py file (universe = full walk; read failures
/// abort hard), so its capabilities are Established within the textual
/// resolution class; answers still carry Provisional authority (see
/// CliSemanticSource) and the approximate backend label travels with
/// every value.
fn coverage_for(tree: &BTreeMap<String, String>) -> CoverageContext {
    let paths: Vec<String> = tree.keys().cloned().collect();
    CoverageContext {
        universe: Universe::new(paths),
        certificate: CoverageCertificate::complete(),
        indexer: INDEXER.to_string(),
    }
}

fn manifest_options(repo: &Path) -> ManifestOptions {
    let mut options = ManifestOptions::new(repo);
    options.exclude_dirs.push(".opencode".to_string());
    options
}

fn manifest_from_state(cli: &CliState) -> Option<Manifest> {
    let entries: Vec<ManifestEntry> = cli
        .manifest_entries
        .iter()
        .filter_map(|(p, d)| {
            Some(ManifestEntry {
                path: p.clone(),
                digest: d.parse().ok()?,
            })
        })
        .collect();
    Manifest::from_entries(entries).ok()
}

/// Reconcile the working tree against the persisted manifest, then
/// advance persisted state (snapshot row + manifest sidecar) to the
/// current tree. Returns (changed set, current tree).
fn reconcile_current(
    repo: &Path,
    store: &mut Store,
    cli: &mut CliState,
) -> CliResult<(ChangedSet, BTreeMap<String, String>)> {
    let changed = if let Some(stored) = manifest_from_state(cli) {
        reconcile_against_working_tree(&stored, &manifest_options(repo))
            .map_err(|e| e.to_string())?
    } else {
        // No persisted manifest (first run after init): treat the whole
        // tree as changed — every projection reevaluates once.
        let current = build_manifest(&manifest_options(repo)).map_err(|e| e.to_string())?;
        ChangedSet {
            added: current.entries().iter().map(|e| e.path.clone()).collect(),
            modified: Vec::new(),
            removed: Vec::new(),
        }
    };
    let tree = state::read_tree(repo)?;
    let snapshot = snapshot_id_for(&tree);
    let parent = cli
        .head
        .as_deref()
        .and_then(|h| h.parse::<SnapshotId>().ok())
        .filter(|p| *p != snapshot);
    put_snapshot_row(store, snapshot, parent, now_ms());
    cli.head = Some(snapshot.to_string());
    let manifest = build_manifest(&manifest_options(repo)).map_err(|e| e.to_string())?;
    cli.manifest_entries = manifest
        .entries()
        .iter()
        .map(|e| (e.path.clone(), e.digest.to_string()))
        .collect();
    Ok((changed, tree))
}

// ─────────────────────────────────────────────────────────────────────
// Subcommands
// ─────────────────────────────────────────────────────────────────────

fn cmd_init(repo: PathBuf, store_path: PathBuf) -> CliResult<serde_json::Value> {
    let tree = state::read_tree(&repo)?;
    let mut store = Store::open(&store_path).map_err(|e| e.to_string())?;
    let snapshot = snapshot_id_for(&tree);
    put_snapshot_row(&mut store, snapshot, None, now_ms());
    let manifest = build_manifest(&manifest_options(&repo)).map_err(|e| e.to_string())?;
    let cli = CliState {
        head: Some(snapshot.to_string()),
        manifest_entries: manifest
            .entries()
            .iter()
            .map(|e| (e.path.clone(), e.digest.to_string()))
            .collect(),
        artifacts: BTreeMap::new(),
    };
    cli.save(&store_path)?;
    Ok(serde_json::json!({
        "ok": true,
        "files_indexed": tree.len(),
        "snapshot": snapshot.to_string(),
        "store": store_path.display().to_string(),
    }))
}

fn cmd_status(repo: PathBuf, store_path: PathBuf) -> CliResult<serde_json::Value> {
    let start = Instant::now();
    let mut store = Store::open(&store_path).map_err(|e| e.to_string())?;
    let mut cli = CliState::load(&store_path);
    let (changed, tree) = reconcile_current(&repo, &mut store, &mut cli)?;
    let changed_json = serde_json::json!({
        "added": changed.added,
        "modified": changed.modified,
        "removed": changed.removed,
    });
    let index = PythonSyntaxIndex::index(&tree);
    let source = CliSemanticSource::new(&index, &tree);
    let snapshot = snapshot_id_for(&tree);
    let transition = Transition::new(&mut store, &source, changed, snapshot, now_ms())
        .with_coverage(coverage_for(&tree));
    let report = transition.run().map_err(|e| e.to_string())?;

    let artifacts: Vec<serde_json::Value> = cli
        .artifacts
        .iter()
        .map(|(key, entry)| {
            let validity = entry
                .id
                .parse::<ArtifactId>()
                .ok()
                .and_then(|id| {
                    store
                        .attestation_history(&id)
                        .ok()
                        .and_then(|h| h.iter().last().map(|a| a.validity()))
                })
                .map(|v| format!("{v:?}"))
                .unwrap_or_else(|| "None".to_string());
            serde_json::json!({"key": key, "kind": entry.kind, "id": entry.id, "validity": validity})
        })
        .collect();
    cli.save(&store_path)?;
    let elapsed = start.elapsed().as_micros() as u64;
    state::emit_event(&store_path, "status", None, None, "mixed", elapsed);
    Ok(serde_json::json!({
        "ok": true,
        "changed": changed_json,
        "projections_reevaluated": report.projections().len(),
        "artifacts": artifacts,
    }))
}

/// Dependency projections per artifact kind.
fn dependency_projections(
    kind: &str,
    key: &str,
    deps: &[String],
    tree: &BTreeMap<String, String>,
) -> CliResult<Vec<Projection>> {
    match kind {
        // `references` resolves through the same approximated caller set
        // as `callers` (the labeled textual resolver), so the projection
        // and its validity semantics are shared.
        "callers" | "references" => Ok(vec![
            Projection::callers(key, Scope::Repository).map_err(|e| e.to_string())?
        ]),
        "definitions" => Ok(vec![Projection::definition(key).map_err(|e| e.to_string())?]),
        "imports" => Ok(vec![
            Projection::imports(key, Scope::Module).map_err(|e| e.to_string())?
        ]),
        "filemap" => {
            let path = state::module_path(key);
            if !tree.contains_key(&path) {
                return Err(format!(
                    "filemap key must be a dotted module name present in the tree (e.g. auth.tokens); '{key}' resolves to missing {path}"
                ));
            }
            Ok(vec![
                Projection::file(state::module_path(key).as_str()).map_err(|e| e.to_string())?
            ])
        }
        "notes" => {
            let mut out = Vec::new();
            for dep in deps {
                let path = state::module_path(dep);
                if !tree.contains_key(&path) {
                    return Err(format!(
                        "notes dep must be a dotted module name present in the tree (e.g. users.service); '{dep}' resolves to missing {path}"
                    ));
                }
                out.push(Projection::file(&path).map_err(|e| e.to_string())?);
            }
            if out.is_empty() {
                return Err(
                    "notes artifacts require at least one --dep FILE (dotted module names the note depends on)"
                        .to_string(),
                );
            }
            Ok(out)
        }
        other => Err(format!("unknown artifact kind {other}")),
    }
}

/// Anchor path for an observation record: the file whose content the
/// projection depends on.
fn anchor_for(projection: &Projection, tree: &BTreeMap<String, String>) -> CliResult<String> {
    match projection.kind() {
        ProjectionKind::FileContent => Ok(projection.subject().canonical().to_string()),
        ProjectionKind::Imports => Ok(state::module_path(projection.subject().canonical())),
        ProjectionKind::Definition => {
            // Definition projections are keyed by symbol OR module
            // subject; a module subject anchors at its own file.
            let subject = projection.subject().canonical();
            if tree.contains_key(state::module_path(subject).as_str()) {
                Ok(state::module_path(subject))
            } else {
                let parts: Vec<&str> = subject.split('.').collect();
                (1..parts.len())
                    .rev()
                    .map(|n| state::module_path(&parts[..n].join(".")))
                    .find(|p| tree.contains_key(p))
                    .ok_or_else(|| format!("definition site of {subject} not found in tree"))
            }
        }
        ProjectionKind::Callers | ProjectionKind::References | ProjectionKind::Signature => {
            let subject = projection.subject().canonical();
            let parts: Vec<&str> = subject.split('.').collect();
            (1..parts.len())
                .rev()
                .map(|n| state::module_path(&parts[..n].join(".")))
                .find(|p| tree.contains_key(p))
                .ok_or_else(|| format!("definition site of {subject} not found in tree"))
        }
        other => Err(format!("no anchor rule for kind {other:?}")),
    }
}

fn cmd_publish(
    repo: PathBuf,
    store_path: PathBuf,
    key: &str,
    kind: &str,
    value: &str,
    deps: &[String],
) -> CliResult<serde_json::Value> {
    let start = Instant::now();
    let mut store = Store::open(&store_path).map_err(|e| e.to_string())?;
    let mut cli = CliState::load(&store_path);
    // Advance persisted state to the current tree first: the task that
    // produced the artifact already edited files, so publish's snapshot
    // id must exist (observations/envelope/attestation all FK to it).
    let (_, tree) = reconcile_current(&repo, &mut store, &mut cli)?;
    let index = PythonSyntaxIndex::index(&tree);
    let source = CliSemanticSource::new(&index, &tree);
    let snapshot = snapshot_id_for(&tree);

    let projections = dependency_projections(kind, key, deps, &tree)?;

    // Seed dependency observations + completeness bindings at the
    // current snapshot (the engine harness's seed pattern, in product
    // code).
    let mut dep_ids = Vec::new();
    let mut evidence = Vec::new();
    let mut pins: Vec<(String, String, String, String)> = Vec::new();
    for projection in &projections {
        let evaluated = source
            .evaluate(projection)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("source cannot evaluate {:?}", projection.kind()))?;
        let digest = digest_str(evaluated.value());
        pins.push((
            pin_kind(projection).to_string(),
            projection.subject().canonical().to_string(),
            pin_scope(projection),
            digest.to_string(),
        ));
        let digest = digest_str(evaluated.value());
        let obs = ProjectionObservation::new(
            canonical_observation_id(&projection.id(), &digest, &snapshot),
            projection.id(),
            snapshot,
            digest,
            None,
        );
        let anchor = anchor_for(projection, &tree)?;
        store
            .record_observation_record(projection, &obs, &anchor)
            .map_err(|e| e.to_string())?;
        // Completeness bindings are established by the first transition
        // (run() records (Q, U, C) when it reevaluates), so the publish
        // path never writes a conflicting binding.
        evidence.push(EvidenceRef::Observation(obs.id()));
        dep_ids.push(projection.id());
    }

    // Envelope: payload pins the dependency digests the value was
    // derived from, plus the agent-readable value. The engine's
    // set-equality contract re-establishes continuity after a settled
    // change; the pins let retrieve refuse to serve a payload that no
    // longer matches the reevaluated dependency values (false-valid
    // stale-reuse guard at the adapter boundary).
    let created = now_ms();
    let art_id = artifact_key_id(key, created);
    let blob_payload = serde_json::json!({
        "pins": pins.iter()
            .map(|(k, s, sc, d)| serde_json::json!([k, s, sc, d]))
            .collect::<Vec<_>>(),
        "value": value,
    });
    let blob = store
        .put_blob(blob_payload.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    let proposition =
        Proposition::new(format!("artifact {key} is a reusable agent-derived {kind}"))
            .map_err(|e| e.to_string())?;
    let derivation = Derivation::new(
        DerivationId::from_hash(digest_str(&format!(
            "trellis.cli.derivation:{key}:{created}"
        ))),
        vec![trellis_core::validity::CaptureChannel::TrellisTool],
        ProducerInfo::new(PRODUCER, "0.2.0", None).map_err(|e| e.to_string())?,
        created,
    );
    let envelope = ArtifactEnvelope::new(
        art_id,
        1,
        ArtifactKind::StructuralSet,
        blob,
        Some(proposition),
        ProducerInfo::new(PRODUCER, "0.2.0", None).map_err(|e| e.to_string())?,
        derivation.clone(),
        dep_ids.clone(),
        snapshot,
        CostRecord::default(),
    )
    .map_err(|e| e.to_string())?;
    store.put_artifact(&envelope).map_err(|e| e.to_string())?;

    // Seed attestation: VALID at publish time under the set-equality
    // verifier, so the red/green contract re-establishes validity when
    // dependency values hold and breaks when they change.
    let att = ValidationAttestation::for_derivation(
        AttestationId::from_hash(digest_str(&format!("trellis.cli.seed:{key}:{created}"))),
        art_id,
        &derivation,
        snapshot,
        None,
        Validity::Valid,
        Authority::Authoritative,
        VerificationLevel::Structural,
        Some(contract_verifier_id(SET_EQUALITY)),
        evidence,
        created,
    );
    store.append_attestation(&att).map_err(|e| e.to_string())?;

    cli.artifacts.insert(
        key.to_string(),
        state::ArtifactEntry {
            id: art_id.to_string(),
            kind: kind.to_string(),
        },
    );
    cli.save(&store_path)?;

    let elapsed = start.elapsed().as_micros() as u64;
    state::emit_event(
        &store_path,
        "publish",
        Some(key),
        None,
        "trellis-engine",
        elapsed,
    );
    Ok(serde_json::json!({
        "ok": true,
        "key": key,
        "artifact": art_id.to_string(),
        "kind": kind,
        "dependencies": dep_ids.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
    }))
}

fn cmd_retrieve(repo: PathBuf, store_path: PathBuf, key: &str) -> CliResult<serde_json::Value> {
    let start = Instant::now();
    let mut store = Store::open(&store_path).map_err(|e| e.to_string())?;
    let mut cli = CliState::load(&store_path);
    let Some(entry) = cli.artifacts.get(key).cloned() else {
        let elapsed = start.elapsed().as_micros() as u64;
        state::emit_event(&store_path, "retrieve", Some(key), None, "unknown", elapsed);
        return Ok(serde_json::json!({
            "ok": true, "key": key, "verdict": "unknown",
            "explanation": "no artifact published under this key",
        }));
    };
    let art_id: ArtifactId = entry
        .id
        .parse()
        .map_err(|e| format!("stored artifact id for {key} unparseable: {e}"))?;

    let (changed, tree) = reconcile_current(&repo, &mut store, &mut cli)?;
    let index = PythonSyntaxIndex::index(&tree);
    let source = CliSemanticSource::new(&index, &tree);
    let snapshot = snapshot_id_for(&tree);
    let transition = Transition::new(&mut store, &source, changed, snapshot, now_ms())
        .with_coverage(coverage_for(&tree));
    let report = transition.run().map_err(|e| e.to_string())?;

    let validity = store
        .attestation_history(&art_id)
        .map_err(|e| e.to_string())?
        .iter()
        .last()
        .map(|a| a.validity());
    let verdict = match validity {
        Some(Validity::Valid) => "valid",
        Some(Validity::Stale) => "stale",
        _ => "unknown",
    };
    let mut explanation = String::new();
    if verdict != "valid" {
        let artifact = store.get_artifact(&art_id).map_err(|e| e.to_string())?;
        let parts: Vec<String> = report
            .projections()
            .iter()
            .filter(|o| artifact.dependencies().contains(&o.projection().id()))
            .map(|o| {
                let kind = format!("{:?}", o.projection().kind());
                let subject = o.projection().subject().canonical().to_string();
                match (o.outcome(), o.new_value()) {
                    (ProjectionOutcomeKind::Changed, Some(v)) => {
                        format!("{kind}({subject}) changed to [{v}]")
                    }
                    (ProjectionOutcomeKind::Unobserved, _) => {
                        format!("{kind}({subject}) unobserved under current coverage")
                    }
                    _ => format!("{kind}({subject}) unchanged"),
                }
            })
            .collect();
        explanation = if parts.is_empty() {
            "no dependency evidence this round; standing is conservative UNKNOWN".to_string()
        } else {
            parts.join("; ")
        };
    }

    cli.save(&store_path)?;
    let elapsed = start.elapsed().as_micros() as u64;
    state::emit_event(&store_path, "retrieve", Some(key), None, verdict, elapsed);
    let mut out = serde_json::json!({
        "ok": true,
        "key": key,
        "verdict": verdict,
        "explanation": explanation,
    });
    // The value leaves the store ONLY under a Valid attestation whose
    // payload pins still match the reevaluated dependency digests
    // (§15: never silently inject stale/unknown artifacts as trusted
    // knowledge). A settled post-change Valid with stale pins is
    // refused as unknown — the payload no longer describes the current
    // dependency values.
    if verdict == "valid" {
        let artifact = store.get_artifact(&art_id).map_err(|e| e.to_string())?;
        let bytes = store
            .get_blob(&artifact.payload_ref())
            .map_err(|e| e.to_string())?;
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("payload corrupt: {e}"))?;
        let pins = payload["pins"]
            .as_array()
            .ok_or("payload missing pins")?
            .iter()
            .filter_map(|p| {
                Some((
                    p[0].as_str()?.to_string(),
                    p[1].as_str()?.to_string(),
                    p[2].as_str()?.to_string(),
                    p[3].as_str()?.to_string(),
                ))
            })
            .collect::<Vec<(String, String, String, String)>>();
        let mut pins_current = true;
        for (kind, subject, scope, _pinned_digest) in &pins {
            let projection = pin_projection(kind, subject, scope)?;
            let current = source
                .evaluate(&projection)
                .map_err(|e| e.to_string())?
                .map(|ev| digest_str(ev.value()).to_string())
                .unwrap_or_default();
            let pinned = pins
                .iter()
                .find(|(k, s, sc, _)| k == kind && s == subject && sc == scope)
                .map(|(_, _, _, d)| d.clone())
                .unwrap_or_default();
            if pinned != current {
                pins_current = false;
            }
        }
        if pins_current {
            out["value"] = serde_json::Value::String(
                payload["value"].as_str().unwrap_or_default().to_string(),
            );
        } else {
            out["verdict"] = serde_json::Value::String("unknown".to_string());
            out["explanation"] = serde_json::Value::String(
                "artifact payload pins no longer match current dependency values; recompute required"
                    .to_string(),
            );
        }
    }
    Ok(out)
}

fn cmd_query(repo: PathBuf, kind: &str, subject: &str) -> CliResult<serde_json::Value> {
    let start = Instant::now();
    let tree = state::read_tree(&repo)?;
    let index = PythonSyntaxIndex::index(&tree);
    let out;
    let (backend, event_path) = match kind {
        "callers" | "references" => {
            // Approximate textual resolver, clearly labeled (spec §8: a
            // syntax-only backend must not claim proven semantic facts).
            let value = CliSemanticSource::new(&index, &tree).approx_callers(subject);
            let path = repo.join(".trellis-events.jsonl");
            out = serde_json::json!({
                "ok": true, "kind": kind, "subject": subject,
                "backend": semantic::APPROX_BACKEND, "value": value,
            });
            (semantic::APPROX_BACKEND, path)
        }
        "definitions" | "imports" => {
            let projection = match kind {
                "definitions" => Projection::definition(subject).map_err(|e| e.to_string())?,
                _ => Projection::imports(subject, Scope::Module).map_err(|e| e.to_string())?,
            };
            let evaluated = CliSemanticSource::new(&index, &tree)
                .evaluate(&projection)
                .map_err(|e| e.to_string())?;
            let path = repo.join(".trellis-events.jsonl");
            out = match evaluated {
                Some(ev) => serde_json::json!({
                    "ok": true, "kind": kind, "subject": subject,
                    "backend": semantic::SYNTAX_BACKEND, "value": ev.value(),
                }),
                None => serde_json::json!({
                    "ok": true, "kind": kind, "subject": subject,
                    "backend": semantic::SYNTAX_BACKEND, "unsupported": true,
                }),
            };
            (semantic::SYNTAX_BACKEND, path)
        }
        other => return Err(format!("unknown query kind {other}")),
    };
    state::emit_event_at(
        &event_path,
        "query",
        None,
        Some(subject),
        backend,
        start.elapsed().as_micros() as u64,
    );
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────
// Transparent deterministic tool reuse (v0.2 ablation)
// ─────────────────────────────────────────────────────────────────────

/// The identical computation runs in both conditions; only whether a
/// validated artifact may serve it first differs.
fn compute_fresh(
    kind: &str,
    subject: &str,
    tree: &BTreeMap<String, String>,
    index: &PythonSyntaxIndex,
) -> CliResult<String> {
    match kind {
        "callers" => Ok(CliSemanticSource::new(index, tree).approx_callers(subject)),
        "references" => Ok(CliSemanticSource::new(index, tree).approx_callers(subject)),
        "definitions" => {
            let projection = Projection::definition(subject).map_err(|e| e.to_string())?;
            Ok(CliSemanticSource::new(index, tree)
                .evaluate(&projection)
                .map_err(|e| e.to_string())?
                .map(|ev| ev.value().to_string())
                .unwrap_or_default())
        }
        "imports" => {
            let projection =
                Projection::imports(subject, Scope::Module).map_err(|e| e.to_string())?;
            Ok(CliSemanticSource::new(index, tree)
                .evaluate(&projection)
                .map_err(|e| e.to_string())?
                .map(|ev| ev.value().to_string())
                .unwrap_or_default())
        }
        other => Err(format!("unknown tool kind {other}")),
    }
}

/// Baseline serves fresh; trellis serves stored only when the engine
/// verdict is Valid AND payload pins match reevaluated dependencies,
/// else computes fresh and auto-captures. Response shape is identical
/// in both conditions except the instrumented `served_from` field.
fn cmd_tool(
    repo: PathBuf,
    store_path: PathBuf,
    mode: &str,
    kind: &str,
    subject: &str,
) -> CliResult<serde_json::Value> {
    let start = Instant::now();
    let tree = state::read_tree(&repo)?;
    let index = PythonSyntaxIndex::index(&tree);
    let fresh_value = compute_fresh(kind, subject, &tree, &index)?;

    let (served_from, value) = if mode == "baseline" {
        ("fresh", fresh_value.clone())
    } else {
        let mut store = Store::open(&store_path).map_err(|e| e.to_string())?;
        let mut cli = CliState::load(&store_path);
        let (changed, tree) = reconcile_current(&repo, &mut store, &mut cli)?;
        let index = PythonSyntaxIndex::index(&tree);
        let source = CliSemanticSource::new(&index, &tree);
        let snapshot = snapshot_id_for(&tree);
        let transition = Transition::new(&mut store, &source, changed, snapshot, now_ms())
            .with_coverage(coverage_for(&tree));
        let _report = transition.run().map_err(|e| e.to_string())?;

        // Look for any artifact whose dependencies cover this query and
        // whose payload pins still match. Validity comes from the engine
        // transition above; pins close the settled-change gap.
        let mut served: Option<String> = None;
        for entry in cli.artifacts.values() {
            if entry.kind != kind && !(kind == "references" && entry.kind == "callers") {
                continue;
            }
            let Ok(art_id) = entry.id.parse::<ArtifactId>() else {
                continue;
            };
            let Ok(artifact) = store.get_artifact(&art_id) else {
                continue;
            };
            let validity = store
                .attestation_history(&art_id)
                .ok()
                .and_then(|h| h.iter().last().map(|a| a.validity()));
            if validity != Some(Validity::Valid) {
                continue;
            }
            let Ok(bytes) = store.get_blob(&artifact.payload_ref()) else {
                continue;
            };
            let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                continue;
            };
            let pins = match payload["pins"].as_array() {
                Some(pins) => pins
                    .iter()
                    .filter_map(|p| {
                        Some((
                            p[0].as_str()?.to_string(),
                            p[1].as_str()?.to_string(),
                            p[2].as_str()?.to_string(),
                            p[3].as_str()?.to_string(),
                        ))
                    })
                    .collect::<Vec<(String, String, String, String)>>(),
                None => continue,
            };
            let pin_kind_expected = match kind {
                "callers" | "references" => "callers",
                "definitions" => "definition",
                "imports" => "imports",
                other => other,
            };
            let covers_query = pins
                .iter()
                .any(|(k, s, _, _)| k == pin_kind_expected && s == subject);
            if !covers_query {
                continue;
            }
            let mut pins_current = true;
            for (k, s, sc, pinned_digest) in &pins {
                let Ok(projection) = pin_projection(k, s, sc) else {
                    pins_current = false;
                    break;
                };
                let current = source
                    .evaluate(&projection)
                    .map_err(|e| e.to_string())?
                    .map(|ev| digest_str(ev.value()).to_string())
                    .unwrap_or_default();
                if pinned_digest != &current {
                    pins_current = false;
                    break;
                }
            }
            if pins_current {
                served = Some(payload["value"].as_str().unwrap_or_default().to_string());
                break;
            }
        }

        match served {
            Some(v) => ("reuse", v),
            None => {
                // Auto-capture: the identical machinery as explicit
                // publish, driven by the tool call itself. Key names the
                // deterministic query; value is the fresh computation.
                let created = now_ms();
                let key = format!("{kind}:{subject}");
                let projections = dependency_projections(kind, subject, &[], &tree)?;
                let mut dep_ids = Vec::new();
                let mut evidence = Vec::new();
                let mut pins: Vec<(String, String, String, String)> = Vec::new();
                for projection in &projections {
                    let evaluated = source
                        .evaluate(projection)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("source cannot evaluate {:?}", projection.kind()))?;
                    let digest = digest_str(evaluated.value());
                    pins.push((
                        pin_kind(projection).to_string(),
                        projection.subject().canonical().to_string(),
                        pin_scope(projection),
                        digest.to_string(),
                    ));
                    let obs = ProjectionObservation::new(
                        canonical_observation_id(&projection.id(), &digest, &snapshot),
                        projection.id(),
                        snapshot,
                        digest,
                        None,
                    );
                    let anchor = anchor_for(projection, &tree)?;
                    store
                        .record_observation_record(projection, &obs, &anchor)
                        .map_err(|e| e.to_string())?;
                    evidence.push(EvidenceRef::Observation(obs.id()));
                    dep_ids.push(projection.id());
                }
                let art_id = artifact_key_id(&key, created);
                let blob_payload = serde_json::json!({
                    "pins": pins.iter()
                        .map(|(k, s, sc, d)| serde_json::json!([k, s, sc, d]))
                        .collect::<Vec<_>>(),
                    "value": fresh_value,
                });
                let blob = store
                    .put_blob(blob_payload.to_string().as_bytes())
                    .map_err(|e| e.to_string())?;
                let proposition =
                    Proposition::new(format!("deterministic tool result for {kind}({subject})"))
                        .map_err(|e| e.to_string())?;
                let derivation = Derivation::new(
                    DerivationId::from_hash(digest_str(&format!(
                        "trellis.cli.derivation:{key}:{created}"
                    ))),
                    vec![trellis_core::validity::CaptureChannel::TrellisTool],
                    ProducerInfo::new(PRODUCER, "0.2.0", None).map_err(|e| e.to_string())?,
                    created,
                );
                let envelope = ArtifactEnvelope::new(
                    art_id,
                    1,
                    ArtifactKind::StructuralSet,
                    blob,
                    Some(proposition),
                    ProducerInfo::new(PRODUCER, "0.2.0", None).map_err(|e| e.to_string())?,
                    derivation.clone(),
                    dep_ids,
                    snapshot,
                    CostRecord::default(),
                )
                .map_err(|e| e.to_string())?;
                store.put_artifact(&envelope).map_err(|e| e.to_string())?;
                let att = ValidationAttestation::for_derivation(
                    AttestationId::from_hash(digest_str(&format!(
                        "trellis.cli.seed:{key}:{created}"
                    ))),
                    art_id,
                    &derivation,
                    snapshot,
                    None,
                    Validity::Valid,
                    Authority::Authoritative,
                    VerificationLevel::Structural,
                    Some(contract_verifier_id(SET_EQUALITY)),
                    evidence,
                    created,
                );
                store.append_attestation(&att).map_err(|e| e.to_string())?;
                cli.artifacts.insert(
                    key.clone(),
                    state::ArtifactEntry {
                        id: art_id.to_string(),
                        kind: kind.to_string(),
                    },
                );
                cli.save(&store_path)?;
                ("fresh+captured", fresh_value)
            }
        }
    };

    let event_path = repo.join(".trellis-events.jsonl");
    state::emit_event_at(
        &event_path,
        "tool",
        None,
        Some(&format!("{kind}:{subject}")),
        served_from,
        start.elapsed().as_micros() as u64,
    );
    Ok(serde_json::json!({
        "ok": true, "kind": kind, "subject": subject,
        "backend": semantic::APPROX_BACKEND,
        "value": value,
        "served_from": served_from,
    }))
}

// ─────────────────────────────────────────────────────────────────────
// Argument parsing (hand-rolled; no new dependency beyond serde_json)
// ─────────────────────────────────────────────────────────────────────

struct Args {
    command: String,
    repo: Option<PathBuf>,
    store: Option<PathBuf>,
    key: Option<String>,
    kind: Option<String>,
    value: Option<String>,
    subject: Option<String>,
    mode: Option<String>,
    deps: Vec<String>,
}

fn parse_args() -> CliResult<Args> {
    let mut it = std::env::args().skip(1);
    let command = it.next().ok_or("usage: trellis <command> [options]")?;
    let mut args = Args {
        command,
        repo: None,
        store: None,
        key: None,
        kind: None,
        value: None,
        subject: None,
        mode: None,
        deps: Vec::new(),
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--repo" => args.repo = Some(it.next().ok_or("missing value for --repo")?.into()),
            "--store" => args.store = Some(it.next().ok_or("missing value for --store")?.into()),
            "--key" => args.key = Some(it.next().ok_or("missing value for --key")?),
            "--kind" => args.kind = Some(it.next().ok_or("missing value for --kind")?),
            "--value" => args.value = Some(it.next().ok_or("missing value for --value")?),
            "--subject" => args.subject = Some(it.next().ok_or("missing value for --subject")?),
            "--mode" => args.mode = Some(it.next().ok_or("missing value for --mode")?),
            "--dep" => args.deps.push(it.next().ok_or("missing value for --dep")?),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(args)
}

fn require<T>(v: Option<T>, what: &str) -> CliResult<T> {
    v.ok_or_else(|| format!("missing required option {what}"))
}

fn run(args: Args) -> CliResult<serde_json::Value> {
    match args.command.as_str() {
        "init" => cmd_init(
            require(args.repo, "--repo")?,
            require(args.store, "--store")?,
        ),
        "status" => cmd_status(
            require(args.repo, "--repo")?,
            require(args.store, "--store")?,
        ),
        "publish" => cmd_publish(
            require(args.repo, "--repo")?,
            require(args.store, "--store")?,
            &require(args.key, "--key")?,
            &require(args.kind, "--kind")?,
            &require(args.value, "--value")?,
            &args.deps,
        ),
        "retrieve" => cmd_retrieve(
            require(args.repo, "--repo")?,
            require(args.store, "--store")?,
            &require(args.key, "--key")?,
        ),
        "query" => cmd_query(
            require(args.repo, "--repo")?,
            &require(args.kind, "--kind")?,
            &require(args.subject, "--subject")?,
        ),
        "tool" => cmd_tool(
            require(args.repo, "--repo")?,
            args.store.unwrap_or_else(|| PathBuf::from("/dev/null")),
            require(args.mode, "--mode")?.as_str(),
            &require(args.kind, "--kind")?,
            &require(args.subject, "--subject")?,
        ),
        other => Err(format!("unknown command {other}")),
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("trellis: {e}");
            std::process::exit(2);
        }
    };
    match run(args) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("trellis: {e}");
            std::process::exit(1);
        }
    }
}

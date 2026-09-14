//! Shared M6/M7/M10 test+benchmark harness: fixture tree management,
//! catalog parsing, seeding, transitions, catalog sweep helpers.
//! Extracted verbatim from the redgreen integration suite (M10: shared
//! with the benchmark harness; no behavior change).

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tempfile::TempDir;

use trellis_core::artifact::{
    ArtifactEnvelope, ArtifactKind, CostRecord, Derivation, ProducerInfo, Proposition,
};
use trellis_core::attestation::{EvidenceRef, ValidationAttestation};
use trellis_core::coverage::{Completeness, CoverageCertificate, Universe};
use trellis_core::ids::{
    ArtifactId, AttestationId, ContentHash, DerivationId, EnvironmentFingerprintId, HashAlgo,
    ManifestId, ProjectionObservationId, RepositoryId, SnapshotId, Timestamp,
};
use trellis_core::observation_id::canonical_observation_id;
use trellis_core::projection::{Projection, ProjectionKind, ProjectionObservation, Scope};
use trellis_core::snapshot::Snapshot;
use trellis_core::validity::{Authority, Validity, VerificationLevel};

use trellis_engine::prelude::*;
use trellis_engine::redgreen::{
    contract_verifier_id, ProjectionOutcome, ReevaluationSource, SyntacticSource, Transition,
    ABSENCE_FACT_CONTRACT, SET_EQUALITY_CONTRACT,
};
use trellis_oracle::{Catalog, Mutation, Op};
use trellis_program::prelude::PythonSyntaxIndex;
use trellis_source::manifest::{build_manifest, ManifestOptions};
use trellis_source::reconcile::reconcile_against_working_tree;
use trellis_store::prelude::Store;

pub const BASE_TIMESTAMP: Timestamp = 1_700_000_000_000;
pub const TIMESTAMP_STEP: Timestamp = 1_000;
pub const FIXTURE_INDEXER: &str = "fixture-semantic-v1";

pub fn digest_of(bytes: &[u8]) -> ContentHash {
    ContentHash::compute(HashAlgo::Blake3, bytes)
}

pub fn digest_str(s: &str) -> ContentHash {
    digest_of(s.as_bytes())
}

// ─────────────────────────────────────────────────────────────────────
// Fixture tree management
// ─────────────────────────────────────────────────────────────────────

pub fn pristine_tree() -> BTreeMap<String, String> {
    let root = trellis_oracle::fixture_base();
    let mut tree = BTreeMap::new();
    for file in trellis_oracle::walk_files(&root) {
        let rel = file
            .strip_prefix(&root)
            .expect("fixture file under root")
            .to_string_lossy()
            .replace('\\', "/");
        let content = std::fs::read_to_string(&file).expect("fixture file readable");
        tree.insert(rel, content);
    }
    tree
}

pub fn write_tree(dir: &Path, tree: &BTreeMap<String, String>) {
    for (path, content) in tree {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent dirs creatable");
        }
        std::fs::write(&target, content).expect("file writable");
    }
}

pub fn read_tree_from(dir: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).expect("dir readable") {
            let entry = entry.expect("entry readable");
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if path.is_dir() {
                if name == "__pycache__" {
                    continue;
                }
                walk(&path, &rel, out);
            } else if name.ends_with(".py") {
                out.insert(
                    rel,
                    std::fs::read_to_string(&path).expect("source readable"),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

pub fn apply_ops(dir: &Path, ops: &[Op]) {
    for op in ops {
        let target = dir.join(&op.path);
        match op.op.as_str() {
            "add_file" | "replace_file" => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).expect("parent dirs creatable");
                }
                std::fs::write(&target, op.content.as_deref().unwrap_or_default())
                    .expect("mutation file writable");
            }
            "delete_file" => {
                std::fs::remove_file(&target).expect("mutation file removable");
            }
            other => panic!("unknown op {other}"),
        }
    }
}

pub fn tree_digest(tree: &BTreeMap<String, String>) -> ContentHash {
    let mut canonical = String::new();
    for (path, content) in tree {
        canonical.push_str(path);
        canonical.push('\0');
        canonical.push_str(content);
        canonical.push('\n');
    }
    digest_str(&canonical)
}

// ─────────────────────────────────────────────────────────────────────
// Fixture semantic source (test-support only; never reads catalog.json)
// ─────────────────────────────────────────────────────────────────────

pub fn module_name(path: &str) -> String {
    let stem = path.strip_suffix(".py").unwrap_or(path);
    let stem = stem.strip_suffix("/__init__").unwrap_or(stem);
    stem.replace('/', ".")
}

pub fn module_path(module: &str) -> String {
    format!("{}.py", module.replace('.', "/"))
}

/// Textual call-graph over the fixture tree. Resolves `from X import n`,
/// aliased imports (`from auth import tokens as tok` + `tok.refresh_token(`),
/// and relative imports (`from ..interfaces import AuthProvider`). A call
/// site's member is the enclosing dotted scope. The defining site of the
/// subject is never a member. This is evidence for the M2 fixture only —
/// the production semantic backend is the M8 SCIP adapter.
pub struct FixtureSource<'a> {
    syntactic: SyntacticSource<'a>,
    pub tree: &'a BTreeMap<String, String>,
}

impl<'a> FixtureSource<'a> {
    pub fn new(index: &'a PythonSyntaxIndex, tree: &'a BTreeMap<String, String>) -> Self {
        Self {
            syntactic: SyntacticSource::new(index),
            tree,
        }
    }

    /// Import bindings of one file: local name → dotted target.
    pub fn bindings(&self, path: &str, module: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let Some(content) = self.tree.get(path) else {
            return out;
        };
        for line in content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("from ") {
                let Some((base, names)) = rest.split_once(" import ") else {
                    continue;
                };
                let absolute = resolve_from_import(module, base.trim());
                for item in names.split(',') {
                    let item = item.trim();
                    let (name, binding) = match item.split_once(" as ") {
                        Some((n, a)) => (n.trim(), a.trim()),
                        None => (item, item),
                    };
                    let target = format!("{absolute}.{name}");
                    out.insert(binding.to_string(), target);
                }
            } else if let Some(rest) = line.strip_prefix("import ") {
                for item in rest.split(',') {
                    let item = item.trim();
                    let (target, binding) = match item.split_once(" as ") {
                        Some((t, a)) => (t.trim(), a.trim()),
                        None => (item, item),
                    };
                    out.insert(binding.to_string(), target.to_string());
                }
            }
        }
        out
    }

    /// (enclosing dotted scope, call expression candidates) per line.
    fn call_sites(&self, path: &str) -> Vec<(String, Option<String>, Vec<CallExpr>)> {
        let mut sites = Vec::new();
        let Some(content) = self.tree.get(path) else {
            return sites;
        };
        let module = module_name(path);
        let mut scopes: Vec<(usize, String)> = Vec::new();
        for line in content.lines() {
            let indent = line.len() - line.trim_start().len();
            while scopes.last().is_some_and(|(i, _)| *i >= indent) {
                scopes.pop();
            }
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("def ") {
                let name: String = rest
                    .split('(')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                scopes.push((indent, name));
                continue;
            }
            if trimmed.starts_with("class ") {
                let name: String = trimmed
                    .strip_prefix("class ")
                    .and_then(|r| r.split('(').next())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                scopes.push((indent, name));
                continue;
            }
            if trimmed.starts_with("from ") || trimmed.starts_with("import ") {
                continue;
            }
            let scope = (!scopes.is_empty()).then(|| {
                scopes
                    .iter()
                    .map(|(_, n)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            });
            sites.push((module.clone(), scope, scan_calls(line)));
        }
        sites
    }

    pub fn callers_value(&self, subject: &str) -> String {
        let def_module = subject.rsplit_once('.').map(|(m, _)| m).unwrap_or("");
        let def_fn = subject.rsplit_once('.').map(|(_, f)| f).unwrap_or(subject);
        let mut members: BTreeSet<String> = BTreeSet::new();
        for path in self.tree.keys() {
            let module = module_name(path);
            let bindings = self.bindings(path, &module);
            for (_line_module, scope, calls) in self.call_sites(path) {
                let Some(scope) = scope else { continue };
                if module == def_module && scope == def_fn {
                    continue;
                }
                for call in calls {
                    let resolved: Option<String> = match &call {
                        CallExpr::Name(name) => bindings
                            .get(name)
                            .filter(|t| *t == subject)
                            .map(|_| format!("{module}.{scope}")),
                        CallExpr::Attribute(alias, tail) => bindings
                            .get(alias)
                            .map(|t| format!("{t}.{tail}"))
                            .filter(|t| t == subject)
                            .map(|_| format!("{module}.{scope}")),
                    };
                    if let Some(member) = resolved {
                        members.insert(member);
                    }
                }
            }
        }
        members.into_iter().collect::<Vec<_>>().join("; ")
    }

    pub fn implementations_value(&self, subject: &str) -> String {
        let def_module = subject.rsplit_once('.').map(|(m, _)| m).unwrap_or("");
        let class_name = subject.rsplit_once('.').map(|(_, c)| c).unwrap_or(subject);
        let mut members: BTreeSet<String> = BTreeSet::new();
        for path in self.tree.keys() {
            let module = module_name(path);
            if module == def_module {
                continue;
            }
            let bindings = self.bindings(path, &module);
            let Some(content) = self.tree.get(path) else {
                continue;
            };
            for line in content.lines() {
                let Some(rest) = line.trim().strip_prefix("class ") else {
                    continue;
                };
                let Some((name, bases)) = rest.split_once('(') else {
                    continue;
                };
                let name = name.trim();
                let bases = bases.rsplit_once(')').map(|(b, _)| b).unwrap_or(bases);
                let inherits = bases.split(',').any(|base| {
                    let base = base.trim();
                    let target = match base.split_once('.') {
                        Some((alias, tail)) => bindings.get(alias).map(|t| format!("{t}.{tail}")),
                        None => bindings.get(base).cloned(),
                    };
                    target.as_deref() == Some(subject)
                });
                if inherits {
                    members.insert(format!("{module}.{name}"));
                }
            }
        }
        let _ = class_name;
        members.into_iter().collect::<Vec<_>>().join("; ")
    }
}

pub enum CallExpr {
    Name(String),
    Attribute(String, String),
}

pub fn scan_calls(line: &str) -> Vec<CallExpr> {
    let mut calls = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &line[start..i];
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'(' {
                calls.push(CallExpr::Name(ident.to_string()));
            } else if j < bytes.len() && bytes[j] == b'.' {
                let mut k = j + 1;
                let attr_start = k;
                while k < bytes.len() && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_') {
                    k += 1;
                }
                let attr = &line[attr_start..k];
                let mut m = k;
                while m < bytes.len() && bytes[m] == b' ' {
                    m += 1;
                }
                if !attr.is_empty() && m < bytes.len() && bytes[m] == b'(' {
                    calls.push(CallExpr::Attribute(ident.to_string(), attr.to_string()));
                }
            }
        } else {
            i += 1;
        }
    }
    calls
}

pub fn resolve_from_import(current_module: &str, base: &str) -> String {
    let dots = base.chars().take_while(|c| *c == '.').count();
    if dots == 0 {
        return base.to_string();
    }
    let rest = &base[dots..];
    let components: Vec<&str> = current_module.split('.').collect();
    let package_len = components.len().saturating_sub(1);
    let up = dots - 1;
    let keep = package_len.saturating_sub(up);
    let mut resolved: Vec<&str> = components[..keep].to_vec();
    if !rest.is_empty() {
        resolved.extend(rest.split('.'));
    }
    resolved.join(".")
}

impl ReevaluationSource for FixtureSource<'_> {
    fn evaluate(
        &self,
        projection: &Projection,
    ) -> Result<
        Option<trellis_engine::redgreen::Evaluated>,
        trellis_engine::redgreen::TransitionError,
    > {
        match projection.kind() {
            ProjectionKind::Callers => Ok(Some(trellis_engine::redgreen::Evaluated::new(
                self.callers_value(projection.subject().canonical()),
                true,
            ))),
            ProjectionKind::Implementations => Ok(Some(trellis_engine::redgreen::Evaluated::new(
                self.implementations_value(projection.subject().canonical()),
                true,
            ))),
            _ => self.syntactic.evaluate(projection),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Catalog parsing helpers
// ─────────────────────────────────────────────────────────────────────

pub fn parse_projection(key: &str) -> Projection {
    let (kind, rest) = key.split_once('(').expect("kind(subject, scope) form");
    let inner = rest.strip_suffix(')').expect("closing paren");
    let (subject, scope) = match inner.rsplit_once(", ") {
        Some((subject, scope)) => (subject, scope),
        // Scope-less keys (the catalog's FileContent entries) bind the
        // file scope by construction.
        None => (inner, "File"),
    };
    let scope = match scope {
        "File" => Scope::File,
        "Module" => Scope::Module,
        "Package" => Scope::Package,
        "Repository" => Scope::Repository,
        other => panic!("unknown scope {other}"),
    };
    match kind {
        "FileContent" => Projection::file(subject),
        "Definition" => Projection::definition(subject),
        "Signature" => Projection::signature(subject),
        "Imports" => Projection::imports(subject, scope),
        "Callers" => Projection::callers(subject, scope),
        "References" => Projection::references(subject, scope),
        "Implementations" => Projection::implementations(subject, scope),
        "Subclasses" => Projection::subclasses(subject, scope),
        other => panic!("unknown kind {other}"),
    }
    .expect("catalog projection key valid")
}

pub fn parse_validity_label(label: &str) -> Validity {
    match label {
        "VALID" => Validity::Valid,
        "STALE" => Validity::Stale,
        "UNKNOWN" => Validity::Unknown,
        other => panic!("unknown validity label {other}"),
    }
}

pub fn parse_kind_label(label: &str) -> ArtifactKind {
    match label {
        "STRUCTURAL_SET" => ArtifactKind::StructuralSet,
        "FACT" => ArtifactKind::Fact,
        other => panic!("unknown artifact kind {other}"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Seeding
// ─────────────────────────────────────────────────────────────────────

pub struct Seeded {
    pub store: Store,
    pub work: TempDir,
    pub tree: BTreeMap<String, String>,
    pub snapshot: SnapshotId,
    pub artifact_ids: BTreeMap<String, ArtifactId>,
}

pub fn snapshot_id_for(tree: &BTreeMap<String, String>) -> SnapshotId {
    SnapshotId::from_hash(tree_digest(tree))
}

pub fn put_snapshot_row(
    store: &mut Store,
    id: SnapshotId,
    parent: Option<SnapshotId>,
    at: Timestamp,
) {
    let manifest = ManifestId::from_hash(digest_of(format!("manifest:{id}").as_bytes()));
    let row = Snapshot::new(
        id,
        RepositoryId::from_hash(digest_of(b"trellis.harness.repo")),
        None,
        manifest,
        None,
        EnvironmentFingerprintId::from_hash(digest_of(b"trellis.harness.env")),
        parent,
        at,
    )
    .expect("snapshot valid");
    store.put_snapshot(&row).expect("snapshot persisted");
}

pub fn contract_for(
    artifact_id: &str,
    kind: &ArtifactKind,
) -> Option<trellis_core::ids::VerifierId> {
    match kind {
        ArtifactKind::StructuralSet => Some(contract_verifier_id(SET_EQUALITY_CONTRACT)),
        ArtifactKind::Fact => (artifact_id == "ART_NO_CALLERS_FACT")
            .then(|| contract_verifier_id(ABSENCE_FACT_CONTRACT)),
        _ => None,
    }
}

pub fn anchor_for(projection: &Projection, tree: &BTreeMap<String, String>) -> String {
    match projection.kind() {
        ProjectionKind::FileContent => projection.subject().canonical().to_string(),
        ProjectionKind::Signature => module_path("auth.interfaces"),
        ProjectionKind::Implementations => module_path("auth.interfaces"),
        ProjectionKind::Callers => {
            // The defining module is the longest dotted prefix of the
            // subject that names an existing module file (mirrors M3's
            // symbol→unit resolution; handles methods like
            // auth.service.AuthService.login → auth/service.py).
            let subject = projection.subject().canonical();
            let parts: Vec<&str> = subject.split('.').collect();
            let path = (1..parts.len())
                .rev()
                .map(|n| module_path(&parts[..n].join(".")))
                .find(|p| tree.contains_key(p))
                .unwrap_or_else(|| panic!("definition site of {subject} not found"));
            path
        }
        other => panic!("no fixture anchor rule for kind {other:?}"),
    }
}

pub fn seed_base() -> Seeded {
    let work = tempfile::tempdir().expect("tempdir");
    let tree = pristine_tree();
    let tree_root = work.path().join("tree");
    std::fs::create_dir_all(&tree_root).expect("tree root creatable");
    write_tree(&tree_root, &tree);

    let mut store = Store::open(work.path().join("metadata.db")).expect("store opens");
    let snapshot = snapshot_id_for(&tree);
    put_snapshot_row(&mut store, snapshot, None, BASE_TIMESTAMP);

    let index = PythonSyntaxIndex::index(&tree);
    let source = FixtureSource::new(&index, &tree);
    let catalog = Catalog::load();

    let mut artifact_ids = BTreeMap::new();
    let mut dep_observations: BTreeMap<String, Vec<ProjectionObservationId>> = BTreeMap::new();

    for artifact in &catalog.seeded_artifacts {
        let mut dep_ids = Vec::new();
        let mut evidence = Vec::new();
        for key in &artifact.depends_on {
            let projection = parse_projection(key);
            let value = source
                .evaluate(&projection)
                .expect("source evaluates")
                .expect("fixture source proves every seeded dependency at base");
            let obs = ProjectionObservation::new(
                canonical_observation_id(&projection.id(), &digest_str(value.value()), &snapshot),
                projection.id(),
                snapshot,
                digest_str(value.value()),
                None,
            );
            store
                .record_observation_record(&projection, &obs, &anchor_for(&projection, &tree))
                .expect("base observation recorded");
            if projection.kind().is_completeness_sensitive() {
                // §8.1: completeness-sensitive observations bind to
                // (Q, U, C). The fixture semantic backend is fully
                // capable over the pristine fixture, so the baseline
                // certificate is complete.
                store
                    .put_completeness_binding(
                        &projection.id(),
                        &snapshot,
                        &Universe::new(tree.keys().cloned()).digest(),
                        &CoverageCertificate::complete().digest(),
                        FIXTURE_INDEXER,
                        Completeness::Complete,
                    )
                    .expect("base binding recorded");
            }
            evidence.push(EvidenceRef::Observation(obs.id()));
            dep_ids.push(projection.id());
            dep_observations
                .entry(artifact.id.clone())
                .or_default()
                .push(obs.id());
        }

        let kind = parse_kind_label(&artifact.kind);
        let art_id = ArtifactId::from_hash(digest_str(&format!(
            "trellis.harness.artifact:{}",
            artifact.id
        )));
        let derivation = Derivation::new(
            DerivationId::from_hash(digest_str(&format!(
                "trellis.harness.derivation:{}",
                artifact.id
            ))),
            vec![trellis_core::validity::CaptureChannel::TrellisTool],
            ProducerInfo::new("trellis-m6-harness", "0.1.0", None).expect("producer"),
            BASE_TIMESTAMP,
        );
        let mut payload = String::new();
        for (key, dep) in artifact.depends_on.iter().zip(&dep_ids) {
            let projection = parse_projection(key);
            let value = source
                .evaluate(&projection)
                .expect("source evaluates")
                .expect("fixture source proves every seeded dependency at base");
            payload.push_str(&format!("{} {}\n", dep, digest_str(value.value())));
        }
        let blob = store.put_blob(payload.as_bytes()).expect("payload blob");
        let envelope = ArtifactEnvelope::new(
            art_id,
            1,
            kind,
            blob,
            artifact
                .proposition
                .as_deref()
                .map(|p| Proposition::new(p).expect("catalog proposition valid")),
            ProducerInfo::new("trellis-m6-harness", "0.1.0", None).expect("producer"),
            derivation.clone(),
            dep_ids.clone(),
            snapshot,
            CostRecord::default(),
        )
        .expect("envelope valid");
        store.put_artifact(&envelope).expect("artifact persisted");

        let att = ValidationAttestation::for_derivation(
            AttestationId::from_hash(digest_str(&format!("trellis.harness.seed:{}", artifact.id))),
            art_id,
            &derivation,
            snapshot,
            None,
            Validity::Valid,
            Authority::Authoritative,
            VerificationLevel::Structural,
            contract_for(&artifact.id, &kind),
            evidence,
            BASE_TIMESTAMP,
        );
        store.append_attestation(&att).expect("base attestation");
        artifact_ids.insert(artifact.id.clone(), art_id);
    }

    Seeded {
        store,
        work,
        tree,
        snapshot,
        artifact_ids,
    }
}

pub fn run_mutation(seeded: &mut Seeded, mutation: &Mutation, now: Timestamp) -> TransitionReport {
    run_mutation_ctx(seeded, mutation, now, None)
}

pub fn coverage_context(
    paths: &[String],
    indexer: &str,
    certificate: CoverageCertificate,
) -> CoverageContext {
    CoverageContext {
        universe: Universe::new(paths.iter().cloned()),
        certificate,
        indexer: indexer.to_string(),
    }
}

pub fn run_mutation_ctx(
    seeded: &mut Seeded,
    mutation: &Mutation,
    now: Timestamp,
    coverage: Option<CoverageContext>,
) -> TransitionReport {
    run_mutation_with(seeded, mutation, now, coverage, |tree, _paths| {
        let _ = tree;
        None
    })
}

/// Run one mutation with a POST-APPLY coverage hook: after the ops are
/// applied and the tree re-read, the hook may return the coverage
/// context describing the mutated world (the §8.2 X5 path — the
/// certificate must describe the post-mutation coverage state).
pub fn run_mutation_with(
    seeded: &mut Seeded,
    mutation: &Mutation,
    now: Timestamp,
    coverage: Option<CoverageContext>,
    post_apply_hook: fn(&BTreeMap<String, String>, &[String]) -> Option<CoverageContext>,
) -> TransitionReport {
    let work_root = seeded.work.path().join("tree");
    let manifest_prev = build_manifest(&ManifestOptions::new(&work_root)).expect("manifest builds");
    apply_ops(&work_root, &mutation.ops);
    let changed = reconcile_against_working_tree(&manifest_prev, &ManifestOptions::new(&work_root))
        .expect("reconciles");
    seeded.tree = read_tree_from(&work_root);
    let snapshot = snapshot_id_for(&seeded.tree);
    put_snapshot_row(&mut seeded.store, snapshot, Some(seeded.snapshot), now);
    seeded.snapshot = snapshot;

    let paths: Vec<String> = seeded.tree.keys().cloned().collect();
    let coverage = match coverage {
        Some(ctx) => Some(ctx),
        None => post_apply_hook(&seeded.tree, &paths),
    };

    let index = PythonSyntaxIndex::index(&seeded.tree);
    let source = FixtureSource::new(&index, &seeded.tree);
    let transition = Transition::new(&mut seeded.store, &source, changed, snapshot, now);
    let transition = match coverage {
        Some(ctx) => transition.with_coverage(ctx),
        None => transition,
    };
    transition.run().expect("transition runs")
}

/// The M7 X5 coverage hook: a certificate from the syntax backend over
/// the post-mutation tree (users/broken.py is an explicit coverage
/// failure). This is the production path where §8.2 coverage-degradation
/// semantics live.
pub fn x5_coverage_hook(
    tree: &BTreeMap<String, String>,
    _paths: &[String],
) -> Option<CoverageContext> {
    let paths: Vec<String> = tree.keys().cloned().collect();
    let certificate = PythonSyntaxIndex::index(tree).coverage_certificate(&paths);
    Some(coverage_context(&paths, FIXTURE_INDEXER, certificate))
}

pub fn latest_validity(store: &Store, artifact: &ArtifactId) -> Option<Validity> {
    store
        .attestation_history(artifact)
        .expect("history readable")
        .iter()
        .last()
        .map(|att| att.validity())
}

pub fn history_len(store: &Store, artifact: &ArtifactId) -> usize {
    store.attestation_history(artifact).expect("history").len()
}

pub fn find_outcome<'a>(
    report: &'a TransitionReport,
    kind: ProjectionKind,
    subject: &str,
) -> &'a ProjectionOutcome {
    report
        .projections()
        .iter()
        .find(|o| o.projection().kind() == kind && o.projection().subject().canonical() == subject)
        .unwrap_or_else(|| panic!("no outcome for {kind:?}({subject})"))
}

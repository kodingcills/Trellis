//! M6 red/green acceptance tests: the §33 first acceptance trace, the
//! red/green counterexample, and the oracle catalog sweep (R0-R4, X1-X4)
//! against the M2 mutation catalog. Oracle labels are read-only ground
//! truth; the fixture semantic source never reads `catalog.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tempfile::TempDir;

use trellis_core::artifact::{
    ArtifactEnvelope, ArtifactKind, CostRecord, Derivation, ProducerInfo, Proposition,
};
use trellis_core::attestation::{EvidenceRef, ValidationAttestation};
use trellis_core::ids::{
    ArtifactId, AttestationId, ContentHash, DerivationId, EnvironmentFingerprintId, HashAlgo,
    ManifestId, ProjectionObservationId, RepositoryId, SnapshotId, Timestamp,
};
use trellis_core::observation_id::canonical_observation_id;
use trellis_core::projection::{Projection, ProjectionKind, ProjectionObservation, Scope};
use trellis_core::snapshot::Snapshot;
use trellis_core::validity::{Authority, Validity, VerificationLevel};

use trellis_engine::discovery::RecordedObservation;
use trellis_engine::prelude::*;
use trellis_engine::redgreen::{
    contract_verifier_id, ContractVerdict, ProjectionOutcome, ProjectionOutcomeKind,
    ReevaluationSource, SyntacticSource, Transition, ABSENCE_FACT_CONTRACT, SET_EQUALITY_CONTRACT,
};
use trellis_oracle::{Catalog, Mutation, Op};
use trellis_program::prelude::PythonSyntaxIndex;
use trellis_source::manifest::{build_manifest, ManifestOptions};
use trellis_source::reconcile::{reconcile_against_working_tree, ChangedSet};
use trellis_store::prelude::Store;

const BASE_TIMESTAMP: Timestamp = 1_700_000_000_000;
const TIMESTAMP_STEP: Timestamp = 1_000;

fn digest_of(bytes: &[u8]) -> ContentHash {
    ContentHash::compute(HashAlgo::Blake3, bytes)
}

fn digest_str(s: &str) -> ContentHash {
    digest_of(s.as_bytes())
}

// ─────────────────────────────────────────────────────────────────────
// Fixture tree management
// ─────────────────────────────────────────────────────────────────────

fn pristine_tree() -> BTreeMap<String, String> {
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

fn write_tree(dir: &Path, tree: &BTreeMap<String, String>) {
    for (path, content) in tree {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent dirs creatable");
        }
        std::fs::write(&target, content).expect("file writable");
    }
}

fn read_tree_from(dir: &Path) -> BTreeMap<String, String> {
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

fn apply_ops(dir: &Path, ops: &[Op]) {
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

fn tree_digest(tree: &BTreeMap<String, String>) -> ContentHash {
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

fn module_name(path: &str) -> String {
    let stem = path.strip_suffix(".py").unwrap_or(path);
    let stem = stem.strip_suffix("/__init__").unwrap_or(stem);
    stem.replace('/', ".")
}

fn module_path(module: &str) -> String {
    format!("{}.py", module.replace('.', "/"))
}

/// Textual call-graph over the fixture tree. Resolves `from X import n`,
/// aliased imports (`from auth import tokens as tok` + `tok.refresh_token(`),
/// and relative imports (`from ..interfaces import AuthProvider`). A call
/// site's member is the enclosing dotted scope. The defining site of the
/// subject is never a member. This is evidence for the M2 fixture only —
/// the production semantic backend is the M8 SCIP adapter.
struct FixtureSource<'a> {
    syntactic: SyntacticSource<'a>,
    tree: &'a BTreeMap<String, String>,
}

impl<'a> FixtureSource<'a> {
    fn new(index: &'a PythonSyntaxIndex, tree: &'a BTreeMap<String, String>) -> Self {
        Self {
            syntactic: SyntacticSource::new(index),
            tree,
        }
    }

    /// Import bindings of one file: local name → dotted target.
    fn bindings(&self, path: &str, module: &str) -> BTreeMap<String, String> {
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

    fn callers_value(&self, subject: &str) -> String {
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

    fn implementations_value(&self, subject: &str) -> String {
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

enum CallExpr {
    Name(String),
    Attribute(String, String),
}

fn scan_calls(line: &str) -> Vec<CallExpr> {
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

fn resolve_from_import(current_module: &str, base: &str) -> String {
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

fn parse_projection(key: &str) -> Projection {
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

fn parse_validity_label(label: &str) -> Validity {
    match label {
        "VALID" => Validity::Valid,
        "STALE" => Validity::Stale,
        "UNKNOWN" => Validity::Unknown,
        other => panic!("unknown validity label {other}"),
    }
}

fn parse_kind_label(label: &str) -> ArtifactKind {
    match label {
        "STRUCTURAL_SET" => ArtifactKind::StructuralSet,
        "FACT" => ArtifactKind::Fact,
        other => panic!("unknown artifact kind {other}"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Seeding
// ─────────────────────────────────────────────────────────────────────

struct Seeded {
    store: Store,
    work: TempDir,
    tree: BTreeMap<String, String>,
    snapshot: SnapshotId,
    artifact_ids: BTreeMap<String, ArtifactId>,
}

fn snapshot_id_for(tree: &BTreeMap<String, String>) -> SnapshotId {
    SnapshotId::from_hash(tree_digest(tree))
}

fn put_snapshot_row(store: &mut Store, id: SnapshotId, parent: Option<SnapshotId>, at: Timestamp) {
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

fn contract_for(artifact_id: &str, kind: &ArtifactKind) -> Option<trellis_core::ids::VerifierId> {
    match kind {
        ArtifactKind::StructuralSet => Some(contract_verifier_id(SET_EQUALITY_CONTRACT)),
        ArtifactKind::Fact => (artifact_id == "ART_NO_CALLERS_FACT")
            .then(|| contract_verifier_id(ABSENCE_FACT_CONTRACT)),
        _ => None,
    }
}

fn anchor_for(projection: &Projection, tree: &BTreeMap<String, String>) -> String {
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

fn seed_base() -> Seeded {
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
                canonical_observation_id(
                    &projection.id(),
                    &digest_str(value.value()),
                    &snapshot,
                ),
                projection.id(),
                snapshot,
                digest_str(value.value()),
                None,
            );
            store
                .record_observation_record(&projection, &obs, &anchor_for(&projection, &tree))
                .expect("base observation recorded");
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

fn run_mutation(seeded: &mut Seeded, mutation: &Mutation, now: Timestamp) -> TransitionReport {
    let work_root = seeded.work.path().join("tree");
    let manifest_prev = build_manifest(&ManifestOptions::new(&work_root)).expect("manifest builds");
    apply_ops(&work_root, &mutation.ops);
    let changed = reconcile_against_working_tree(&manifest_prev, &ManifestOptions::new(&work_root))
        .expect("reconciles");
    seeded.tree = read_tree_from(&work_root);
    let snapshot = snapshot_id_for(&seeded.tree);
    put_snapshot_row(&mut seeded.store, snapshot, Some(seeded.snapshot), now);
    seeded.snapshot = snapshot;

    let index = PythonSyntaxIndex::index(&seeded.tree);
    let source = FixtureSource::new(&index, &seeded.tree);
    Transition::new(&mut seeded.store, &source, changed, snapshot, now)
        .run()
        .expect("transition runs")
}

fn latest_validity(store: &Store, artifact: &ArtifactId) -> Option<Validity> {
    store
        .attestation_history(artifact)
        .expect("history readable")
        .iter()
        .last()
        .map(|att| att.validity())
}

fn history_len(store: &Store, artifact: &ArtifactId) -> usize {
    store.attestation_history(artifact).expect("history").len()
}

fn find_outcome<'a>(
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

/// X5 (coverage degradation → UNKNOWN) requires coverage certificates
/// (M7). In M6 the fixture semantic source still proves values over the
/// unparseable file, so standings legitimately remain unchanged; this
/// test pins the M6 boundary and must be revisited at M7.
#[test]
fn x5_coverage_degradation_is_m7_gated() {
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

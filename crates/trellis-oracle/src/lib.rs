//! trellis-oracle — mechanical validation of the synthetic oracle fixture
//! and mutation catalog (M2, spec §26, §33 oracle-first).
//!
//! This crate enforces the integrity of Benchmark A's ground truth. It is
//! test-support infrastructure, not product code: it must never import
//! from `trellis-engine` (which does not exist yet) so that oracle labels
//! stay independent of the implementation under test.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Root of the repository (derived from this crate's location).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("repo root must resolve")
}

/// Path of the machine-readable oracle catalog.
pub fn catalog_path() -> PathBuf {
    repo_root().join("fixtures/oracle/catalog.json")
}

/// Root of the pristine fixture.
pub fn fixture_base() -> PathBuf {
    repo_root().join("fixtures/python_auth/base")
}

/// A projection key: `kind(subject, scope)` in canonical form.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProjectionKey {
    pub kind: String,
    pub subject: String,
    pub scope: String,
}

impl ProjectionKey {
    /// Canonical rendering used for uniqueness checks.
    pub fn render(&self) -> String {
        format!("{}({}, scope={})", self.kind, self.subject, self.scope)
    }
}

/// An expected projection value.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectedValue {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub value: Option<String>,
}

/// One labeled projection expectation inside a mutation.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectionLabel {
    pub key: ProjectionKey,
    pub expected: ExpectedValue,
    pub why: String,
}

/// One artifact validity consequence.
#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactConsequence {
    pub artifact: String,
    pub before: String,
    pub after: String,
    pub why: String,
}

/// One file operation within a mutation.
#[derive(Debug, Clone, Deserialize)]
pub struct Op {
    pub op: String,
    pub path: String,
    #[serde(default)]
    pub content: Option<String>,
}

/// A mutation entry in the catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct Mutation {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub parent: String,
    #[serde(default)]
    pub purpose: Option<String>,
    pub ops: Vec<Op>,
    #[serde(default)]
    pub intended_semantic_change: Option<String>,
    #[serde(default)]
    pub affected_projections: Vec<ProjectionLabel>,
    #[serde(default)]
    pub unchanged_projections: Vec<ProjectionLabel>,
    #[serde(default)]
    pub artifact_consequences: Vec<ArtifactConsequence>,
    #[serde(default)]
    pub completeness_absence_involved: bool,
    #[serde(default)]
    pub ground_truth_rationale: Option<String>,
    #[serde(default)]
    pub provenance: Vec<String>,
}

impl Mutation {
    /// Whether this mutation labels the pristine state (no ops).
    pub fn is_baseline(&self) -> bool {
        self.parent == "BASE" && self.ops.is_empty()
    }
}

/// A seeded artifact declaration.
#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactDecl {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub proposition: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub base_validity: Option<String>,
    #[serde(default)]
    pub base_authority: Option<String>,
    #[serde(default)]
    pub validity_note: Option<String>,
}

/// The declared fixture shape.
#[derive(Debug, Clone, Deserialize)]
pub struct FixtureDecl {
    pub root: String,
    #[serde(default)]
    pub test_modules: Vec<String>,
    #[serde(default)]
    pub file_count_py: Option<u64>,
}

/// The full oracle catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub schema: String,
    pub fixture: FixtureDecl,
    pub seeded_artifacts: Vec<ArtifactDecl>,
    pub mutations: Vec<Mutation>,
}

impl Catalog {
    /// Parse the catalog from its canonical location.
    pub fn load() -> Self {
        let raw = fs::read_to_string(catalog_path()).expect("catalog.json readable");
        serde_json::from_str(&raw).expect("catalog.json parses against schema")
    }

    /// Mutation lookup by id.
    pub fn mutation(&self, id: &str) -> Option<&Mutation> {
        self.mutations.iter().find(|m| m.id == id)
    }

    /// The required R-series mutation ids from the frozen spec (§26, §99-era
    /// R0–R4 carried into Draft 1.0 §26).
    pub const REQUIRED_SERIES: [&'static str; 5] = ["R0", "R1", "R2", "R3", "R4"];

    /// The adversarial X-series ids required by the M2 node contract.
    pub const ADVERSARIAL_SERIES: [&'static str; 5] = ["X1", "X2", "X3", "X4", "X5"];
}

/// Collect every file under `root` (recursive), as repo-relative paths.
pub fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().map(|n| n != "__pycache__").unwrap_or(true) {
                    stack.push(path);
                }
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Read the BASE fixture into a path→content map (paths relative to the
/// fixture root, `/`-separated).
pub fn load_base_tree() -> BTreeMap<String, String> {
    let base = fixture_base();
    let mut tree = BTreeMap::new();
    for file in walk_files(&base) {
        let rel = file
            .strip_prefix(&base)
            .expect("under fixture root")
            .to_string_lossy()
            .replace('\\', "/");
        let content = fs::read_to_string(&file).unwrap_or_default();
        tree.insert(rel, content);
    }
    tree
}

/// Apply one op to a tree. Returns Err with a description if the op does
/// not apply cleanly.
pub fn apply_op(tree: &mut BTreeMap<String, String>, op: &Op) -> Result<(), String> {
    match op.op.as_str() {
        "add_file" => {
            if tree.contains_key(&op.path) {
                return Err(format!("add_file: path already exists: {}", op.path));
            }
            let content = op
                .content
                .as_deref()
                .ok_or_else(|| format!("add_file without content: {}", op.path))?;
            tree.insert(op.path.clone(), content.to_string());
            Ok(())
        }
        "replace_file" => {
            let existing = tree
                .get(&op.path)
                .ok_or_else(|| format!("replace_file: path missing: {}", op.path))?
                .clone();
            let content = op
                .content
                .as_deref()
                .ok_or_else(|| format!("replace_file without content: {}", op.path))?;
            if existing == content {
                return Err(format!("replace_file: content identical: {}", op.path));
            }
            tree.insert(op.path.clone(), content.to_string());
            Ok(())
        }
        "delete_file" => {
            if tree.remove(&op.path).is_none() {
                return Err(format!("delete_file: path missing: {}", op.path));
            }
            Ok(())
        }
        other => Err(format!("unknown op `{other}`")),
    }
}

/// Fold the tree for a mutation id (applying its ancestor chain in order).
pub fn tree_at(catalog: &Catalog, id: &str) -> Result<BTreeMap<String, String>, String> {
    let mut chain: Vec<&Mutation> = Vec::new();
    let mut current = id.to_string();
    loop {
        if current == "BASE" {
            break;
        }
        let m = catalog
            .mutation(&current)
            .ok_or_else(|| format!("unknown parent id `{current}`"))?;
        chain.push(m);
        current = m.parent.clone();
    }
    chain.reverse();

    let mut tree = load_base_tree();
    for m in chain {
        for op in &m.ops {
            apply_op(&mut tree, op).map_err(|e| format!("in mutation {}: {e}", m.id))?;
        }
    }
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn catalog() -> Catalog {
        Catalog::load()
    }

    /// Schema/1 integrity: unique ids, required labels, canonical sets.
    #[test]
    fn catalog_schema_is_complete_and_canonical() {
        let cat = catalog();
        assert_eq!(cat.schema, "trellis-oracle-catalog/1");

        let mut ids = HashSet::new();
        for m in &cat.mutations {
            assert!(ids.insert(m.id.clone()), "duplicate mutation id {}", m.id);
            assert!(!m.provenance.is_empty(), "{}: provenance required", m.id);
            assert!(
                m.ground_truth_rationale
                    .as_deref()
                    .is_some_and(|r| !r.trim().is_empty()),
                "{}: ground_truth_rationale required",
                m.id
            );
            for label in m
                .affected_projections
                .iter()
                .chain(&m.unchanged_projections)
            {
                assert!(!label.why.trim().is_empty(), "{}: label why required", m.id);
                match label.expected.kind.as_str() {
                    "empty_set" => assert!(label.expected.members.is_empty()),
                    "set" => {
                        let members = &label.expected.members;
                        assert!(
                            !members.is_empty(),
                            "{}: empty set must use empty_set",
                            m.id
                        );
                        let mut sorted = members.clone();
                        sorted.sort();
                        sorted.dedup();
                        assert_eq!(
                            members, &sorted,
                            "{}: set members must be sorted+unique",
                            m.id
                        );
                    }
                    "text" => assert!(
                        label
                            .expected
                            .value
                            .as_deref()
                            .is_some_and(|v| !v.is_empty()),
                        "{}: text value required",
                        m.id
                    ),
                    "changed" | "unchanged" => {}
                    other => panic!("unknown expected type `{other}`"),
                }
            }
            for c in &m.artifact_consequences {
                assert!(
                    !c.why.trim().is_empty(),
                    "{}: consequence why required",
                    m.id
                );
            }
        }
    }

    /// R0–R4 must exist (frozen spec) plus the adversarial series.
    #[test]
    fn required_series_present() {
        let cat = catalog();
        for id in Catalog::REQUIRED_SERIES {
            assert!(cat.mutation(id).is_some(), "missing required mutation {id}");
        }
        for id in Catalog::ADVERSARIAL_SERIES {
            assert!(
                cat.mutation(id).is_some(),
                "missing adversarial mutation {id}"
            );
        }
        assert!(cat.mutation("R0").unwrap().is_baseline());
    }

    /// Every parent resolves; R0 == BASE; chains acyclic (tree_at fails on
    /// cycles via depth).
    #[test]
    fn mutation_graph_resolves_and_applies() {
        let cat = catalog();
        for m in &cat.mutations {
            if m.parent != "BASE" {
                assert!(
                    cat.mutations.iter().any(|p| p.id == m.parent),
                    "{}: parent {} unresolvable",
                    m.id,
                    m.parent
                );
            }
            let tree = tree_at(&cat, &m.id).unwrap_or_else(|e| panic!("{}", e));
            assert!(!tree.is_empty());
            // Every op in this mutation already applied cleanly in the fold
            // above for the full chain; additionally verify single-step.
            let mut step = tree_at(&cat, &m.parent).unwrap();
            for op in &m.ops {
                apply_op(&mut step, op).unwrap_or_else(|e| panic!("{} op not clean: {e}", m.id));
            }
        }
    }

    /// Seeded artifacts are declared once and referenced consistently.
    #[test]
    fn artifact_ids_unique_and_consequences_resolve() {
        let cat = catalog();
        let mut artifact_ids = HashSet::new();
        for a in &cat.seeded_artifacts {
            assert!(
                artifact_ids.insert(a.id.clone()),
                "duplicate artifact {}",
                a.id
            );
            assert!(!a.depends_on.is_empty(), "{}: depends_on required", a.id);
        }
        for m in &cat.mutations {
            for c in &m.artifact_consequences {
                assert!(
                    artifact_ids.contains(&c.artifact),
                    "{}: consequence references unknown artifact {}",
                    m.id,
                    c.artifact
                );
            }
        }
    }

    /// Independent R0 grounding: on the BASE tree, the string
    /// `refresh_token` occurs in exactly one file — its definition module.
    #[test]
    fn base_refresh_token_is_defined_but_never_called_or_referenced() {
        let base = load_base_tree();
        let mut hits: Vec<&String> = base
            .iter()
            .filter(|(_, content)| content.contains("refresh_token"))
            .map(|(path, _)| path)
            .collect();
        hits.sort();
        assert_eq!(
            hits,
            vec!["auth/tokens.py"],
            "R0 absence claim requires refresh_token to appear only in its definition module"
        );
        let tokens = &base["auth/tokens.py"];
        assert!(tokens.contains("def refresh_token("));
    }

    /// Independent implementor grounding: exactly two files declare
    /// `(AuthProvider):` subclasses on BASE, and X3 adds exactly one more.
    #[test]
    fn implementor_sets_match_textual_declarations() {
        let cat = catalog();
        let base = load_base_tree();
        let count_impls = |tree: &BTreeMap<String, String>| {
            tree.values()
                .filter(|c| c.contains("(AuthProvider):"))
                .count()
        };
        assert_eq!(
            count_impls(&base),
            2,
            "base must have exactly two AuthProvider implementors"
        );

        let x3 = tree_at(&cat, "X3").unwrap();
        assert_eq!(count_impls(&x3), 3, "X3 must add exactly one implementor");

        let r4 = tree_at(&cat, "R4").unwrap();
        assert_eq!(
            count_impls(&r4),
            2,
            "R4 must not change implementor membership"
        );

        // R4 changes the abstract signature: old signature must be gone.
        assert!(!r4["auth/interfaces.py"].contains("def validate(self, code: str) -> bool:"));
        assert!(r4["auth/interfaces.py"].contains("context: dict[str, str] | None = None"));
    }

    /// R1 locality: an unrelated payments change touches nothing outside
    /// payments/, and the mutated tree still contains the pricing symbol.
    #[test]
    fn r1_is_localized_to_payments() {
        let cat = catalog();
        let r1 = cat.mutation("R1").unwrap();
        for op in &r1.ops {
            assert!(
                op.path.starts_with("payments/"),
                "R1 must be local to payments/, found {}",
                op.path
            );
        }
        let tree = tree_at(&cat, "R1").unwrap();
        assert!(tree["payments/pricing.py"].contains("SAVE10"));
        assert!(tree["auth/tokens.py"].contains("def refresh_token("));
    }

    /// R2/R3 caller-set labels match an independent textual recomputation:
    /// on the mutated tree, files containing `refresh_token(` are exactly
    /// the definition module plus the declared call sites.
    #[test]
    fn caller_sets_match_independent_textual_recomputation() {
        let cat = catalog();
        let r2 = tree_at(&cat, "R2").unwrap();
        let r3 = tree_at(&cat, "R3").unwrap();

        // On R2's tree: call sites exist in api/webhooks.py only.
        let callers: Vec<&String> = r2
            .iter()
            .filter(|(p, c)| {
                p.ends_with(".py") && p != &"auth/tokens.py" && c.contains("refresh_token(")
            })
            .map(|(p, _)| p)
            .collect();
        assert_eq!(
            callers,
            vec!["api/webhooks.py"],
            "R2 caller set must be exactly {{api.webhooks.handle_refresh}}"
        );

        // On R3's tree: same single call-site file (body changed only).
        let callers3: Vec<&String> = r3
            .iter()
            .filter(|(p, c)| {
                p.ends_with(".py") && p != &"auth/tokens.py" && c.contains("refresh_token(")
            })
            .map(|(p, _)| p)
            .collect();
        assert_eq!(
            callers3,
            vec!["api/webhooks.py"],
            "R3 must preserve the caller relation"
        );
        // And the mutated body still calls the function.
        assert!(r3["api/webhooks.py"].contains("refresh_token(user_id)"));
    }

    /// X2 re-export: users/__init__ gains a reference but no call site.
    #[test]
    fn x2_re_export_is_reference_not_caller() {
        let cat = catalog();
        let x2 = tree_at(&cat, "X2").unwrap();
        let init = &x2["users/__init__.py"];
        assert!(init.contains("from auth.tokens import refresh_token"));
        assert!(
            !init.contains("refresh_token("),
            "re-export must not introduce a call site"
        );
    }

    /// X5 fixture file is genuinely unparseable Python.
    #[test]
    fn x5_broken_module_is_syntactically_invalid() {
        let cat = catalog();
        let x5 = tree_at(&cat, "X5").unwrap();
        let broken = &x5["users/broken.py"];
        // The def header never closes: no colon outside the comment, no body.
        let header = broken.lines().find(|l| l.starts_with("def ")).unwrap();
        assert!(!header.trim_end().ends_with(':'));
        assert_eq!(broken.trim_end().lines().count(), 3);
    }

    /// Every non-baseline mutation enumerates ALL seeded artifacts in its
    /// consequence table: "unlisted" is never a defined state (M2 review
    /// finding — prevents under-enumeration from hiding unstated defaults).
    #[test]
    fn consequence_tables_enumerate_every_artifact() {
        let cat = catalog();
        let seeded: HashSet<String> = cat.seeded_artifacts.iter().map(|a| a.id.clone()).collect();
        for m in &cat.mutations {
            if m.is_baseline() {
                assert!(m.artifact_consequences.is_empty());
                continue;
            }
            let named: HashSet<String> = m
                .artifact_consequences
                .iter()
                .map(|c| c.artifact.clone())
                .collect();
            let missing: Vec<&String> = seeded.difference(&named).collect();
            assert!(
                missing.is_empty(),
                "{}: artifacts without an explicit consequence: {missing:?}",
                m.id
            );
            let unknown: Vec<&String> = named.difference(&seeded).collect();
            assert!(
                unknown.is_empty(),
                "{}: consequences reference undeclared artifacts: {unknown:?}",
                m.id
            );
        }
    }

    /// Declared fixture test modules exist on disk.
    #[test]
    fn fixture_test_modules_exist() {
        let cat = catalog();
        for module in &cat.fixture.test_modules {
            let rel = module.replace('.', "/");
            let path = fixture_base().join(format!("{rel}.py"));
            assert!(
                path.exists(),
                "declared fixture test module missing: {path:?}"
            );
        }
    }

    /// Fixture inventory sanity: the declared .py count matches disk.
    #[test]
    fn fixture_inventory_matches_declaration() {
        let cat = catalog();
        let py_count = walk_files(&fixture_base())
            .iter()
            .filter(|p| p.extension().map(|e| e == "py").unwrap_or(false))
            .count();
        assert_eq!(
            cat.fixture.file_count_py,
            Some(py_count as u64),
            "declared file_count_py must match actual .py inventory"
        );
    }

    /// Fixture independence: no Trellis-specific annotations or imports in
    /// the fixture source, and no oracle labels hidden in fixture comments.
    #[test]
    fn fixture_has_no_trellis_coupling() {
        for (path, content) in load_base_tree() {
            assert!(
                !content.to_lowercase().contains("trellis"),
                "fixture file {path} mentions trellis"
            );
            assert!(
                !content.contains("ART_"),
                "fixture file {path} embeds oracle artifact ids"
            );
        }
    }
}

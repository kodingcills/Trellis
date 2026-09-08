//! Integration tests: fixture-backed projections and architecture
//! boundary tests (M3 node contract, spec §9.3, §20, §8).

use std::collections::BTreeMap;

use crate::prelude::*;

fn fixture_tree() -> BTreeMap<String, String> {
    // Read the M2 fixture through the M1 layer (no second filesystem model).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/python_auth/base")
        .canonicalize()
        .expect("fixture exists");
    trellis_source::prelude::read_tree(&trellis_source::prelude::ManifestOptions::new(root))
        .expect("fixture reads")
}

fn index_fixture() -> PythonSyntaxIndex {
    PythonSyntaxIndex::index(&fixture_tree())
}

#[test]
fn full_fixture_indexes_deterministically_across_runs() {
    let i1 = index_fixture();
    let i2 = index_fixture();
    assert_eq!(i1.unit_count(), i2.unit_count());
    // Determinism across repeated index builds: identical extraction.
    let m = ModuleId::from_path("auth/tokens.py").unwrap();
    let d1 = i1.definitions_in(&m);
    let d2 = i2.definitions_in(&m);
    assert_eq!(d1, d2);
    assert!(d1.is_proven());
}

#[test]
fn traversal_order_independence() {
    // Build two indexes from trees constructed in different orders; the
    // projection values must be identical.
    let t1 = fixture_tree();
    let keys: Vec<String> = t1.keys().cloned().collect();
    let mut reversed_tree = BTreeMap::new();
    for k in keys.iter().rev() {
        reversed_tree.insert(k.clone(), t1[k].clone());
    }
    let i1 = PythonSyntaxIndex::index(&t1);
    let i2 = PythonSyntaxIndex::index(&reversed_tree);
    let m = ModuleId::from_path("users/service.py").unwrap();
    assert_eq!(i1.definitions_in(&m), i2.definitions_in(&m));
}

#[test]
fn definitions_identity_on_fixture() {
    let idx = index_fixture();
    // Module function.
    let def = idx
        .definition(&SymbolPath::new("auth.tokens.refresh_token").unwrap())
        .proven()
        .expect("proven definition query");
    let def = def.expect("refresh_token defined");
    assert_eq!(def.kind, DefKind::Function);

    // Class + method.
    let cls = idx
        .definition(&SymbolPath::new("auth.service.AuthService").unwrap())
        .proven()
        .flatten()
        .expect("AuthService defined");
    assert_eq!(cls.kind, DefKind::Class);
    let method = idx
        .definition(&SymbolPath::new("auth.service.AuthService.login").unwrap())
        .proven()
        .flatten()
        .expect("login defined");
    assert_eq!(method.kind, DefKind::Method);

    // Nested function.
    let nested = idx
        .definition(&SymbolPath::new("auth.tokens._sign").unwrap())
        .proven()
        .flatten();
    assert!(nested.is_some(), "module-level private function indexed");
}

#[test]
fn provider_implementations_exist_as_classes_but_semantic_query_unsupported() {
    let idx = index_fixture();
    // The classes are syntactically defined.
    let oidc = idx
        .definition(&SymbolPath::new("auth.providers.oidc_provider.OidcAuthProvider").unwrap())
        .proven()
        .flatten();
    assert!(oidc.is_some());
    // But the SEMANTIC implementations query must not fabricate a set.
    let iface = SymbolPath::new("auth.interfaces.AuthProvider").unwrap();
    let answer = idx.implementations(&iface);
    assert!(
        !answer.is_proven(),
        "implementations must not be proven by a syntax backend"
    );
}

#[test]
fn signature_extraction_matches_oracle_normalized_forms() {
    let idx = index_fixture();
    // M2 oracle: validate's base signature is "(self, code: str) -> bool".
    let sig = idx
        .signature(&SymbolPath::new("auth.interfaces.AuthProvider.validate").unwrap())
        .proven()
        .flatten()
        .expect("validate signature");
    assert_eq!(sig.canonical(), "(self, code: str) -> bool");

    // tokens.issue_token(user_id: str, ttl_seconds: int | None = None) -> str
    let sig = idx
        .signature(&SymbolPath::new("auth.tokens.issue_token").unwrap())
        .proven()
        .flatten()
        .expect("issue_token signature");
    assert_eq!(
        sig.canonical(),
        "(user_id: str, ttl_seconds: int | None = None) -> str"
    );
}

#[test]
fn imports_and_aliases_represented_syntactically() {
    let idx = index_fixture();
    // auth/service.py: `from .interfaces import AuthProvider` — a relative
    // from-import; the syntactic target keeps its relative form.
    let m = ModuleId::from_path("auth/service.py").unwrap();
    let imports = idx.imports(&m).proven().expect("service imports");
    let auth_provider = imports
        .iter()
        .find(|i| i.binding == "AuthProvider")
        .expect("AuthProvider import recorded");
    assert!(auth_provider.from_import);
    assert!(auth_provider.target.ends_with(".interfaces.AuthProvider"));

    // Multiple names from one from-import are separate imports.
    let session = idx
        .imports(&ModuleId::from_path("auth/session.py").unwrap())
        .proven()
        .unwrap();
    let bindings: Vec<&str> = session.iter().map(|i| i.binding.as_str()).collect();
    assert!(bindings.contains(&"issue_token") && bindings.contains(&"verify_token"));
    assert!(session.iter().any(|i| i.binding == "Settings"));

    // Aliased import binding: verify alias capture on a synthetic unit.
    let mut t = BTreeMap::new();
    t.insert(
        "m.py".to_string(),
        "import subprocess as sp\nfrom os import path as p, sep\n".to_string(),
    );
    let idx2 = PythonSyntaxIndex::index(&t);
    let imports = idx2
        .imports(&ModuleId::from_path("m.py").unwrap())
        .proven()
        .unwrap();
    assert!(imports.contains(&Import {
        target: "subprocess".into(),
        binding: "sp".into(),
        from_import: false,
        line: 1,
    }));
    assert!(imports.contains(&Import {
        target: "os.path".into(),
        binding: "p".into(),
        from_import: true,
        line: 2,
    }));
    assert!(imports.contains(&Import {
        target: "os.sep".into(),
        binding: "sep".into(),
        from_import: true,
        line: 2,
    }));
}

#[test]
fn formatting_only_change_preserves_signature_value() {
    let src_a = "def f(a: int, b: str = \"x\") -> bool:\n    return True\n";
    let src_b = "def f( a :  int ,   b : str=\"x\" ) -> bool :  # styled\n    return True\n";
    let mut t1 = BTreeMap::new();
    t1.insert("m.py".to_string(), src_a.to_string());
    let mut t2 = BTreeMap::new();
    t2.insert("m.py".to_string(), src_b.to_string());
    let i1 = PythonSyntaxIndex::index(&t1);
    let i2 = PythonSyntaxIndex::index(&t2);
    let s1 = i1
        .signature(&SymbolPath::new("m.f").unwrap())
        .proven()
        .flatten()
        .unwrap();
    let s2 = i2
        .signature(&SymbolPath::new("m.f").unwrap())
        .proven()
        .flatten()
        .unwrap();
    assert_eq!(s1.canonical(), s2.canonical());
    assert_eq!(s1, s2);
}

#[test]
fn true_signature_change_alters_value() {
    let src_a = "def f(a: int) -> bool:\n    return True\n";
    let src_b = "def f(a: int, c: str | None = None) -> bool:\n    return True\n";
    let mut t1 = BTreeMap::new();
    t1.insert("m.py".to_string(), src_a.to_string());
    let mut t2 = BTreeMap::new();
    t2.insert("m.py".to_string(), src_b.to_string());
    let i1 = PythonSyntaxIndex::index(&t1);
    let i2 = PythonSyntaxIndex::index(&t2);
    let s1 = i1
        .signature(&SymbolPath::new("m.f").unwrap())
        .proven()
        .flatten()
        .unwrap();
    let s2 = i2
        .signature(&SymbolPath::new("m.f").unwrap())
        .proven()
        .flatten()
        .unwrap();
    assert_ne!(s1, s2);
    assert_eq!(s2.canonical(), "(a: int, c: str | None = None) -> bool");
}

#[test]
fn malformed_python_yields_explicit_failed_status() {
    let mut t = BTreeMap::new();
    t.insert(
        "good.py".to_string(),
        "def ok() -> None:\n    pass\n".to_string(),
    );
    t.insert(
        "broken.py".to_string(),
        "def fragment(  # never closed\n".to_string(),
    );
    let idx = PythonSyntaxIndex::index(&t);
    // Broken unit is present with explicit Failed status...
    let status = idx
        .parse_status(&ModuleId::from_path("broken.py").unwrap())
        .proven()
        .unwrap();
    assert!(!status.is_ok());
    // ...and the good unit is unaffected.
    let status = idx
        .parse_status(&ModuleId::from_path("good.py").unwrap())
        .proven()
        .unwrap();
    assert!(status.is_ok());
    // The failure is queryable, never silent: the unit exists with status.
    assert_eq!(idx.unit_count(), 2);
}

#[test]
fn semantic_projections_never_return_authoritative_empty_sets() {
    let idx = index_fixture();
    let sym = SymbolPath::new("auth.tokens.refresh_token").unwrap();
    for answer in [
        // Type-erased check: every semantic query must be Unsupported.
        match idx.references(&sym) {
            crate::index::Answer::Proven(_) => false,
            crate::index::Answer::Unsupported(_) => true,
        },
        match idx.callers(&sym) {
            crate::index::Answer::Proven(_) => false,
            crate::index::Answer::Unsupported(_) => true,
        },
        match idx.implementations(&sym) {
            crate::index::Answer::Proven(_) => false,
            crate::index::Answer::Unsupported(_) => true,
        },
        match idx.subclasses(&sym) {
            crate::index::Answer::Proven(_) => false,
            crate::index::Answer::Unsupported(_) => true,
        },
    ] {
        assert!(
            answer,
            "semantic query must return Unsupported, never Proven(empty)"
        );
    }
}

#[test]
fn unsupported_unit_returns_explicit_unsupported_not_empty() {
    let idx = PythonSyntaxIndex::empty();
    let m = ModuleId::from_path("nope.py").unwrap();
    let answer = idx.definitions_in(&m);
    assert!(
        !answer.is_proven(),
        "unindexed unit must be Unsupported, not empty"
    );
}

#[test]
fn async_functions_and_defaults_and_splats() {
    let mut t = BTreeMap::new();
    t.insert(
        "m.py".to_string(),
        r#"
async def fetch(url: str, *, timeout: float = 1.5, **headers: str) -> None:
    pass

def combine(*args: int, sep: str = "-", **kw: str) -> str:
    return "-"
"#
        .to_string(),
    );
    let idx = PythonSyntaxIndex::index(&t);
    let f = idx
        .signature(&SymbolPath::new("m.fetch").unwrap())
        .proven()
        .flatten()
        .unwrap();
    assert!(f.is_async);
    assert_eq!(
        f.canonical(),
        "(url: str, *, timeout: float = 1.5, **headers: str) -> None"
    );
    let c = idx
        .signature(&SymbolPath::new("m.combine").unwrap())
        .proven()
        .flatten()
        .unwrap();
    assert!(!c.is_async);
    assert_eq!(
        c.canonical(),
        "(*args: int, sep: str = \"-\", **kw: str) -> str"
    );
}

#[test]
fn source_state_consumed_through_m1_abstractions() {
    // The fixture tree is read via trellis_source::read_tree (M1 layer);
    // this test documents the dependency direction mechanically: build a
    // manifest and reconcile, then index the reconciled tree — one
    // filesystem model only.
    use trellis_source::prelude::*;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/python_auth/base")
        .canonicalize()
        .unwrap();
    let options = ManifestOptions::new(root);
    let manifest = build_manifest(&options).expect("manifest");
    let tree = read_tree(&options).expect("tree");
    // Manifest digest must match a re-read (M1 determinism), and the
    // indexed unit set must be exactly the manifest's .py set.
    let manifest2 = build_manifest(&options).unwrap();
    assert_eq!(manifest.id(), manifest2.id());
    let idx = PythonSyntaxIndex::index(&tree);
    let py_units: Vec<String> = manifest
        .entries()
        .iter()
        .filter(|e| e.path.ends_with(".py"))
        .filter_map(|e| ModuleId::from_path(&e.path).map(|m| m.name().to_string()))
        .collect();
    assert_eq!(py_units.len(), idx.unit_count());
}

#[test]
fn changed_symbols_maps_changed_files_to_affected_symbols() {
    use trellis_source::prelude::*;
    // Reconcile a mutated fixture copy: change users/service.py only.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    // copy fixture
    copy_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/python_auth/base")
            .as_path(),
        &root,
    );
    let options = ManifestOptions::new(&root);
    let stored = build_manifest(&options).expect("manifest");
    let tree_before = read_tree(&options).expect("tree");

    // Mutate users/service.py in place.
    let svc = root.join("users/service.py");
    let content = std::fs::read_to_string(&svc).unwrap() + "\n# touched\n";
    std::fs::write(&svc, content).unwrap();

    let changed = reconcile_against_working_tree(&stored, &options).expect("reconcile");
    let changed_paths: Vec<String> = changed
        .added
        .iter()
        .chain(changed.modified.iter())
        .cloned()
        .collect();
    assert_eq!(changed_paths, vec!["users/service.py".to_string()]);

    let idx = PythonSyntaxIndex::index(&tree_before);
    let affected = idx
        .changed_symbols(&changed_paths)
        .proven()
        .expect("proven");
    assert!(
        affected.contains(&SymbolPath::new("users.service.UserService").unwrap()),
        "changed file's symbols must be candidates"
    );
    assert!(
        !affected.contains(&SymbolPath::new("auth.tokens.refresh_token").unwrap()),
        "unrelated file's symbols must not appear"
    );
}

#[test]
fn changed_symbols_degrades_when_a_changed_unit_failed_to_parse() {
    let mut t = BTreeMap::new();
    t.insert(
        "good.py".to_string(),
        "def ok() -> None:\n    pass\n".to_string(),
    );
    t.insert(
        "broken.py".to_string(),
        "def fragment(  # never closed\n".to_string(),
    );
    let idx = PythonSyntaxIndex::index(&t);
    let changed = vec!["good.py".to_string(), "broken.py".to_string()];
    let answer = idx.changed_symbols(&changed);
    assert!(
        !answer.is_proven(),
        "a failed-parse unit in the changed set must downgrade the answer"
    );
    // Parsed-only changed set stays proven.
    let answer = idx.changed_symbols(&["good.py".to_string()]);
    assert!(answer.is_proven());
}

#[test]
fn failed_parse_units_degrade_queries_to_unsupported_never_partial_proven() {
    // A definition AFTER the syntax error would be silently missing from a
    // partial extraction; the query surface must refuse instead.
    let mut t = BTreeMap::new();
    t.insert(
        "broken.py".to_string(),
        "def fragment(  # never closed\n\ndef later():\n    pass\n".to_string(),
    );
    let idx = PythonSyntaxIndex::index(&t);
    let m = ModuleId::from_path("broken.py").unwrap();
    // Parse status is explicitly Failed and queryable...
    let status = idx.parse_status(&m).proven().expect("status proven");
    assert!(!status.is_ok());
    // ...but definition/signature/definitions_in/imports degrade to Unsupported.
    assert!(!idx
        .definition(&SymbolPath::new("broken.later").unwrap())
        .is_proven());
    assert!(!idx
        .signature(&SymbolPath::new("broken.later").unwrap())
        .is_proven());
    assert!(!idx.definitions_in(&m).is_proven());
    assert!(!idx.imports(&m).is_proven());
    assert!(!idx.extract_definitions(&m).is_proven());
}

#[test]
fn file_digest_is_content_derived_and_deterministic() {
    let mut t = BTreeMap::new();
    t.insert("a.py".to_string(), "x = 1\n".to_string());
    let idx = PythonSyntaxIndex::index(&t);
    let d1 = idx.file_digest("a.py").proven().flatten().expect("digest");
    let d2 = idx.file_digest("a.py").proven().flatten().expect("digest");
    assert_eq!(d1, d2);
    assert!(d1.to_string().starts_with("blake3:"));
    // Formatting change alters the content digest (it is content, not syntax).
    let mut t2 = BTreeMap::new();
    t2.insert("a.py".to_string(), "x = 1  # comment\n".to_string());
    let idx2 = PythonSyntaxIndex::index(&t2);
    let d3 = idx2.file_digest("a.py").proven().flatten().unwrap();
    assert_ne!(d1, d3);
    // Absent file: proven None (clean syntactic absence).
    assert!(idx.file_digest("nope.py").proven().flatten().is_none());
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

#[test]
fn file_digest_out_of_universe_paths_are_unsupported_not_proven_absence() {
    let mut t = fixture_tree();
    t.insert("README.md".to_string(), "docs\n".to_string());
    let idx = PythonSyntaxIndex::index(&t);
    // A tracked non-Python file is outside the indexed universe — the
    // backend must refuse, never claim proven absence.
    assert!(!idx.file_digest("README.md").is_proven());
    assert!(!idx.file_digest("/abs/auth.py").is_proven());
    // Canonical .py absent from the tree IS proven absence (index built
    // from the full reconciled tree).
    assert!(idx.file_digest("nope.py").proven().flatten().is_none());
    // Present .py files keep content-derived digests.
    assert!(idx
        .file_digest("auth/tokens.py")
        .proven()
        .flatten()
        .is_some());
}

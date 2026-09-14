//! M8 freeze acceptance: authoritative semantic evaluation through the
//! transition with SCIP-frozen values, and the §6 freeze-rule scenarios
//! end-to-end over the oracle fixture. SCIP index construction here is
//! the harness stand-in for a real scip-python run (the adapter is
//! exercised for real in trellis-scip's own suite).

use scip::types::{
    Document, Index, Metadata, Occurrence, Relationship, SymbolInformation, ToolInfo,
};
use trellis_core::coverage::{Completeness, CompletenessEvaluator};
use trellis_core::projection::ProjectionKind;

use trellis_scip::prelude::*;
use trellis_scip::{freeze_semantic_snapshot, ScipIdentity};

const TOKENS: &str = "auth.tokens.refresh_token";
const HANDLE: &str = "api.webhooks.handle_refresh";

fn py_symbol(path: &str, scope: &[&str]) -> String {
    let mut s = format!("scip-python pip python_auth 1.0.0 `{path}`");
    for (i, part) in scope.iter().enumerate() {
        s.push('/');
        s.push_str(part);
        if i == scope.len() - 1 {
            s.push('.');
        }
    }
    s
}

fn def_occ(symbol: String, range: Vec<i32>) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = symbol;
    o.symbol_roles = 1;
    o.range = range;
    o
}

fn ref_occ(symbol: String, range: Vec<i32>) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = symbol;
    o.symbol_roles = 0;
    o.range = range;
    o
}

fn sym(symbol: &str) -> SymbolInformation {
    let mut si = SymbolInformation::new();
    si.symbol = symbol.to_string();
    si
}

fn implements(target: &str) -> Relationship {
    let mut r = Relationship::new();
    r.symbol = target.to_string();
    r.is_implementation = true;
    r
}

fn provider_symbol() -> String {
    py_symbol("auth/interfaces.py", &["AuthProvider"])
}

/// BASE-shaped index: refresh_token defined (external in this subset),
/// login defined with no callers, two AuthProvider implementations.
fn base_fixture_index() -> Index {
    let tokens_sym = py_symbol("auth/tokens.py", &["refresh_token"]);
    let login_sym = py_symbol("users/service.py", &["UserService", "login"]);
    let provider_sym = provider_symbol();
    let oidc_sym = py_symbol("auth/providers/oidc_provider.py", &["OidcAuthProvider"]);
    let password_sym = py_symbol(
        "auth/providers/password_provider.py",
        &["PasswordAuthProvider"],
    );

    let mut metadata = Metadata::new();
    let mut tool = ToolInfo::new();
    tool.name = "scip-python".to_string();
    tool.version = "9.9.9".to_string();
    metadata.tool_info = Some(tool).into();

    let mut doc_users = Document::new();
    doc_users.language = "python".to_string();
    doc_users.relative_path = "users/service.py".to_string();
    doc_users.occurrences = vec![def_occ(login_sym.clone(), vec![30, 8, 34, 70])];
    doc_users.symbols = vec![sym(&login_sym)];

    let mut doc_interfaces = Document::new();
    doc_interfaces.language = "python".to_string();
    doc_interfaces.relative_path = "auth/interfaces.py".to_string();
    doc_interfaces.occurrences = vec![def_occ(provider_sym.clone(), vec![7, 6, 12, 40])];
    doc_interfaces.symbols = vec![sym(&provider_sym)];

    let mut doc_oidc = Document::new();
    doc_oidc.language = "python".to_string();
    doc_oidc.relative_path = "auth/providers/oidc_provider.py".to_string();
    doc_oidc.occurrences = vec![def_occ(oidc_sym.clone(), vec![8, 6, 11, 50])];
    let mut si_oidc = sym(&oidc_sym);
    si_oidc.relationships = vec![implements(&provider_sym)];
    doc_oidc.symbols = vec![si_oidc];

    let mut doc_password = Document::new();
    doc_password.language = "python".to_string();
    doc_password.relative_path = "auth/providers/password_provider.py".to_string();
    doc_password.occurrences = vec![def_occ(password_sym.clone(), vec![8, 6, 11, 55])];
    let mut si_password = sym(&password_sym);
    si_password.relationships = vec![implements(&provider_sym)];
    doc_password.symbols = vec![si_password];

    let external_tokens = sym(&tokens_sym);

    Index {
        metadata: Some(metadata).into(),
        documents: vec![doc_users, doc_interfaces, doc_oidc, doc_password],
        external_symbols: vec![external_tokens],
        ..Default::default()
    }
}

/// R2-shaped index: api/webhooks.py added, calling refresh_token.
fn r2_index() -> Index {
    let mut index = base_fixture_index();
    let tokens_sym = py_symbol("auth/tokens.py", &["refresh_token"]);
    let handle_sym = py_symbol("api/webhooks.py", &["handle_refresh"]);

    let mut doc_api = Document::new();
    doc_api.language = "python".to_string();
    doc_api.relative_path = "api/webhooks.py".to_string();
    doc_api.occurrences = vec![
        def_occ(handle_sym.clone(), vec![5, 4, 9, 60]),
        ref_occ(tokens_sym.clone(), vec![7, 11, 7, 40]),
    ];
    doc_api.symbols = vec![sym(&handle_sym)];
    index.documents.push(doc_api);
    index
}

fn identity() -> ScipIdentity {
    ScipIdentity::new("scip-python", "9.9.9")
}

fn fixture_tree() -> std::collections::BTreeMap<String, String> {
    let root = trellis_oracle::fixture_base();
    let mut tree = std::collections::BTreeMap::new();
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

fn universe_of(tree: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    tree.keys().cloned().collect()
}

/// Fixture freeze: the authoritative callers query is exact against the
/// oracle R2 label; the certificate claims only what SCIP proves.
#[test]
fn fixture_freeze_callers_exact_vs_oracle() {
    let tree = fixture_tree();
    let paths = universe_of(&tree);
    let decision = freeze_semantic_snapshot(
        &r2_index(),
        &identity(),
        &["api/webhooks.py".to_string()],
        None,
        &paths,
    );
    let freeze = match decision {
        FreezeDecision::Freeze(f) => f,
        FreezeDecision::Reuse { .. } => panic!("python change must freeze"),
    };

    assert_eq!(
        freeze.graph.callers_value(TOKENS),
        HANDLE,
        "frozen callers must equal the oracle R2 expectation"
    );
    let cert = freeze.graph.coverage_certificate(&paths);
    // The harness index covers a subset of the fixture universe; every
    // unindexed eligible file must be an explicit failure (never a
    // silent gap that could look like proven absence).
    assert!(
        cert.failures.contains(&"auth/tokens.py".to_string()),
        "auth/tokens.py is outside this reduced index subset: {:?}",
        cert.failures.len()
    );
    assert_eq!(
        cert.all_units_participated,
        trellis_core::coverage::CoverageState::Failed
    );
}

/// §6 end-to-end: README-only change does NOT trigger a SCIP rerun; an
/// auth/*.py change does.
#[test]
fn freeze_rule_readme_vs_auth_change() {
    let tree = fixture_tree();
    let paths = universe_of(&tree);
    let index = base_fixture_index();

    let first = freeze_semantic_snapshot(&index, &identity(), &[], None, &paths);
    let frozen = match first {
        FreezeDecision::Freeze(f) => f,
        FreezeDecision::Reuse { .. } => panic!("anchor freeze expected"),
    };

    let reuse = freeze_semantic_snapshot(
        &index,
        &identity(),
        &["README.md".to_string()],
        Some(frozen.semantic_snapshot_id),
        &paths,
    );
    match reuse {
        FreezeDecision::Reuse {
            semantic_snapshot_id,
            ..
        } => {
            assert_eq!(semantic_snapshot_id, frozen.semantic_snapshot_id);
        }
        FreezeDecision::Freeze(_) => panic!("README change must reuse"),
    }

    let rerun = freeze_semantic_snapshot(
        &index,
        &identity(),
        &["auth/service.py".to_string()],
        Some(frozen.semantic_snapshot_id),
        &paths,
    );
    assert!(matches!(rerun, FreezeDecision::Freeze(_)));
}

/// Authoritative absence from the frozen BASE graph, then the R2 freeze
/// flipping the value — the M8 shape of the §33 first acceptance trace.
#[test]
fn frozen_semantic_values_drive_absence_then_invalidate() {
    let tree = fixture_tree();
    let paths = universe_of(&tree);

    let freeze =
        match freeze_semantic_snapshot(&base_fixture_index(), &identity(), &[], None, &paths) {
            FreezeDecision::Freeze(f) => f,
            FreezeDecision::Reuse { .. } => panic!("anchor freeze expected"),
        };
    assert_eq!(
        freeze.graph.callers_value(TOKENS),
        "",
        "BASE callers(refresh_token) is authoritative absence"
    );

    let r2_freeze = match freeze_semantic_snapshot(
        &r2_index(),
        &identity(),
        &["api/webhooks.py".to_string()],
        Some(freeze.semantic_snapshot_id),
        &paths,
    ) {
        FreezeDecision::Freeze(f) => f,
        FreezeDecision::Reuse { .. } => panic!("python change must freeze"),
    };
    assert_eq!(
        r2_freeze.graph.callers_value(TOKENS),
        HANDLE,
        "R2 freeze flips absence to the new caller"
    );
}

/// Freeze identity is deterministic for the same index + identity, and
/// the coverage evidence records the indexer identity (§8.1).
#[test]
fn freeze_identity_and_evidence_are_deterministic() {
    let tree = fixture_tree();
    let paths = universe_of(&tree);

    let a = freeze_semantic_snapshot(&base_fixture_index(), &identity(), &[], None, &paths);
    let b = freeze_semantic_snapshot(&base_fixture_index(), &identity(), &[], None, &paths);
    match (a, b) {
        (FreezeDecision::Freeze(x), FreezeDecision::Freeze(y)) => {
            assert_eq!(x.semantic_snapshot_id, y.semantic_snapshot_id);
            assert_eq!(x.indexer_identity, identity().render());
            assert_eq!(x.report.identity, identity().render());
        }
        _ => panic!("freeze expected"),
    }

    // Over the subset universe the index actually covers, the freeze
    // certificate satisfies completeness for semantic kinds. Over the
    // full fixture universe the honest verdict is Unknown (the reduced
    // harness index does not cover all ~35 files).
    let freeze =
        match freeze_semantic_snapshot(&base_fixture_index(), &identity(), &[], None, &paths) {
            FreezeDecision::Freeze(f) => f,
            FreezeDecision::Reuse { .. } => panic!("anchor freeze expected"),
        };
    let subset = vec![
        "users/service.py".to_string(),
        "auth/interfaces.py".to_string(),
        "auth/providers/oidc_provider.py".to_string(),
        "auth/providers/password_provider.py".to_string(),
    ];
    let cert_subset = freeze.graph.coverage_certificate(&subset);
    assert_eq!(
        CompletenessEvaluator::evaluate(ProjectionKind::Callers, &cert_subset),
        Completeness::Complete
    );
    let cert_full = freeze.graph.coverage_certificate(&paths);
    assert_eq!(
        CompletenessEvaluator::evaluate(ProjectionKind::Callers, &cert_full),
        Completeness::Unknown,
        "unindexed eligible files are explicit failures — never silent gaps"
    );
}

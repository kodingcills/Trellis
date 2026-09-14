//! M8 adapter tests: SCIP ingest normalization, certificate honesty,
//! freeze rule (§6 examples), determinism. SCIP knowledge confined to
//! this crate's tests.

use scip::types::{
    descriptor, Document, Index, Metadata, Occurrence, Relationship, SymbolInformation, ToolInfo,
};
use trellis_core::coverage::{Completeness, CompletenessEvaluator, CoverageState};
use trellis_core::projection::ProjectionKind;
use trellis_scip::prelude::*;
use trellis_scip::{classify_changes, ChangeClass, ScipIdentity};

fn descriptor(name: &str, suffix: descriptor::Suffix) -> scip::types::Descriptor {
    let mut d = scip::types::Descriptor::new();
    d.name = name.to_string();
    d.suffix = suffix.into();
    d
}

/// A scip-python-shaped symbol string: backtick-escaped file descriptor,
/// then the scope chain.
const PROVIDER: &str = "auth.interfaces.AuthProvider";

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

fn ref_occ(symbol: &str, range: Vec<i32>) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = symbol.to_string();
    o.symbol_roles = 0;
    o.range = range;
    o
}

fn def_occ(symbol: &str, range: Vec<i32>) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = symbol.to_string();
    o.symbol_roles = 0b0001;
    o.range = range;
    o
}

fn symbol_info(symbol: &str) -> SymbolInformation {
    let mut si = SymbolInformation::new();
    si.symbol = symbol.to_string();
    si
}

fn implements(relationship_target: &str) -> Relationship {
    let mut r = Relationship::new();
    r.symbol = relationship_target.to_string();
    r.is_implementation = true;
    r
}

const TOKENS: &str = "auth.tokens.refresh_token";

/// A minimal SCIP index over the fixture shape: api/webhooks.py and
/// users/alias_caller.py call refresh_token; oidc/password implement
/// AuthProvider.
fn fixture_index() -> Index {
    let tokens_sym = py_symbol("auth/tokens.py", &["refresh_token"]);
    let handle_sym = py_symbol("api/webhooks.py", &["handle_refresh"]);
    let rotate_sym = py_symbol("users/alias_caller.py", &["rotate_now"]);
    let provider_sym = py_symbol("auth/interfaces.py", &["AuthProvider"]);
    let oidc_sym = py_symbol("auth/providers/oidc_provider.py", &["OidcAuthProvider"]);
    let password_sym = py_symbol(
        "auth/providers/password_provider.py",
        &["PasswordAuthProvider"],
    );

    let mut metadata = Metadata::new();
    let mut tool = ToolInfo::new();
    tool.name = "scip-python".to_string();
    tool.version = "1.2.3".to_string();
    metadata.tool_info = Some(tool).into();

    let mut doc_api = Document::new();
    doc_api.language = "python".to_string();
    doc_api.relative_path = "api/webhooks.py".to_string();
    doc_api.occurrences = vec![
        def_occ(&handle_sym, vec![5, 4, 9, 60]),
        ref_occ(&tokens_sym, vec![7, 11, 7, 40]),
    ];
    doc_api.symbols = vec![symbol_info(&handle_sym)];

    let mut doc_alias = Document::new();
    doc_alias.language = "python".to_string();
    doc_alias.relative_path = "users/alias_caller.py".to_string();
    doc_alias.occurrences = vec![
        def_occ(&rotate_sym, vec![5, 4, 8, 50]),
        ref_occ(&tokens_sym, vec![7, 11, 7, 40]),
    ];
    doc_alias.symbols = vec![symbol_info(&rotate_sym)];

    let mut doc_interfaces = Document::new();
    doc_interfaces.language = "python".to_string();
    doc_interfaces.relative_path = "auth/interfaces.py".to_string();
    doc_interfaces.symbols = vec![symbol_info(&provider_sym)];

    let mut doc_oidc = Document::new();
    doc_oidc.language = "python".to_string();
    doc_oidc.relative_path = "auth/providers/oidc_provider.py".to_string();
    let mut si_oidc = symbol_info(&oidc_sym);
    si_oidc.relationships = vec![implements(&provider_sym)];
    doc_oidc.symbols = vec![si_oidc];

    let mut doc_password = Document::new();
    doc_password.language = "python".to_string();
    doc_password.relative_path = "auth/providers/password_provider.py".to_string();
    let mut si_password = symbol_info(&password_sym);
    si_password.relationships = vec![implements(&provider_sym)];
    doc_password.symbols = vec![si_password];

    // scip-python emits external_symbols for referenced-but-elsewhere-
    // defined symbols; the tokens symbol is defined in a document
    // outside this reduced index, so it appears here.
    let external_tokens = symbol_info(&tokens_sym);
    Index {
        metadata: Some(metadata).into(),
        documents: vec![doc_api, doc_alias, doc_interfaces, doc_oidc, doc_password],
        external_symbols: vec![external_tokens],
        ..Default::default()
    }
}

fn universe() -> Vec<String> {
    vec![
        "api/webhooks.py".to_string(),
        "users/alias_caller.py".to_string(),
        "auth/interfaces.py".to_string(),
        "auth/providers/oidc_provider.py".to_string(),
        "auth/providers/password_provider.py".to_string(),
    ]
}

fn identity() -> ScipIdentity {
    ScipIdentity::new("scip-python", "1.2.3")
}

#[test]
fn symbol_decoding_produces_dotted_subjects() {
    let (graph, report) = ingest(&fixture_index());
    assert_eq!(report.documents_ingested, 5);
    assert_eq!(report.documents_non_python, 0);
    assert!(
        graph.callers.contains_key(TOKENS),
        "callers keyed by dotted subject: {:?}",
        graph.callers.keys()
    );
    assert!(
        graph.implementations.contains_key(PROVIDER),
        "implementations keyed by dotted subject: {:?}",
        graph.implementations.keys()
    );
    // Descriptor helper sanity: namespace/terms decode as expected.
    let parsed = scip::symbol::parse_symbol(&py_symbol("auth/tokens.py", &["refresh_token"]))
        .expect("parses");
    assert_eq!(parsed.descriptors[0].name, "auth/tokens.py");
    let _ = descriptor("x", descriptor::Suffix::Term);
}

#[test]
fn caller_sets_exact_and_aliased_caller_detected() {
    let (graph, _) = ingest(&fixture_index());
    // R2+X1 shapes: both call sites are callers of refresh_token.
    assert_eq!(
        graph.callers_value(TOKENS),
        "api.webhooks.handle_refresh; users.alias_caller.rotate_now"
    );
    // X3 shape: two implementations.
    assert_eq!(
        graph.implementations_value(PROVIDER),
        "auth.providers.oidc_provider.OidcAuthProvider; auth.providers.password_provider.PasswordAuthProvider"
    );
}

#[test]
fn definition_site_is_never_a_member() {
    let (graph, report) = ingest(&fixture_index());
    let members = graph.callers.get(TOKENS).expect("caller set exists");
    assert!(
        !members.iter().any(|m| m == TOKENS),
        "definition site must never be a caller member"
    );
    assert_eq!(report.references_consumed, 2);
}

#[test]
fn certificate_is_honest_about_universe() {
    let (graph, _) = ingest(&fixture_index());
    let cert = graph.coverage_certificate(&universe());
    assert_eq!(cert.resolved_reference_coverage, CoverageState::Established);
    assert_eq!(cert.inheritance_relationships, CoverageState::Established);
    assert!(cert.failures.is_empty());

    // A file missing from the index is an explicit failure (never a
    // silent gap that could look like proven absence).
    let mut with_broken = universe();
    with_broken.push("users/broken.py".to_string());
    let degraded = graph.coverage_certificate(&with_broken);
    assert_eq!(degraded.failures, vec!["users/broken.py".to_string()]);
    assert_eq!(degraded.all_units_participated, CoverageState::Failed);
}

#[test]
fn evaluator_complete_for_full_scip_coverage() {
    let (graph, _) = ingest(&fixture_index());
    let cert = graph.coverage_certificate(&universe());
    assert_eq!(
        CompletenessEvaluator::evaluate(ProjectionKind::Callers, &cert),
        Completeness::Complete
    );
}

#[test]
fn classifier_is_conservative() {
    assert_eq!(
        classify_changes(&["README.md".into()]),
        ChangeClass::NonPython
    );
    assert_eq!(
        classify_changes(&["setup.cfg".into(), "docs/x.md".into()]),
        ChangeClass::NonPython
    );
    assert_eq!(
        classify_changes(&["auth/service.py".into()]),
        ChangeClass::PythonSource
    );
    assert_eq!(
        classify_changes(&["README.md".into(), "users/models.py".into()]),
        ChangeClass::PythonSource,
        "one Python file flips the classification"
    );
}

#[test]
fn freeze_identity_is_deterministic() {
    let index = fixture_index();
    let decision = freeze_semantic_snapshot(
        &index,
        &identity(),
        &["auth/service.py".to_string()],
        None,
        &universe(),
    );
    let decision2 = freeze_semantic_snapshot(
        &index,
        &identity(),
        &["auth/service.py".to_string()],
        None,
        &universe(),
    );
    match (decision, decision2) {
        (FreezeDecision::Freeze(a), FreezeDecision::Freeze(b)) => {
            assert_eq!(a.semantic_snapshot_id, b.semantic_snapshot_id);
            assert_eq!(a.graph, b.graph);
            assert_eq!(
                a.report.identity,
                "trellis-scip-ingest/1 (scip-python 1.2.3)"
            );
        }
        _ => panic!("freeze expected"),
    }
}

/// Regression (M8 review OPTIONAL closed in code): a module-level
/// reference (no enclosing def) in a dotted module must attribute the
/// reference to the FULL module (`auth.tokens`), not a truncated
/// parent (`auth`).
#[test]
fn module_level_reference_attributes_full_module() {
    let tokens_sym = py_symbol("auth/tokens.py", &["refresh_token"]);
    let mut doc = Document::new();
    doc.language = "python".to_string();
    doc.relative_path = "auth/tokens.py".to_string();
    // Module-level reference (line 1, no enclosing def).
    doc.occurrences = vec![ref_occ(&tokens_sym, vec![1, 0, 1, 20])];
    let index = minimal_index(vec![doc], Some(symbol_info(&tokens_sym)));
    let (graph, _) = ingest(&index);
    let refs = graph.references.get(TOKENS).expect("reference set exists");
    assert_eq!(
        refs,
        &["auth.tokens".to_string()],
        "module-level reference must attribute the full module"
    );
}

/// Minimal single-document index with the referenced symbol as an
/// external symbol.
fn minimal_index(documents: Vec<Document>, external: Option<SymbolInformation>) -> Index {
    let mut metadata = Metadata::new();
    let mut tool = ToolInfo::new();
    tool.name = "scip-python".to_string();
    tool.version = "9.9.9".to_string();
    metadata.tool_info = Some(tool).into();
    Index {
        metadata: Some(metadata).into(),
        documents,
        external_symbols: external.into_iter().collect(),
        ..Default::default()
    }
}

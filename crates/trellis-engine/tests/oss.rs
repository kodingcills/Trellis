//! M12 — pinned OSS external validation (spec §26, §25 D): a real OSS
//! Python repository (pip 26.2.1, vendored at a content-digest lock) is
//! a realism/integration fixture, NEVER a validity oracle. Measures:
//! indexing coverage at scale, reconciliation behavior at scale, and
//! absence-claim behavior on the partially-covered reality of a real
//! repo (M7 semantics: uncovered → UNKNOWN, never authoritative
//! absence).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use trellis_core::coverage::CoverageState;
use trellis_core::projection::ProjectionKind;

use trellis_program::prelude::{Answer, ModuleId, ProgramIndex, PythonSyntaxIndex};
use trellis_source::manifest::{build_manifest, ManifestOptions};
use trellis_source::reconcile::reconcile_against_working_tree;

#[path = "harness/mod.rs"]
mod harness;

/// Results anchor to the workspace-root benchmarks/ dir.
fn benchmarks_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("benchmarks")
}

use harness::*;

/// The pinned fixture root (fixtures/oss_pip, PIN.json lock).
fn oss_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/oss_pip")
}

/// Load the pinned tree (relative path → content), excluding PIN.json.
fn oss_tree() -> (BTreeMap<String, String>, String) {
    let root = oss_root();
    let pin_raw = std::fs::read_to_string(root.join("PIN.json")).expect("PIN readable");
    let digest = pin_raw
        .split("\"content_digest\":")
        .nth(1)
        .and_then(|s| s.split('"').nth(1))
        .unwrap_or_default()
        .to_string();
    let mut tree = BTreeMap::new();
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
                out.insert(rel, std::fs::read_to_string(&path).expect("readable"));
            }
        }
    }
    walk(&root, "", &mut tree);
    (tree, digest)
}

/// THE M12 measurement: index + reconcile at scale with the coverage
/// report committed; absence-claim semantics verified against the real
/// repo's partial coverage.
#[test]
fn oss_pinned_fixture_index_reconcile_at_scale() {
    let start = Instant::now();
    let (tree, pin_digest) = oss_tree();
    let file_count = tree.len();

    // ── Indexing at scale: coverage report inputs ────────────────────
    let index = PythonSyntaxIndex::index(&tree);
    let index_us = start.elapsed().as_micros();
    let mut parsed = 0usize;
    let mut parse_failed = 0usize;
    for path in tree.keys() {
        if let Some(module) = ModuleId::from_path(path) {
            if let Answer::Proven(status) = index.parse_status(&module) {
                if status.is_ok() {
                    parsed += 1;
                } else {
                    // Parse failures are explicit evidence, never
                    // silently omitted (spec §8; real repos contain
                    // syntactically imperfect files).
                    parse_failed += 1;
                }
            }
        }
    }

    // ── Reconciliation at scale: real add/modify/delete smoke ────────
    let work = tempfile::tempdir().expect("tempdir");
    let tree_root = work.path().join("tree");
    std::fs::create_dir_all(&tree_root).expect("tree root");
    write_tree(&tree_root, &tree);
    let manifest = build_manifest(&ManifestOptions::new(&tree_root)).expect("manifest builds");

    let modify_target = tree
        .keys()
        .find(|p| p.ends_with(".py") && !p.contains("__init__"))
        .expect("a .py file exists")
        .clone();
    std::fs::write(
        tree_root.join(&modify_target),
        format!(
            "\"\"\"M12 reconciliation smoke: modified.\n\"\"\"\n\n{}",
            tree[&modify_target]
        ),
    )
    .expect("writable");
    let added_path = "m12_added_smoke.py";
    std::fs::write(tree_root.join(added_path), "def m12_added():\n    pass\n").expect("writable");
    let delete_target = tree
        .keys()
        .find(|p| p.ends_with(".py") && !p.contains("__init__") && p.as_str() != modify_target)
        .expect("a deletable file")
        .clone();
    std::fs::remove_file(tree_root.join(&delete_target)).expect("removable");

    let t_reconcile = Instant::now();
    let changed = reconcile_against_working_tree(&manifest, &ManifestOptions::new(&tree_root))
        .expect("reconciles");
    let reconcile_us = t_reconcile.elapsed().as_micros();
    assert_eq!(changed.modified, vec![modify_target.clone()]);
    assert_eq!(changed.added, vec![added_path.to_string()]);
    assert_eq!(changed.removed, vec![delete_target.clone()]);

    // ── Absence-claim behavior on the partially-covered repo (M7) ────
    let mutated_tree = read_tree_from(&tree_root);
    let index2 = PythonSyntaxIndex::index(&mutated_tree);

    // Covered subset: the _internal/utils subtree with a certificate
    // over exactly that universe.
    let covered: Vec<String> = mutated_tree
        .keys()
        .filter(|p| p.starts_with("_internal/utils/"))
        .cloned()
        .collect();
    let cert_covered = index2.coverage_certificate(&covered);
    assert_eq!(
        cert_covered.resolved_reference_coverage,
        CoverageState::Unproven,
        "the syntax backend can never claim resolved-reference coverage \
         (§8.2 honesty floor; semantic authority arrives only via M8 SCIP)"
    );

    // Full-repo certificate: real repos contain units the reduced
    // backend does not fully cover; the certificate must report them
    // explicitly (never a silent gap that could look like proven
    // absence).
    let full: Vec<String> = mutated_tree.keys().cloned().collect();
    let cert_full = index2.coverage_certificate(&full);
    assert!(
        !cert_full.failures.is_empty()
            || cert_full.all_units_participated == CoverageState::Established,
        "coverage certificate must account for the full universe"
    );

    // Absence-claim verdicts under M7 semantics: semantic kinds over
    // the covered subset may be COMPLETE only if every capability the
    // certificate claims is Established — the syntax backend claims
    // Unproven resolution, so semantic kinds must answer UNKNOWN.
    let covered_verdict = trellis_core::coverage::CompletenessEvaluator::evaluate(
        ProjectionKind::Callers,
        &cert_covered,
    );
    assert_eq!(
        covered_verdict,
        trellis_core::coverage::Completeness::Unknown,
        "syntax-backend certificates cannot establish absence authority"
    );

    // ── Resource envelope committed ──────────────────────────────────
    let report = format!(
        "M12 — pinned OSS external validation (spec §26, §25 D)\n\
         ========================================================\n\
         fixture: pip 26.2.1 (vendored snapshot, MIT license)\n\
         pin: content digest {pin_digest}\n\
         files indexed: {file_count} (parsed {parsed}, parse-failed \
         {parse_failed} — explicit evidence, never silent)\n\
         \nresource envelope:\n\
         index (full vendored tree): {index_us}us\n\
         reconcile (add/modify/delete smoke): {reconcile_us}us\n\
         \ncoverage report:\n\
         covered subset (_internal/utils/*): {} files\n\
         syntax-backend certificate: resolution Unproven → semantic\n\
         kinds answer UNKNOWN over this repo (never authoritative\n\
         absence, §8.2; authoritative semantics arrive via the M8\n\
         SCIP freeze path over a real scip-python index)\n\
         \nNOT an oracle: this fixture validates realism/integration only;\n\
         upstream drift is handled by the content-digest pin alone.\n",
        covered.len(),
    );
    std::fs::create_dir_all(benchmarks_dir()).expect("benchmarks dir");
    std::fs::write(benchmarks_dir().join("oss_coverage_report.txt"), report)
        .expect("results written");
}

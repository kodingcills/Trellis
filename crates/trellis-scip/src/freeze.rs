//! Freeze rule + authoritative freeze production (spec §6, §9.1).
//!
//! The tightened freeze rule (§6): a freeze is required only when the
//! requested authoritative decision depends on semantic projections
//! whose current SCIP snapshot is **not provably compatible** with the
//! current source state. A change classifier proves compatibility:
//! changes to files that cannot affect the Python semantic universe
//! (non-Python files) leave the frozen snapshot applicable; any Python
//! source change potentially invalidates it.
//!
//! A freeze produces a [`SemanticFreeze`]: the deterministic
//! `SemanticSnapshotId`, the normalized graph, and the coverage
//! certificate SCIP proves over the eligible universe — everything an
//! M6/M7 transition needs for authoritative semantic evaluation.

use trellis_core::ids::SemanticSnapshotId;

use crate::ingest::{self, IngestReport};
use crate::ScipGraph;
use crate::{digest_of, ScipIdentity};

/// Which changes can affect the Python semantic universe (§6 change
/// classifier). The classifier is conservative: a Python-source change
/// is never provably compatible with an existing snapshot; non-Python
/// changes are provably compatible (they cannot add/remove/rename
/// Python symbols or references).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeClass {
    /// The changed files cannot affect the Python semantic universe.
    NonPython,
    /// At least one Python source file changed (or the Python universe
    /// itself changed): the frozen snapshot is not provably compatible.
    PythonSource,
}

/// Classify a reconciled changed-file set (§6 examples: `README.md` →
/// NonPython; `auth/service.py` → PythonSource).
pub fn classify_changes(changed_paths: &[String]) -> ChangeClass {
    if changed_paths.iter().any(|p| p.ends_with(".py")) {
        ChangeClass::PythonSource
    } else {
        ChangeClass::NonPython
    }
}

/// The frozen authoritative semantic state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticFreeze {
    /// Deterministic identity of the frozen semantic snapshot (§19).
    pub semantic_snapshot_id: SemanticSnapshotId,
    /// The normalized program graph Trellis owns.
    pub graph: ScipGraph,
    /// Coverage evidence (§8.1): indexer identity rendering.
    pub indexer_identity: String,
    /// Ingest accounting for the run log.
    pub report: IngestReport,
}

/// Whether a freeze is required and the fresh freeze when it runs.
pub enum FreezeDecision {
    /// The existing snapshot is provably compatible: no SCIP rerun
    /// (§6 example: SCIP @ S17 + README change → reuse S17).
    Reuse {
        /// The semantic snapshot that remains applicable.
        semantic_snapshot_id: SemanticSnapshotId,
        /// Why the classifier proved compatibility.
        reason: String,
    },
    /// A freeze is required; here is the fresh authoritative state.
    Freeze(SemanticFreeze),
}

/// Run the freeze rule: given the parsed SCIP index (fresh, from the
/// actual SCIP batch run), the identity it declares, the classified
/// changes, and the last frozen semantic snapshot id (if any), decide
/// whether authoritative semantics require a new freeze.
///
/// `fresh_index` is the *potential* freeze input: callers may run SCIP
/// unconditionally and let this function discard the result on
/// Reuse (v0.1 economics keep the adapter honest; incremental SCIP
/// reuse is deferred to M10's measured bottleneck path).
#[must_use]
pub fn freeze_semantic_snapshot(
    fresh_index: &scip::types::Index,
    identity: &ScipIdentity,
    changed_paths: &[String],
    last_frozen: Option<SemanticSnapshotId>,
    _eligible_universe: &[String],
) -> FreezeDecision {
    match classify_changes(changed_paths) {
        ChangeClass::NonPython => {
            if let Some(existing) = last_frozen {
                return FreezeDecision::Reuse {
                    semantic_snapshot_id: existing,
                    reason: "change classifier: no Python source files changed; \
                             the frozen semantic snapshot is provably compatible (§6)"
                        .to_string(),
                };
            }
            // No prior freeze: scheduled anchors always freeze (§6).
            FreezeDecision::Freeze(run_freeze(fresh_index, identity))
        }
        ChangeClass::PythonSource => FreezeDecision::Freeze(run_freeze(fresh_index, identity)),
    }
}

fn run_freeze(index: &scip::types::Index, identity: &ScipIdentity) -> SemanticFreeze {
    let (graph, report) = ingest::ingest(index);
    let semantic_snapshot_id = graph.semantic_snapshot_id(identity);
    SemanticFreeze {
        semantic_snapshot_id,
        graph,
        indexer_identity: identity.render(),
        report,
    }
}

/// The certificate a freeze establishes over the eligible universe
/// (§8.2) — re-exported from the graph for transition wiring.
pub fn freeze_certificate(
    graph: &ScipGraph,
    eligible_universe: &[String],
) -> trellis_core::coverage::CoverageCertificate {
    graph.coverage_certificate(eligible_universe)
}

/// Digest helper re-export for freeze-site evidence encoding.
pub fn freeze_digest(bytes: &[u8]) -> trellis_core::ids::ContentHash {
    digest_of(bytes)
}

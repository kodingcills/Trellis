//! trellis-scip — the authoritative SCIP freeze adapter (M8, spec §9.1,
//! §8.2, §6).
//!
//! This crate is the ONLY place in Trellis where SCIP-specific types and
//! knowledge may appear (spec §8.2, §24): SCIP is an input adapter, and
//! Trellis owns its normalized internal representation. Everything this
//! crate exports speaks Trellis types — semantic snapshots, canonical
//! caller/reference/implementation sets, and coverage certificates.
//!
//! Ingest honesty floor (§8.2): the certificate this adapter produces
//! claims exactly what SCIP proves for the ingested index —
//! resolved-reference and inheritance capabilities are Established
//! because SCIP relationships are precise; declaration inventory is
//! Established for units actually present in the index; units missing
//! from the index are explicit failures, never silent gaps.

use std::collections::BTreeMap;

use trellis_core::coverage::{CoverageCertificate, CoverageState};
use trellis_core::ids::{ContentHash, HashAlgo, SemanticSnapshotId};

pub mod freeze;
pub use crate::freeze::{
    classify_changes, freeze_semantic_snapshot, ChangeClass, FreezeDecision, SemanticFreeze,
};
pub mod ingest;

pub mod prelude {
    pub use crate::freeze::{freeze_semantic_snapshot, FreezeDecision, SemanticFreeze};
    pub use crate::ingest::{ingest, IngestReport};
    pub use crate::ScipGraph;
}

/// Domain-tagged BLAKE3 digest.
pub(crate) fn digest_of(bytes: &[u8]) -> ContentHash {
    ContentHash::compute(HashAlgo::Blake3, bytes)
}

/// The indexer identity recorded in coverage evidence (§8.1): adapter
/// version + tool name + tool version from the SCIP metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipIdentity {
    /// The SCIP tool that produced the index (e.g. "scip-python").
    pub tool: String,
    /// The tool's declared version.
    pub version: String,
    /// The adapter's own ingest version (bump on normalization changes).
    pub adapter: String,
}

impl ScipIdentity {
    /// The adapter ingest identity: bump when normalization semantics
    /// change (spec §19: implementation versions hash into identity).
    pub const ADAPTER_VERSION: &'static str = "trellis-scip-ingest/1";

    /// Identity from SCIP metadata fields.
    #[must_use]
    pub fn new(tool: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            version: version.into(),
            adapter: Self::ADAPTER_VERSION.to_string(),
        }
    }

    /// Canonical rendering for coverage evidence (§8.1).
    #[must_use]
    pub fn render(&self) -> String {
        format!("{} ({} {})", self.adapter, self.tool, self.version)
    }
}

/// The normalized semantic graph Trellis ingests a SCIP index into
/// (spec §9.3: Trellis owns the internal program graph). Canonical sets:
/// sorted, deduplicated, member form `<dotted module>.<dotted scope>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScipGraph {
    /// symbol → callers (precise call/reference resolution from SCIP).
    pub callers: BTreeMap<String, Vec<String>>,
    /// symbol → referencing modules/units (non-definition occurrences).
    pub references: BTreeMap<String, Vec<String>>,
    /// interface/trait symbol → implementing symbols (relationships).
    pub implementations: BTreeMap<String, Vec<String>>,
    /// interface/trait symbol → subclass symbols (relationships).
    pub subclasses: BTreeMap<String, Vec<String>>,
    /// Relative paths of the documents present in the index.
    pub documents: Vec<String>,
}

impl ScipGraph {
    /// The canonical value rendering for a Callers projection subject
    /// (dotted path form), or the empty string for proven absence —
    /// matching the fixture semantic source's encoding so M6/M7
    /// value-identity semantics hold across backends.
    #[must_use]
    pub fn callers_value(&self, dotted_symbol: &str) -> String {
        let members = self
            .callers
            .get(dotted_symbol)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        members.join("; ")
    }

    /// The canonical value rendering for an Implementations projection
    /// subject.
    #[must_use]
    pub fn implementations_value(&self, dotted_symbol: &str) -> String {
        let members = self
            .implementations
            .get(dotted_symbol)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        members.join("; ")
    }

    /// Deterministic semantic snapshot identity: BLAKE3 over the
    /// canonical graph encoding plus the indexer identity (§19: same
    /// graph + same backend → same SemanticSnapshotId).
    #[must_use]
    pub fn semantic_snapshot_id(&self, identity: &ScipIdentity) -> SemanticSnapshotId {
        let mut material = String::from("trellis.semantic-snapshot.v1");
        material.push('\u{1f}');
        material.push_str(&identity.render());
        for (subject, members) in &self.callers {
            material.push('\u{1f}');
            material.push_str("callers:");
            material.push_str(subject);
            material.push('=');
            material.push_str(&members.join(","));
        }
        for (subject, members) in &self.references {
            material.push('\u{1f}');
            material.push_str("references:");
            material.push_str(subject);
            material.push('=');
            material.push_str(&members.join(","));
        }
        for (subject, members) in &self.implementations {
            material.push('\u{1f}');
            material.push_str("implementations:");
            material.push_str(subject);
            material.push('=');
            material.push_str(&members.join(","));
        }
        for (subject, members) in &self.subclasses {
            material.push('\u{1f}');
            material.push_str("subclasses:");
            material.push_str(subject);
            material.push('=');
            material.push_str(&members.join(","));
        }
        for doc in &self.documents {
            material.push('\u{1f}');
            material.push_str("doc:");
            material.push_str(doc);
        }
        SemanticSnapshotId::from_hash(digest_of(material.as_bytes()))
    }

    /// The coverage certificate this adapter can honestly establish for
    /// the given eligible universe (§8.2): capabilities SCIP proves for
    /// the ingested index; every eligible unit absent from the index is
    /// an explicit failure (never a silent gap that could look like
    /// proven absence).
    #[must_use]
    pub fn coverage_certificate(&self, universe_paths: &[String]) -> CoverageCertificate {
        let indexed: std::collections::BTreeSet<&String> = self.documents.iter().collect();
        let failures: Vec<String> = universe_paths
            .iter()
            .filter(|p| !indexed.contains(*p))
            .cloned()
            .collect();
        if failures.is_empty() {
            CoverageCertificate {
                universe_complete: CoverageState::Established,
                all_units_participated: CoverageState::Established,
                declaration_inventory_complete: CoverageState::Established,
                resolved_reference_coverage: CoverageState::Established,
                inheritance_relationships: CoverageState::Established,
                failures: Vec::new(),
            }
        } else {
            CoverageCertificate {
                universe_complete: CoverageState::Established,
                all_units_participated: CoverageState::Failed,
                declaration_inventory_complete: CoverageState::Failed,
                resolved_reference_coverage: CoverageState::Failed,
                inheritance_relationships: CoverageState::Failed,
                failures,
            }
        }
    }
}

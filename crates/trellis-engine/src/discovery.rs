//! Conservative dirty-candidate discovery (spec §18 steps 1-3).
//!
//! Given a reconcile [`ChangedSet`], determine which stored
//! [`ProjectionObservation`]s **may** be affected. Soundness rule: never
//! omit a potentially affected observation; over-approximation is allowed
//! and documented.
//!
//! Discovery classes — every observation is anchored to its recording-time
//! source unit (`observation_anchors`, the file→observations reverse
//! index); a change to that unit selects the observation:
//!
//! 1. **File-scoped** (`FileContent`): subject IS the changed path.
//! 2. **Unit-anchored** (`Definition`, `Signature`, `Imports`): the
//!    observation's anchor path is in the changed set, OR the module/symbol
//!    inventory changed (any added or removed `.py` file): M3 resolves
//!    symbols by longest indexed module prefix and modules can alias
//!    (`m.py` vs `m/__init__.py`), so inventory changes can rebind which
//!    unit backs a symbol or a file-digest. Anchors are recorded at
//!    observation time and persisted (M4), so a moved/removed symbol stays
//!    anchored to its historical file. No module-name→path derivation
//!    happens here — M3 owns module identity; M5 consumes the recorded
//!    anchor.
//! 3. **Completeness-sensitive** (`Callers`, `References`,
//!    `Implementations`, `Subclasses`, `RepositorySearch`): candidates on
//!    **any** file-level change — a modified file can add/remove call
//!    sites; added/removed files change the eligible universe.
//! 4. **Conservative fallback** (all remaining kinds, e.g. `ConfigValue`,
//!    `ToolVersion`): inputs not yet modeled → candidate on any
//!    file-level change. Never assumed unaffected.
//!
//! An empty changed set selects nothing.
//!
//! **Semantics (M5 boundary):** membership means *may be affected* — the
//! projection value must be reevaluated before any conclusion. It does NOT
//! mean the value changed (M6's value-equality comparison) and it does NOT
//! mean any artifact is stale (M6's contract evaluation).

use std::collections::BTreeSet;

use trellis_core::ids::ProjectionObservationId;
use trellis_core::projection::{Projection, ProjectionKind};
use trellis_program::prelude::ModuleId;
use trellis_source::reconcile::ChangedSet;

/// A recorded observation plus its full canonical projection key and
/// recording-time source anchor, as persisted by [`crate::observe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedObservation {
    /// The full canonical projection key (kind, subject, property, scope).
    pub projection: Projection,
    /// The persisted observation.
    pub observation: trellis_core::projection::ProjectionObservation,
    /// The source unit the observation was evaluated against.
    pub anchor_path: String,
}

/// A recorded symbol location (symbol→file reverse-index row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolLocation {
    /// Canonical symbol path.
    pub symbol: String,
    /// The file that defined it at recording time.
    pub source_path: String,
}

/// The conservative reevaluation candidate set.
///
/// **Semantics (M5 boundary):** membership means *may be affected* — the
/// projection value must be reevaluated before any conclusion. It does NOT
/// mean the value changed (that is M6's value-equality comparison) and it
/// does NOT mean any artifact is stale (that is M6's contract evaluation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReevaluationCandidates {
    observations: BTreeSet<ProjectionObservationId>,
}

impl ReevaluationCandidates {
    /// Candidate observation ids in deterministic (canonical id) order.
    #[must_use]
    pub fn ids(&self) -> Vec<ProjectionObservationId> {
        self.observations.iter().copied().collect()
    }

    /// Number of candidates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.observations.len()
    }

    /// Whether there are no candidates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }

    /// Whether a specific observation is a candidate.
    #[must_use]
    pub fn contains(&self, id: &ProjectionObservationId) -> bool {
        self.observations.contains(id)
    }

    fn insert(&mut self, id: ProjectionObservationId) {
        self.observations.insert(id);
    }

    fn from_ids(ids: impl IntoIterator<Item = ProjectionObservationId>) -> Self {
        Self {
            observations: ids.into_iter().collect(),
        }
    }
}

/// Whether a projection kind makes completeness/absence claims and
/// therefore reacts to eligible-universe or symbol-inventory changes.
#[must_use]
pub fn is_completeness_sensitive(kind: ProjectionKind) -> bool {
    kind.is_completeness_sensitive()
}

/// Discover reevaluation candidates for a changed-file set over the
/// recorded observations (with persisted anchors).
///
/// Deterministic: candidates are deduplicated and ordered by canonical
/// observation id; the changed-set iteration order does not influence the
/// result. Survives restart: all inputs come from M4 persistence.
#[must_use]
pub fn reevaluation_candidates(
    changed: &ChangedSet,
    recorded: &[RecordedObservation],
) -> ReevaluationCandidates {
    let mut changed_paths: BTreeSet<&str> = BTreeSet::new();
    for p in changed
        .added
        .iter()
        .chain(&changed.modified)
        .chain(&changed.removed)
    {
        changed_paths.insert(p.as_str());
    }
    let universe_changed = !changed_paths.is_empty();

    let mut candidates = ReevaluationCandidates::from_ids(std::iter::empty());
    for rec in recorded {
        let subject_changed: bool = match rec.projection.kind() {
            // File-scoped: subject IS the path.
            ProjectionKind::FileContent => {
                changed_paths.contains(rec.projection.subject().canonical())
            }
            // Unit-anchored: the recorded source unit changed, OR the
            // module/symbol inventory changed (add/remove can rebind M3's
            // longest-prefix symbol resolution and module aliasing).
            ProjectionKind::Definition | ProjectionKind::Signature | ProjectionKind::Imports => {
                // Inventory change = a canonical Python unit added/removed
                // (non-Python files cannot rebind program resolution).
                let inventory_changed = changed
                    .added
                    .iter()
                    .chain(&changed.removed)
                    .any(|p| ModuleId::from_path(p).is_some());
                changed_paths.contains(rec.anchor_path.as_str()) || inventory_changed
            }
            // Completeness/absence-bearing kinds: any file-level change can
            // alter call sites, references, implementors, or the eligible
            // universe. Conservative.
            kind if is_completeness_sensitive(kind) => universe_changed,
            // Conservative fallback: kinds whose inputs are not yet modeled
            // (ConfigValue, ToolVersion, …) are candidates on any
            // file-level change. Never assumed unaffected.
            _ => universe_changed,
        };
        if subject_changed {
            candidates.insert(rec.observation.id());
        }
    }
    candidates
}

/// Load recorded observations from persistence. **Fail closed**: an
/// observation whose descriptor or anchor row is missing is corrupt
/// persisted state — an explicit error, never a silently omitted
/// observation (spec §8; discovery must never lose a candidate).
///
/// # Errors
/// [`trellis_store::StoreError::Corrupt`] for missing descriptors or
/// anchors; other persistence failures propagate.
pub fn recorded_from_store(
    store: &trellis_store::Store,
) -> Result<Vec<RecordedObservation>, trellis_store::StoreError> {
    let observations = store.all_projection_observations()?;
    let mut out = Vec::with_capacity(observations.len());
    for obs in &observations {
        let (kind, variant, subject, scope) = store.projection_descriptor(&obs.projection())?;
        let anchor = store.observation_anchor(&obs.id())?;
        let kind = parse_kind(&kind)?;
        let scope = parse_scope(&scope)?;
        let projection = Projection::new(
            kind,
            subject_from_variant(kind, &variant, &subject)?,
            property_of(kind),
            scope,
        )?;
        // Fail closed: a reconstructed descriptor that does not hash to
        // the observation's projection id is corrupt persisted state —
        // consuming it would silently change discovery classification
        // (spec §8; identity must round-trip, spec §19).
        if projection.id() != obs.projection() {
            return Err(trellis_store::StoreError::Corrupt(format!(
                "descriptor for {} does not hash to the recorded projection id",
                obs.projection()
            )));
        }
        out.push(RecordedObservation {
            projection,
            observation: obs.clone(),
            anchor_path: anchor,
        });
    }
    Ok(out)
}

fn parse_kind(s: &str) -> Result<ProjectionKind, trellis_store::StoreError> {
    use ProjectionKind::*;
    Ok(match s {
        "file" => FileContent,
        "definition" => Definition,
        "signature" => Signature,
        "references" => References,
        "callers" => Callers,
        "implementations" => Implementations,
        "imports" => Imports,
        "subclasses" => Subclasses,
        "repository_search" => RepositorySearch,
        "config_value" => ConfigValue,
        "tool_version" => ToolVersion,
        _ => {
            return Err(trellis_store::StoreError::Corrupt(format!(
                "unknown projection kind {s}"
            )))
        }
    })
}

fn parse_scope(s: &str) -> Result<trellis_core::projection::Scope, trellis_store::StoreError> {
    use trellis_core::projection::Scope;
    Ok(match s {
        "File" => Scope::File,
        "Module" => Scope::Module,
        "Package" => Scope::Package,
        "Repository" => Scope::Repository,
        _ => {
            return Err(trellis_store::StoreError::Corrupt(format!(
                "unknown projection scope {s}"
            )))
        }
    })
}

/// Reconstruct the exact Subject from the persisted variant + payload.
/// The variant is identity-bearing (it participates in `Projection::id`),
/// so it must round-trip exactly — inferring it from kind would break
/// legal cross-variant projections (spec §7, §19).
fn subject_from_variant(
    _kind: ProjectionKind,
    variant: &str,
    subject: &str,
) -> Result<trellis_core::projection::Subject, trellis_store::StoreError> {
    use trellis_core::projection::Subject;
    Ok(match variant {
        "File" => Subject::File(subject.to_string()),
        "Symbol" => Subject::Symbol(subject.to_string()),
        "Module" => Subject::Module(subject.to_string()),
        "Text" => Subject::Text(subject.to_string()),
        "ConfigKey" => Subject::ConfigKey(subject.to_string()),
        "Tool" => Subject::Tool(subject.to_string()),
        _ => {
            return Err(trellis_store::StoreError::Corrupt(format!(
                "unknown subject variant {variant}"
            )))
        }
    })
}

fn property_of(kind: ProjectionKind) -> trellis_core::projection::Property {
    trellis_core::projection::Property::for_kind(kind)
        .expect("every kind has a consistent property")
}

//! Projection-observation production (M3 amendment: deferred from M3 to
//! M5 — observation recording requires snapshot context).
//!
//! Each helper evaluates one syntactic projection — supplied as the full
//! M0 [`Projection`] (kind, subject, property, scope) so the observation's
//! projection identity is the **canonical M0 identity**, never a reduced
//! local reimplementation — against a [`PythonSyntaxIndex`] and, when the
//! backend **proves** a value, wraps it in a [`ProjectionObservation`]
//! bound to the given snapshot. Value digests are computed over
//! Trellis-canonical encodings (spec §19).
//!
//! Semantic projection kinds (Callers/References/Implementations/
//! Subclasses/RepositorySearch) are **not** producible by the M3 syntax
//! backend: the answer is [`Answer::Unsupported`], so no observation is
//! produced and nothing synthetic is recorded (M5 boundary 6).

use trellis_core::ids::{ContentHash, SnapshotId};
use trellis_core::projection::{Projection, ProjectionKind, ProjectionObservation};

use trellis_program::prelude::{Answer, ProgramIndex, PythonSyntaxIndex};

/// Observation-production errors. Explicit, never silent (spec §8).
#[derive(Debug, PartialEq, Eq)]
pub enum ObserveError {
    /// The supplied projection's kind does not match the helper's query
    /// family — associating one kind's identity with another kind's value
    /// would corrupt the observation record.
    KindMismatch {
        /// The kind the helper evaluates.
        expected: ProjectionKind,
        /// The kind the caller supplied.
        got: ProjectionKind,
    },
}

impl std::fmt::Display for ObserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObserveError::KindMismatch { expected, got } => {
                write!(f, "kind mismatch: expected {expected:?}, got {got:?}")
            }
        }
    }
}

impl std::error::Error for ObserveError {}

fn check_kind(projection: &Projection, expected: ProjectionKind) -> Result<(), ObserveError> {
    if projection.kind() == expected {
        Ok(())
    } else {
        Err(ObserveError::KindMismatch {
            expected,
            got: projection.kind(),
        })
    }
}

/// Canonical value digest for a proven answer: BLAKE3 over the canonical
/// value rendering.
fn digest_of(canonical: &str) -> ContentHash {
    ContentHash::compute(trellis_core::ids::HashAlgo::Blake3, canonical.as_bytes())
}

pub use trellis_core::observation_id::canonical_observation_id;

fn observation(
    projection: &Projection,
    snapshot: SnapshotId,
    value_canonical: &str,
) -> ProjectionObservation {
    let value_hash = digest_of(value_canonical);
    ProjectionObservation::new(
        canonical_observation_id(&projection.id(), &value_hash, &snapshot),
        projection.id(),
        snapshot,
        value_hash,
        None,
    )
}

/// An observation together with the authoritative source unit it was
/// evaluated against, as resolved by M3. `resolved_source_path` is the
/// path that MUST be persisted as the observation's discovery anchor —
/// M5 never re-derives module→path rules and M6 never reconstructs
/// resolution semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedObservation {
    // Private: external code can neither construct nor mutate these — the
    // observation identity and the authoritative resolved anchor are
    // established exclusively by the observe helpers (M5 review cycle 4:
    // a caller-authored anchor would create a false negative).
    observation: ProjectionObservation,
    resolved_source_path: String,
}

impl ObservedObservation {
    /// The produced observation.
    #[must_use]
    pub fn observation(&self) -> &ProjectionObservation {
        &self.observation
    }

    /// M3's authoritative current source unit for the evaluation.
    #[must_use]
    pub fn resolved_source_path(&self) -> &str {
        &self.resolved_source_path
    }
}

/// Observe a file content digest (`file(path)` projection).
pub fn observe_file_digest(
    index: &PythonSyntaxIndex,
    projection: &Projection,
    snapshot: SnapshotId,
) -> Result<Option<ObservedObservation>, ObserveError> {
    check_kind(projection, ProjectionKind::FileContent)?;
    let path = projection.subject().canonical();
    Ok(match index.file_digest(path) {
        Answer::Proven(Some(digest)) => Some(ObservedObservation {
            observation: observation(projection, snapshot, &digest.to_string()),
            resolved_source_path: path.to_string(),
        }),
        // Proven-absent (canonical .py removed) or out-of-universe paths
        // produce no observation: removal is surfaced by the reconcile
        // changed-set, not by an observation value.
        _ => None,
    })
}

/// Observe a symbol's syntactic definition (`definition(symbol)`).
pub fn observe_definition(
    index: &PythonSyntaxIndex,
    projection: &Projection,
    snapshot: SnapshotId,
) -> Result<Option<ObservedObservation>, ObserveError> {
    check_kind(projection, ProjectionKind::Definition)?;
    let Some(symbol) = trellis_program::prelude::SymbolPath::new(projection.subject().canonical())
    else {
        return Ok(None);
    };
    let Some((module, resolved)) = index.resolve_symbol_unit(&symbol) else {
        return Ok(None);
    };
    Ok(match index.definition(&symbol) {
        Answer::Proven(Some(def)) => {
            let canonical = format!("{:?}@{}", def.kind, def.symbol);
            Some(ObservedObservation {
                observation: observation(projection, snapshot, &canonical),
                resolved_source_path: resolved,
            })
        }
        // Proven absence for a cleanly parsed unit is a valid observation
        // value too: an absent definition is real evidence.
        Answer::Proven(None) => Some(ObservedObservation {
            observation: observation(projection, snapshot, "absent"),
            resolved_source_path: index.unit_path(&module).unwrap_or_default().to_string(),
        }),
        _ => None,
    })
}

/// Observe a symbol's normalized signature (`signature(symbol)`).
pub fn observe_signature(
    index: &PythonSyntaxIndex,
    projection: &Projection,
    snapshot: SnapshotId,
) -> Result<Option<ObservedObservation>, ObserveError> {
    check_kind(projection, ProjectionKind::Signature)?;
    let Some(symbol) = trellis_program::prelude::SymbolPath::new(projection.subject().canonical())
    else {
        return Ok(None);
    };
    let Some((module, resolved)) = index.resolve_symbol_unit(&symbol) else {
        return Ok(None);
    };
    Ok(match index.signature(&symbol) {
        Answer::Proven(Some(sig)) => Some(ObservedObservation {
            observation: observation(projection, snapshot, &sig.canonical()),
            resolved_source_path: resolved,
        }),
        Answer::Proven(None) => Some(ObservedObservation {
            observation: observation(projection, snapshot, "absent"),
            resolved_source_path: index.unit_path(&module).unwrap_or_default().to_string(),
        }),
        _ => None,
    })
}

/// Observe a module's syntactic imports (`imports(module, scope)`) as a
/// canonical sorted set. Line numbers are excluded: they are formatting,
/// not semantics.
pub fn observe_imports(
    index: &PythonSyntaxIndex,
    projection: &Projection,
    snapshot: SnapshotId,
) -> Result<Option<ObservedObservation>, ObserveError> {
    check_kind(projection, ProjectionKind::Imports)?;
    let Some(module) = trellis_program::prelude::ModuleId::from_path(&format!(
        "{}.py",
        projection.subject().canonical().replace('.', "/")
    )) else {
        return Ok(None);
    };
    let Some(imports) = index.imports(&module).proven() else {
        return Ok(None);
    };
    let mut canonical: Vec<String> = imports
        .iter()
        .map(|i| format!("{} as {} ({})", i.target, i.binding, i.from_import))
        .collect();
    canonical.sort();
    canonical.dedup();
    let Some(resolved) = index.unit_path(&module).map(str::to_string) else {
        return Ok(None);
    };
    Ok(Some(ObservedObservation {
        observation: observation(projection, snapshot, &canonical.join("; ")),
        resolved_source_path: resolved,
    }))
}

/// Record one observation's full discovery state in the sanctioned order:
/// the observation row, its projection descriptor, and its source anchor.
/// Discovery state must never be partially persisted — a missing
/// descriptor or anchor fails closed at discovery time (spec §8).
///
/// `anchor_path` is the source unit the observation was evaluated against.
///
/// # Errors
/// Persistence failures.
pub fn record_observation(
    store: &mut trellis_store::Store,
    projection: &Projection,
    observed: &ObservedObservation,
) -> Result<(), trellis_store::StoreError> {
    // Single atomic store operation: observation + descriptor + anchor in
    // one transaction, with identity/canonical-id/anchor validation inside
    // the store (never partially persisted).
    store.record_observation_record(
        projection,
        observed.observation(),
        observed.resolved_source_path(),
    )
}

/// Record the defining source unit of every symbol extracted from a unit
/// (symbol→file reverse index). Composite-PK idempotent: all historical
/// locations are preserved. Returns the number of locations recorded.
///
/// # Errors
/// Persistence failures.
pub fn record_unit_index(
    index: &PythonSyntaxIndex,
    module: &trellis_program::prelude::ModuleId,
    source_path: &str,
    store: &mut trellis_store::Store,
) -> Result<usize, trellis_store::StoreError> {
    let defs = match index.extract_definitions(module) {
        Answer::Proven(defs) => defs,
        // Failed-parse units have no trustworthy locations; the failure is
        // already explicit via parse_status (never silent, spec §8).
        _ => return Ok(0),
    };
    let mut n = 0;
    for (def, _sig) in defs {
        store.put_symbol_location(def.symbol.as_str(), module.name(), source_path)?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use trellis_core::ids::HashAlgo;
    use trellis_core::projection::Scope;

    fn tree_with(content: &str) -> BTreeMap<String, String> {
        let mut t = BTreeMap::new();
        t.insert("m.py".to_string(), content.to_string());
        t
    }

    #[test]
    fn definition_observation_present_and_absent_are_distinct_values() {
        let t = tree_with("def f() -> None:\n    pass\n");
        let idx = PythonSyntaxIndex::index(&t);
        let snap = SnapshotId::from_hash(ContentHash::compute(HashAlgo::Blake3, b"s1"));
        let p_f = Projection::definition("m.f").unwrap();
        let p_g = Projection::definition("m.g").unwrap();
        let present = observe_definition(&idx, &p_f, snap)
            .unwrap()
            .unwrap()
            .observation;
        let absent = observe_definition(&idx, &p_g, snap)
            .unwrap()
            .unwrap()
            .observation;
        assert_ne!(present.value_digest(), absent.value_digest());
    }

    #[test]
    fn signature_observation_changes_with_true_change_only() {
        let snap = SnapshotId::from_hash(ContentHash::compute(HashAlgo::Blake3, b"s1"));
        let p = Projection::signature("m.f").unwrap();
        let t1 = tree_with("def f(a: int) -> bool:\n    return True\n");
        let t2 = tree_with("def f( a : int ) -> bool :  # styled\n    return True\n");
        let t3 = tree_with("def f(a: int, c: str | None = None) -> bool:\n    return True\n");
        let s1 = observe_signature(&PythonSyntaxIndex::index(&t1), &p, snap)
            .unwrap()
            .unwrap()
            .observation;
        let s2 = observe_signature(&PythonSyntaxIndex::index(&t2), &p, snap)
            .unwrap()
            .unwrap()
            .observation;
        let s3 = observe_signature(&PythonSyntaxIndex::index(&t3), &p, snap)
            .unwrap()
            .unwrap()
            .observation;
        assert_eq!(s1.value_digest(), s2.value_digest(), "formatting-only");
        assert_ne!(s1.value_digest(), s3.value_digest(), "true change");
    }

    #[test]
    fn imports_observation_excludes_line_numbers() {
        let snap = SnapshotId::from_hash(ContentHash::compute(HashAlgo::Blake3, b"s1"));
        let p = Projection::imports("m", Scope::Repository).unwrap();
        let t1 = tree_with("import os\nimport sys\n");
        let t2 = tree_with("\n\nimport os\nimport sys\n");
        let i1 = observe_imports(&PythonSyntaxIndex::index(&t1), &p, snap)
            .unwrap()
            .unwrap()
            .observation;
        let i2 = observe_imports(&PythonSyntaxIndex::index(&t2), &p, snap)
            .unwrap()
            .unwrap()
            .observation;
        assert_eq!(i1.value_digest(), i2.value_digest(), "position shift only");
    }

    #[test]
    fn semantic_kinds_produce_no_observations() {
        // Boundary: the M3 backend cannot prove callers etc., so no
        // observation is manufactured (M5 boundary 6).
        let t = tree_with("def f():\n    pass\n");
        let idx = PythonSyntaxIndex::index(&t);
        let symbol = trellis_program::prelude::SymbolPath::new("m.f").unwrap();
        let answer = ProgramIndex::callers(&idx, &symbol);
        assert!(!answer.is_proven());
    }

    #[test]
    fn observation_identity_is_deterministic_and_scope_sensitive() {
        let t = tree_with("def f() -> None:\n    pass\n");
        let idx = PythonSyntaxIndex::index(&t);
        let snap = SnapshotId::from_hash(ContentHash::compute(HashAlgo::Blake3, b"s1"));
        let p = Projection::definition("m.f").unwrap();
        let o1 = observe_definition(&idx, &p, snap)
            .unwrap()
            .unwrap()
            .observation;
        let o2 = observe_definition(&idx, &p, snap)
            .unwrap()
            .unwrap()
            .observation;
        assert_eq!(o1.id(), o2.id());
        // Same value at a different snapshot → different observation.
        let snap2 = SnapshotId::from_hash(ContentHash::compute(HashAlgo::Blake3, b"s2"));
        let o3 = observe_definition(&idx, &p, snap2)
            .unwrap()
            .unwrap()
            .observation;
        assert_ne!(o1.id(), o3.id());
        // Same subject, different scope → different projection id →
        // different observation (full M0 identity participates).
        let p_pkg = Projection::new(
            ProjectionKind::Definition,
            trellis_core::projection::Subject::Symbol("m.f".into()),
            trellis_core::projection::Property::Definition,
            Scope::Package,
        )
        .unwrap();
        assert_ne!(p.id(), p_pkg.id());
        let o4 = observe_definition(&idx, &p_pkg, snap)
            .unwrap()
            .unwrap()
            .observation;
        assert_ne!(o1.id(), o4.id());
    }

    #[test]
    fn wrong_kind_projection_refused_explicitly() {
        let t = tree_with("def f() -> None:\n    pass\n");
        let idx = PythonSyntaxIndex::index(&t);
        let snap = SnapshotId::from_hash(ContentHash::compute(HashAlgo::Blake3, b"s1"));
        let wrong = Projection::signature("m.f").unwrap();
        let err = observe_definition(&idx, &wrong, snap).unwrap_err();
        assert_eq!(
            err,
            ObserveError::KindMismatch {
                expected: ProjectionKind::Definition,
                got: ProjectionKind::Signature,
            }
        );
        let wrong2 = Projection::file("m.f").unwrap();
        assert!(observe_signature(&idx, &wrong2, snap).is_err());
        assert!(observe_imports(&idx, &wrong2, snap).is_err());
        assert!(observe_file_digest(&idx, &Projection::definition("m.f").unwrap(), snap).is_err());
    }
}

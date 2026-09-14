//! Universe and coverage certificates (M7, spec §8).
//!
//! Absence and completeness claims are only as sound as the universe
//! they were evaluated over: `callers(x) = {}` is a claim about a query
//! **and** the universe it covered **and** the evidence that coverage
//! held. Every completeness-sensitive observation therefore binds to
//! `(Q, U, C)`.
//!
//! Backend independence is structural (Pin 2):
//! - [`ProjectionKind::requirements`] declares the *proof obligations* a
//!   backend must satisfy for a query of that kind to establish absence
//!   or completeness — no backend-specific knowledge lives here.
//! - [`CoverageCertificate`] declares the *capabilities* a backend
//!   actually established for one snapshot over one universe. It is
//!   produced by the backend, never by the engine.
//! - [`CompletenessEvaluator`] checks `Requirements(Q) ⊆ Capabilities(C)`
//!   and answers [`Completeness::Complete`] or
//!   [`Completeness::Unknown`] — nothing else.

use crate::ids::ContentHash;
use crate::projection::ProjectionKind;

/// Domain-tagged BLAKE3 digest over canonical bytes.
pub fn digest_of(bytes: &[u8]) -> ContentHash {
    ContentHash::compute(crate::ids::HashAlgo::Blake3, bytes)
}

/// The explicit eligible universe a completeness-sensitive query was
/// evaluated over (spec §8.1: set of eligible units). The descriptor is
/// canonical content: sorted relative paths of eligible units, so its
/// digest is deterministic and machine-independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Universe {
    /// Eligible source units (canonical relative paths), sorted.
    paths: Vec<String>,
}

impl Universe {
    /// Build a universe from eligible paths. Duplicates collapse; order
    /// is canonicalized.
    #[must_use]
    pub fn new(paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut unique: Vec<String> = paths.into_iter().map(Into::into).collect();
        unique.sort();
        unique.dedup();
        Self { paths: unique }
    }

    /// Eligible paths in canonical order.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Canonical digest: BLAKE3 over domain-tagged sorted paths.
    #[must_use]
    pub fn digest(&self) -> ContentHash {
        let mut material = String::from("trellis.universe.v1");
        for path in &self.paths {
            material.push('\u{1f}');
            material.push_str(path);
        }
        digest_of(material.as_bytes())
    }
}

/// Whether a specific coverage capability was established, is known to
/// be violated, or was never proven (§8.2). `Unproven` never satisfies a
/// required capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageState {
    /// The backend established the capability.
    Established,
    /// The backend proved the capability is violated.
    Failed,
    /// The backend has no evidence either way.
    Unproven,
}

/// The capabilities one semantic backend established for one snapshot
/// over one universe (spec §8.2). Produced by the backend adapter; the
/// engine never fabricates one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageCertificate {
    /// Every eligible unit in the universe was enumerated and indexed.
    pub universe_complete: CoverageState,
    /// Every relevant semantic unit participated in the analysis.
    pub all_units_participated: CoverageState,
    /// The declaration inventory of every unit is complete (cross-
    /// validation evidence only — never proof of reference resolution).
    pub declaration_inventory_complete: CoverageState,
    /// Call/reference edges were resolved (not merely enumerated).
    pub resolved_reference_coverage: CoverageState,
    /// Inheritance/subclass relationships are known.
    pub inheritance_relationships: CoverageState,
    /// Units with known indexing failures (path → reason); any entry
    /// makes scope-affecting capabilities non-establishable.
    pub failures: Vec<String>,
}

impl CoverageCertificate {
    /// The certificate a fully-capable backend produces over a fully
    /// indexed universe.
    #[must_use]
    pub fn complete() -> Self {
        Self {
            universe_complete: CoverageState::Established,
            all_units_participated: CoverageState::Established,
            declaration_inventory_complete: CoverageState::Established,
            resolved_reference_coverage: CoverageState::Established,
            inheritance_relationships: CoverageState::Established,
            failures: Vec::new(),
        }
    }

    /// A certificate from a backend that failed to index the given
    /// units: participation and reference resolution are explicitly
    /// Failed for the affected scope (§8: a failed unit can never look
    /// like proven absence).
    #[must_use]
    pub fn with_failed_units(units: &[String]) -> Self {
        Self {
            universe_complete: CoverageState::Established,
            all_units_participated: CoverageState::Failed,
            declaration_inventory_complete: CoverageState::Failed,
            resolved_reference_coverage: CoverageState::Failed,
            inheritance_relationships: CoverageState::Unproven,
            failures: units.to_vec(),
        }
    }

    /// Canonical digest over every field (§8.1 coverage digest).
    #[must_use]
    pub fn digest(&self) -> ContentHash {
        let state = |s: CoverageState| match s {
            CoverageState::Established => "established",
            CoverageState::Failed => "failed",
            CoverageState::Unproven => "unproven",
        };
        let mut material = String::from("trellis.coverage-certificate.v1");
        for field in [
            state(self.universe_complete),
            state(self.all_units_participated),
            state(self.declaration_inventory_complete),
            state(self.resolved_reference_coverage),
            state(self.inheritance_relationships),
        ] {
            material.push('\u{1f}');
            material.push_str(field);
        }
        for failure in &self.failures {
            material.push('\u{1f}');
            material.push_str("failure:");
            material.push_str(failure);
        }
        digest_of(material.as_bytes())
    }
}

/// Whether a query's completeness requirements are satisfied by a
/// certificate (§8.2): the only two honest answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    /// Requirements(Q) ⊆ Capabilities(C): an empty result is potentially
    /// authoritative absence.
    Complete,
    /// Requirements unsatisfied: an empty result is UNKNOWN, never
    /// authoritative absence.
    Unknown,
}

/// Backend-independent completeness evaluator (Pin 2).
pub struct CompletenessEvaluator;

impl CompletenessEvaluator {
    /// `Requirements(Q) ⊆ Capabilities(C) → COMPLETE, otherwise UNKNOWN`.
    #[must_use]
    pub fn evaluate(kind: ProjectionKind, certificate: &CoverageCertificate) -> Completeness {
        if !kind.is_completeness_sensitive() {
            // Non-completeness-sensitive kinds make no absence claims.
            return Completeness::Complete;
        }
        // Requirements(Q) for every completeness-sensitive kind, per
        // spec §8.2 (Callers example generalized):
        //   1. eligible universe fully enumerated
        //   2. every relevant semantic unit participated
        //   3. resolved-call-reference capability available
        //   4. no unresolved coverage failures affecting query scope
        let requirements = [
            certificate.universe_complete,
            certificate.all_units_participated,
            certificate.resolved_reference_coverage,
        ];
        let failures_absent = certificate.failures.is_empty();
        if requirements
            .iter()
            .all(|s| *s == CoverageState::Established)
            && failures_absent
        {
            Completeness::Complete
        } else {
            Completeness::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_digest_is_order_and_duplicate_insensitive() {
        let a = Universe::new(["b.py", "a.py"]);
        let b = Universe::new(["a.py", "b.py", "a.py"]);
        assert_eq!(a, b);
        assert_eq!(a.digest(), b.digest());
        let c = Universe::new(["a.py", "c.py"]);
        assert_ne!(a.digest(), c.digest());
    }

    #[test]
    fn certificate_digest_covers_every_field() {
        let base = CoverageCertificate::complete();
        assert_eq!(base.digest(), CoverageCertificate::complete().digest());
        let mut degraded = CoverageCertificate::complete();
        degraded.resolved_reference_coverage = CoverageState::Unproven;
        assert_ne!(base.digest(), degraded.digest());
        let mut with_failure = CoverageCertificate::complete();
        with_failure.failures = vec!["users/broken.py".to_string()];
        assert_ne!(base.digest(), with_failure.digest());
    }

    #[test]
    fn evaluator_satisfies_only_complete_certificates() {
        for kind in [
            ProjectionKind::Callers,
            ProjectionKind::References,
            ProjectionKind::Implementations,
            ProjectionKind::Subclasses,
            ProjectionKind::RepositorySearch,
        ] {
            assert_eq!(
                CompletenessEvaluator::evaluate(kind, &CoverageCertificate::complete()),
                Completeness::Complete
            );
            assert_eq!(
                CompletenessEvaluator::evaluate(
                    kind,
                    &CoverageCertificate::with_failed_units(&["users/broken.py".into()])
                ),
                Completeness::Unknown
            );
            let mut unproven = CoverageCertificate::complete();
            unproven.resolved_reference_coverage = CoverageState::Unproven;
            assert_eq!(
                CompletenessEvaluator::evaluate(kind, &unproven),
                Completeness::Unknown
            );
        }
        // Non-completeness-sensitive kinds make no absence claims.
        assert_eq!(
            CompletenessEvaluator::evaluate(
                ProjectionKind::Signature,
                &CoverageCertificate::with_failed_units(&["x.py".into()])
            ),
            Completeness::Complete
        );
    }
}

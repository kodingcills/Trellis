//! Artifacts and derivations (spec §10, §13, §16).
//!
//! An artifact is an immutable value with a provenance trail — never a
//! mutable truth container. Validity, authority, and verification level
//! live on attestations (spec §12); `CaptureTrust` originates here, on the
//! derivation, and propagates from there.

use std::fmt;

use crate::error::DomainError;
use crate::ids::{ArtifactId, BlobId, DerivationId, ProjectionId, SnapshotId, Timestamp};
use crate::validity::{capture_trust_from_channels, CaptureChannel, CaptureTrust};

/// The externally meaningful kind of an artifact (spec §10). Kind defines
/// the *interface* of the validity contract; verifiers implement it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactKind {
    /// A canonical structural set (caller set, reference set, …).
    StructuralSet,
    /// A recorded observation of the world.
    Observation,
    /// A fact claim.
    Fact,
    /// A fact derived from other artifacts.
    DerivedFact,
    /// A natural-language summary.
    Summary,
    /// A hypothesis — surfaced as "previously considered", never trusted
    /// reuse (spec §13, Class D).
    Hypothesis,
    /// A plan.
    Plan,
    /// A recorded execution result.
    ExecutionResult,
    /// A patch. Never replayed after repository changes in v0.1.
    Patch,
}

impl ArtifactKind {
    /// The artifact reuse class (spec §13). Class and capture trust are
    /// orthogonal: a class describes what kind of claim the artifact makes,
    /// capture trust describes how well its derivation was observed.
    #[must_use]
    pub const fn reuse_class(self) -> ReuseClass {
        match self {
            ArtifactKind::StructuralSet => ReuseClass::DeterministicStructural,
            ArtifactKind::Observation => ReuseClass::DeterministicStructural,
            ArtifactKind::Fact => ReuseClass::VerifiedFact,
            ArtifactKind::DerivedFact => ReuseClass::VerifiedFact,
            ArtifactKind::Summary => ReuseClass::ModelDerived,
            ArtifactKind::Hypothesis => ReuseClass::Hypothesis,
            ArtifactKind::Plan => ReuseClass::Plan,
            ArtifactKind::ExecutionResult => ReuseClass::Historical,
            ArtifactKind::Patch => ReuseClass::Historical,
        }
    }
}

/// Artifact reuse classes (spec §13, Classes A–F).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReuseClass {
    /// Class A — deterministic structural artifacts; auto-reusable when
    /// dependencies are green and completeness policies hold.
    DeterministicStructural,
    /// Class B — deterministically verified facts; reusable while the
    /// attached verifier remains valid.
    VerifiedFact,
    /// Class C — model-derived facts; candidates only, require sufficient
    /// authority/verification or explicit consumer policy.
    ModelDerived,
    /// Class D — hypotheses; never trusted reuse.
    Hypothesis,
    /// Class E — plans; parts may serve as reference material.
    Plan,
    /// Class F — patches/execution results; historical outputs only.
    Historical,
}

/// A normalized claim the artifact makes, if it is a propositional claim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Proposition(String);

impl Proposition {
    /// Construct a proposition; rejects empty/whitespace text.
    pub fn new(text: impl Into<String>) -> Result<Self, DomainError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(DomainError::EmptyField("proposition"));
        }
        Ok(Self(text))
    }

    /// The proposition text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Proposition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Provenance of the producing agent/harness (spec §50: provider, model,
/// versions). M0 records the strings; richer structured provenance lands
/// with the model adapter phase.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProducerInfo {
    harness: String,
    version: String,
    model: Option<String>,
}

impl ProducerInfo {
    /// Construct producer info; `harness` and `version` must be non-empty.
    pub fn new(
        harness: impl Into<String>,
        version: impl Into<String>,
        model: Option<String>,
    ) -> Result<Self, DomainError> {
        let harness = harness.into();
        let version = version.into();
        if harness.trim().is_empty() {
            return Err(DomainError::EmptyField("producer.harness"));
        }
        if version.trim().is_empty() {
            return Err(DomainError::EmptyField("producer.version"));
        }
        Ok(Self {
            harness,
            version,
            model,
        })
    }

    /// The harness that produced the artifact.
    #[must_use]
    pub fn harness(&self) -> &str {
        &self.harness
    }

    /// Harness version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Model identifier, if model-derived.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

/// Cost accounting for an artifact's production (spec §63: cold-run
/// overhead must be measurable). All fields optional; zero-cost is the
/// default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CostRecord {
    /// Capture cost in milliseconds.
    pub capture_ms: Option<u64>,
    /// Validation cost in milliseconds.
    pub validation_ms: Option<u64>,
    /// Recomputation cost in milliseconds.
    pub recompute_ms: Option<u64>,
    /// Model tokens consumed, if any.
    pub tokens: Option<u64>,
}

/// How an artifact was produced (spec §16: derivation = provenance, never
/// correctness). Owns the [`CaptureTrust`] that propagates into every
/// attestation of artifacts derived here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Derivation {
    id: DerivationId,
    channels: Vec<CaptureChannel>,
    capture_trust: CaptureTrust,
    producer: ProducerInfo,
    created_at: Timestamp,
}

impl Derivation {
    /// Construct a derivation from the observation channels that
    /// participated. Capture trust is **computed**, not claimed: it is the
    /// weakest participating channel (spec §13). No channels at all means
    /// nothing was captured → [`CaptureTrust::Unobserved`].
    pub fn new(
        id: DerivationId,
        channels: Vec<CaptureChannel>,
        producer: ProducerInfo,
        created_at: Timestamp,
    ) -> Self {
        let trust = capture_trust_from_channels(&channels);
        Self {
            id,
            channels,
            capture_trust: trust,
            producer,
            created_at,
        }
    }

    /// Identity of this derivation.
    #[must_use]
    pub const fn id(&self) -> DerivationId {
        self.id
    }

    /// The capture channels that participated.
    #[must_use]
    pub fn channels(&self) -> &[CaptureChannel] {
        &self.channels
    }

    /// The derived capture trust (weakest participating channel).
    #[must_use]
    pub const fn capture_trust(&self) -> CaptureTrust {
        self.capture_trust
    }

    /// Producer provenance.
    #[must_use]
    pub const fn producer(&self) -> &ProducerInfo {
        &self.producer
    }

    /// Creation time (unix epoch millis, UTC).
    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }
}

/// The immutable artifact envelope (spec §10).
///
/// **Note what is absent:** no validity, authority, or verification fields.
/// Those belong to [`crate::attestation::ValidationAttestation`]. All fields
/// are private with read-only accessors: artifacts are immutable by
/// construction (spec §2.6).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactEnvelope {
    id: ArtifactId,
    schema_version: u32,
    kind: ArtifactKind,
    payload_ref: BlobId,
    proposition: Option<Proposition>,
    producer: ProducerInfo,
    derivation: Derivation,
    dependencies: Vec<ProjectionId>,
    created_snapshot: SnapshotId,
    cost: CostRecord,
}

impl ArtifactEnvelope {
    /// Construct an artifact envelope.
    ///
    /// # Invariants
    /// - The dependency list is stored **sorted and deduplicated**;
    ///   construction normalizes rather than trusting the caller.
    /// - The proposition is required for propositional kinds (`Fact`,
    ///   `DerivedFact`, `Hypothesis`) and rejected as empty elsewhere.
    /// - `id` is caller-supplied in M0; M1 derives it from canonical
    ///   serialization (`H(schema, kind, canonical_payload)`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ArtifactId,
        schema_version: u32,
        kind: ArtifactKind,
        payload_ref: BlobId,
        proposition: Option<Proposition>,
        producer: ProducerInfo,
        derivation: Derivation,
        dependencies: Vec<ProjectionId>,
        created_snapshot: SnapshotId,
        cost: CostRecord,
    ) -> Result<Self, DomainError> {
        if matches!(
            kind,
            ArtifactKind::Fact | ArtifactKind::DerivedFact | ArtifactKind::Hypothesis
        ) && proposition.is_none()
        {
            return Err(DomainError::EmptyField("proposition"));
        }
        if let Some(p) = &proposition {
            if p.as_str().trim().is_empty() {
                return Err(DomainError::EmptyField("proposition"));
            }
        }
        let mut deps = dependencies;
        deps.sort_unstable();
        deps.dedup();
        Ok(Self {
            id,
            schema_version,
            kind,
            payload_ref,
            proposition,
            producer,
            derivation,
            dependencies: deps,
            created_snapshot,
            cost,
        })
    }

    /// Content identity of this artifact.
    #[must_use]
    pub const fn id(&self) -> ArtifactId {
        self.id
    }

    /// Schema version of the envelope and payload.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// The artifact kind.
    #[must_use]
    pub const fn kind(&self) -> ArtifactKind {
        self.kind
    }

    /// The artifact's reuse class (kind-derived).
    #[must_use]
    pub const fn reuse_class(&self) -> ReuseClass {
        self.kind.reuse_class()
    }

    /// CAS blob holding the canonical payload.
    #[must_use]
    pub const fn payload_ref(&self) -> BlobId {
        self.payload_ref
    }

    /// The proposition, if this is a propositional artifact.
    #[must_use]
    pub const fn proposition(&self) -> Option<&Proposition> {
        self.proposition.as_ref()
    }

    /// Producer provenance.
    #[must_use]
    pub const fn producer(&self) -> &ProducerInfo {
        &self.producer
    }

    /// The derivation — how this artifact was produced.
    #[must_use]
    pub const fn derivation(&self) -> &Derivation {
        &self.derivation
    }

    /// The capture trust inherited from the derivation.
    #[must_use]
    pub const fn capture_trust(&self) -> CaptureTrust {
        self.derivation.capture_trust()
    }

    /// Dependency projections, sorted and deduplicated.
    #[must_use]
    pub fn dependencies(&self) -> &[ProjectionId] {
        &self.dependencies
    }

    /// The snapshot this artifact was created against.
    #[must_use]
    pub const fn created_snapshot(&self) -> SnapshotId {
        self.created_snapshot
    }

    /// Production cost record.
    #[must_use]
    pub const fn cost(&self) -> CostRecord {
        self.cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ContentHash, HashAlgo};
    use std::str::FromStr;

    fn h(seed: u8) -> ContentHash {
        let d: Vec<u8> = (0..32).map(|i| seed.wrapping_add(i)).collect();
        ContentHash::from_bytes(HashAlgo::Blake3, &d).unwrap()
    }

    fn producer() -> ProducerInfo {
        ProducerInfo::new("bench-agent", "0.1.0", Some("test-model".to_string())).unwrap()
    }

    fn derivation() -> Derivation {
        Derivation::new(
            DerivationId::from_hash(h(20)),
            vec![CaptureChannel::TrellisTool],
            producer(),
            100,
        )
    }

    fn envelope(
        kind: ArtifactKind,
        deps: Vec<ProjectionId>,
        prop: Option<Proposition>,
    ) -> ArtifactEnvelope {
        ArtifactEnvelope::new(
            ArtifactId::from_hash(h(30)),
            1,
            kind,
            BlobId::from_hash(h(31)),
            prop,
            producer(),
            derivation(),
            deps,
            SnapshotId::from_hash(h(32)),
            CostRecord::default(),
        )
        .unwrap()
    }

    #[test]
    fn dependencies_are_sorted_and_deduplicated() {
        let a = ProjectionId::from_hash(h(1));
        let b = ProjectionId::from_hash(h(2));
        let c = ProjectionId::from_hash(h(3));
        let art = envelope(ArtifactKind::StructuralSet, vec![c, a, b, a], None);
        assert_eq!(art.dependencies(), &[a, b, c]);
    }

    #[test]
    fn capture_trust_propagates_from_derivation() {
        let art = envelope(ArtifactKind::StructuralSet, vec![], None);
        assert_eq!(art.capture_trust(), CaptureTrust::Complete);

        let unobserved = Derivation::new(
            DerivationId::from_hash(h(21)),
            vec![CaptureChannel::TrellisTool, CaptureChannel::Unobserved],
            producer(),
            100,
        );
        let art2 = ArtifactEnvelope::new(
            ArtifactId::from_hash(h(30)),
            1,
            ArtifactKind::StructuralSet,
            BlobId::from_hash(h(31)),
            None,
            producer(),
            unobserved,
            vec![],
            SnapshotId::from_hash(h(32)),
            CostRecord::default(),
        )
        .unwrap();
        assert_eq!(art2.capture_trust(), CaptureTrust::Unobserved);
        assert!(!crate::validity::default_auto_reuse(
            crate::validity::Validity::Valid,
            crate::validity::Authority::Authoritative,
            art2.capture_trust(),
        ));
    }

    #[test]
    fn propositional_kinds_require_proposition() {
        let res = ArtifactEnvelope::new(
            ArtifactId::from_hash(h(30)),
            1,
            ArtifactKind::Fact,
            BlobId::from_hash(h(31)),
            None,
            producer(),
            derivation(),
            vec![],
            SnapshotId::from_hash(h(32)),
            CostRecord::default(),
        );
        assert!(matches!(
            res.unwrap_err(),
            DomainError::EmptyField("proposition")
        ));

        let ok = envelope(
            ArtifactKind::Fact,
            vec![],
            Some(Proposition::new("normalize_email is idempotent").unwrap()),
        );
        assert_eq!(ok.kind(), ArtifactKind::Fact);
        assert_eq!(ok.reuse_class(), ReuseClass::VerifiedFact);
        assert_eq!(
            ok.proposition().unwrap().to_string(),
            "normalize_email is idempotent"
        );
        assert_eq!(ok.derivation().capture_trust(), CaptureTrust::Complete);
    }

    #[test]
    fn empty_proposition_text_rejected() {
        assert!(Proposition::new("   ").is_err());
    }

    #[test]
    fn producer_info_validated() {
        assert!(ProducerInfo::new("", "1.0", None).is_err());
        assert!(ProducerInfo::new("agent", " ", None).is_err());
        let p = ProducerInfo::new("agent", "1.0", Some("model".to_string())).unwrap();
        assert_eq!(p.model(), Some("model"));
    }

    #[test]
    fn ids_roundtrip_through_display() {
        let art = envelope(ArtifactKind::StructuralSet, vec![], None);
        assert_eq!(
            ArtifactId::from_str(&art.id().to_string()).unwrap(),
            art.id()
        );
        assert_eq!(
            DerivationId::from_str(&art.derivation().id().to_string()).unwrap(),
            art.derivation().id()
        );
    }
}

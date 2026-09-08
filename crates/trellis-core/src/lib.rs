//! trellis-core — the Trellis domain model and its invariants (spec §4).
//!
//! This crate implements **M0 — domain invariants** of the frozen
//! implementation plan: IDs, artifacts, derivations, projections, snapshots,
//! validation attestations, and the epistemic enums, with every frozen
//! invariant enforced by construction.
//!
//! Reference: *Trellis Architecture & Technical Design Report — Draft 1.0
//! (Frozen)* in the repository root. Section numbers in doc comments refer
//! to that document.
//!
//! Core architecture in one paragraph: artifacts are immutable values with
//! derivations (provenance); dependency projections name the world-state
//! functions an artifact's value depends on; validation attestations form an
//! append-only, snapshot-scoped history of an artifact's validity and
//! authority; reuse policy is a function of all four epistemic dimensions —
//! validity, authority, capture trust, verification level — never one enum.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod artifact;
pub mod attestation;
pub mod error;
pub mod ids;
pub mod projection;
pub mod snapshot;
pub mod validity;

/// Everything a consumer of the domain model typically needs.
pub mod prelude {
    pub use crate::artifact::{
        ArtifactEnvelope, ArtifactKind, CostRecord, Derivation, ProducerInfo, Proposition,
        ReuseClass,
    };
    pub use crate::attestation::{AttestationHistory, EvidenceRef, ValidationAttestation};
    pub use crate::error::DomainError;
    pub use crate::ids::{
        ArtifactId, AttestationId, BlobId, ContentHash, DerivationId, EnvironmentFingerprintId,
        GitOid, HashAlgo, ManifestId, ProjectionId, ProjectionObservationId, RepositoryId,
        SemanticSnapshotId, SnapshotId, Timestamp, VerifierId,
    };
    pub use crate::projection::{
        Projection, ProjectionKind, ProjectionObservation, Property, Scope, Subject,
    };
    pub use crate::snapshot::Snapshot;
    pub use crate::validity::{
        capture_trust_from_channels, default_auto_reuse, Authority, CaptureChannel, CaptureTrust,
        EngineState, Validity, VerificationLevel,
    };
}

#[cfg(test)]
mod tests {
    /// The nucleus scenario from spec §33, expressed as domain invariants:
    /// an absence artifact whose attestation goes stale when its caller-set
    /// dependency changes, then a caller-body mutation that must *not*
    /// propagate (value-equality cutoff).
    #[test]
    fn acceptance_trace_domain_shape() {
        use crate::prelude::*;
        use std::str::FromStr;

        // R0: callers(refresh_token) = ∅, COMPLETE, authoritative.
        let callers = Projection::callers("app.auth.refresh_token", Scope::Repository).unwrap();
        assert!(callers.kind().is_completeness_sensitive());
        let artifact = ArtifactEnvelope::new(
            ArtifactId::from_hash(
                ContentHash::from_str(&format!("blake3:{}", "11".repeat(32))).unwrap(),
            ),
            1,
            ArtifactKind::Fact,
            BlobId::from_hash(
                ContentHash::from_str(&format!("blake3:{}", "22".repeat(32))).unwrap(),
            ),
            Some(Proposition::new("refresh_token has no external callers").unwrap()),
            ProducerInfo::new("bench-agent", "0.1.0", None).unwrap(),
            Derivation::new(
                DerivationId::from_hash(
                    ContentHash::from_str(&format!("blake3:{}", "33".repeat(32))).unwrap(),
                ),
                vec![CaptureChannel::TrellisTool],
                ProducerInfo::new("bench-agent", "0.1.0", None).unwrap(),
                1_000,
            ),
            vec![ProjectionId::from_hash(
                ContentHash::from_str(&format!("blake3:{}", "44".repeat(32))).unwrap(),
            )],
            SnapshotId::from_hash(
                ContentHash::from_str(&format!("blake3:{}", "55".repeat(32))).unwrap(),
            ),
            CostRecord::default(),
        )
        .unwrap();

        // S0 attestation: VALID / AUTHORITATIVE.
        let mut history = AttestationHistory::new(artifact.id());
        history
            .append(ValidationAttestation::for_derivation(
                AttestationId::from_hash(
                    ContentHash::from_str(&format!("blake3:{}", "66".repeat(32))).unwrap(),
                ),
                artifact.id(),
                artifact.derivation(),
                artifact.created_snapshot(),
                None,
                Validity::Valid,
                Authority::Authoritative,
                VerificationLevel::Structural,
                None,
                vec![],
                1_100,
            ))
            .unwrap();
        let s0 = history.strongest().unwrap();
        assert!(default_auto_reuse(
            s0.validity(),
            s0.authority(),
            s0.capture_trust()
        ));

        // Mutation: new caller appears → STALE / AUTHORITATIVE is appended,
        // never substituted (append-only; S0 attestation unchanged).
        history
            .append(ValidationAttestation::for_derivation(
                AttestationId::from_hash(
                    ContentHash::from_str(&format!("blake3:{}", "77".repeat(32))).unwrap(),
                ),
                artifact.id(),
                artifact.derivation(),
                SnapshotId::from_hash(
                    ContentHash::from_str(&format!("blake3:{}", "88".repeat(32))).unwrap(),
                ),
                None,
                Validity::Stale,
                Authority::Authoritative,
                VerificationLevel::Structural,
                None,
                vec![],
                2_000,
            ))
            .unwrap();

        let stale = history.strongest().unwrap();
        assert_eq!(stale.validity(), Validity::Stale);
        assert!(!default_auto_reuse(
            stale.validity(),
            stale.authority(),
            stale.capture_trust()
        ));
        assert_eq!(history.len(), 2);
        // The provisional-era record retains its original standing.
        assert_eq!(history.iter().next().unwrap().validity(), Validity::Valid);
    }
}

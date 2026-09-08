//! Validation attestations (spec §12): the append-only evidentiary history
//! of an artifact.
//!
//! Authority belongs to immutable attestations scoped to a particular
//! source+semantic snapshot — never to mutable fields on the artifact.
//! Individual attestations never transition; an artifact's validation
//! history strengthens only by appending a stronger attestation.

use crate::artifact::Derivation;
use crate::error::DomainError;
use crate::ids::{
    ArtifactId, AttestationId, BlobId, ProjectionObservationId, SnapshotId, Timestamp, VerifierId,
};
use crate::validity::{Authority, CaptureTrust, Validity, VerificationLevel};

/// A reference to evidence backing an attestation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvidenceRef {
    /// A CAS blob (e.g. a verifier result, an evaluation transcript).
    Blob(BlobId),
    /// A projection observation backing a dependency check.
    Observation(ProjectionObservationId),
    /// A prior attestation this one strengthens.
    PriorAttestation(AttestationId),
}

/// One immutable record of an artifact's standing at a particular world
/// state (spec §12.1).
///
/// CaptureTrust is **propagated, not claimed**: it is copied from the
/// derivation the artifact was produced by, and [`for_derivation`] enforces
/// that by taking the derivation itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidationAttestation {
    attestation_id: AttestationId,
    artifact_id: ArtifactId,
    target_source_snapshot: SnapshotId,
    target_semantic_snapshot: Option<SnapshotId>,
    validity: Validity,
    authority: Authority,
    verification_level: VerificationLevel,
    verifier: Option<VerifierId>,
    evidence: Vec<EvidenceRef>,
    capture_trust: CaptureTrust,
    created_at: Timestamp,
}

impl ValidationAttestation {
    /// Construct an attestation whose capture trust is propagated from the
    /// derivation (spec §11: CaptureTrust originates on the derivation).
    ///
    /// `target_semantic_snapshot` is `Some` when validity was established
    /// against an authoritative SCIP-derived semantic snapshot, and `None`
    /// for mid-task overlay evaluations (spec §9.2).
    #[allow(clippy::too_many_arguments)]
    pub fn for_derivation(
        attestation_id: AttestationId,
        artifact_id: ArtifactId,
        derivation: &Derivation,
        target_source_snapshot: SnapshotId,
        target_semantic_snapshot: Option<SnapshotId>,
        validity: Validity,
        authority: Authority,
        verification_level: VerificationLevel,
        verifier: Option<VerifierId>,
        evidence: Vec<EvidenceRef>,
        created_at: Timestamp,
    ) -> Self {
        Self {
            attestation_id,
            artifact_id,
            target_source_snapshot,
            target_semantic_snapshot,
            validity,
            authority,
            verification_level,
            verifier,
            evidence,
            capture_trust: derivation.capture_trust(),
            created_at,
        }
    }

    /// Identity of this attestation.
    #[must_use]
    pub const fn id(&self) -> AttestationId {
        self.attestation_id
    }

    /// The artifact this attestation speaks about.
    #[must_use]
    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }

    /// The source snapshot (world state) validity was evaluated against.
    #[must_use]
    pub const fn target_source_snapshot(&self) -> SnapshotId {
        self.target_source_snapshot
    }

    /// The authoritative semantic snapshot used, if any (spec §9).
    #[must_use]
    pub const fn target_semantic_snapshot(&self) -> Option<SnapshotId> {
        self.target_semantic_snapshot
    }

    /// Validity at the target world.
    #[must_use]
    pub const fn validity(&self) -> Validity {
        self.validity
    }

    /// Authority of the establishing evidence.
    #[must_use]
    pub const fn authority(&self) -> Authority {
        self.authority
    }

    /// Verification strength at establishment.
    #[must_use]
    pub const fn verification_level(&self) -> VerificationLevel {
        self.verification_level
    }

    /// The verifier that established validity, if one was applied.
    #[must_use]
    pub const fn verifier(&self) -> Option<VerifierId> {
        self.verifier
    }

    /// Evidence references.
    #[must_use]
    pub fn evidence(&self) -> &[EvidenceRef] {
        &self.evidence
    }

    /// Capture trust propagated from the derivation.
    #[must_use]
    pub const fn capture_trust(&self) -> CaptureTrust {
        self.capture_trust
    }

    /// Creation time (unix epoch millis, UTC).
    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }
}

/// The append-only validation history of one artifact (spec §12.2).
///
/// # Invariants
/// - **Append-only.** There is no remove, no replace, no mutation. Earlier
///   attestations remain exactly what they always were.
/// - Every entry references the same artifact.
/// - Entries are appended in non-decreasing `created_at` order.
/// - [`AttestationHistory::strongest`] resolves the strongest recorded
///   standing deterministically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationHistory {
    artifact_id: ArtifactId,
    entries: Vec<ValidationAttestation>,
}

impl AttestationHistory {
    /// Start the (possibly empty) history of one artifact.
    #[must_use]
    pub const fn new(artifact_id: ArtifactId) -> Self {
        Self {
            artifact_id,
            entries: Vec::new(),
        }
    }

    /// The artifact this history belongs to.
    #[must_use]
    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }

    /// Append an attestation. Enforces the history invariants; never
    /// mutates existing entries.
    ///
    /// # Errors
    /// - [`DomainError::ArtifactMismatch`] if the attestation references a
    ///   different artifact.
    /// - [`DomainError::DuplicateAttestation`] if the attestation ID was
    ///   already recorded.
    /// - [`DomainError::AttestationOutOfOrder`] if `created_at` precedes
    ///   the latest recorded entry.
    pub fn append(&mut self, attestation: ValidationAttestation) -> Result<(), DomainError> {
        if attestation.artifact_id() != self.artifact_id {
            return Err(DomainError::ArtifactMismatch {
                history: self.artifact_id.to_string(),
                attestation: attestation.artifact_id().to_string(),
            });
        }
        if self.entries.iter().any(|e| e.id() == attestation.id()) {
            return Err(DomainError::DuplicateAttestation {
                id: attestation.id().to_string(),
            });
        }
        if let Some(latest) = self.entries.last() {
            if attestation.created_at() < latest.created_at() {
                return Err(DomainError::AttestationOutOfOrder {
                    latest: latest.created_at(),
                    rejected: attestation.created_at(),
                });
            }
        }
        self.entries.push(attestation);
        Ok(())
    }

    /// All attestations in append order. Read-only.
    pub fn iter(&self) -> std::slice::Iter<'_, ValidationAttestation> {
        self.entries.iter()
    }

    /// Number of recorded attestations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether any attestation is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The strongest recorded standing, deterministically:
    ///
    /// 1. `AUTHORITATIVE` over `PROVISIONAL`;
    /// 2. then higher [`VerificationLevel`];
    /// 3. then latest `created_at`;
    /// 4. then lexicographically greatest attestation ID (deterministic
    ///    tiebreak).
    ///
    /// Applicability to a *current* world (reconciliation) is decided by the
    /// engine at query time (spec §12.3); this method resolves standing
    /// among recorded attestations only.
    #[must_use]
    pub fn strongest(&self) -> Option<&ValidationAttestation> {
        self.entries.iter().max_by(|a, b| {
            strength_key(a)
                .cmp(&strength_key(b))
                .then_with(|| a.id().cmp(&b.id()))
        })
    }
}

fn strength_key(a: &ValidationAttestation) -> (u8, u8, u64) {
    let authority_rank = match a.authority() {
        Authority::Provisional => 0,
        Authority::Authoritative => 1,
    };
    let verification_rank = match a.verification_level() {
        VerificationLevel::Unverified => 0,
        VerificationLevel::EvidenceBacked => 1,
        VerificationLevel::Structural => 2,
        VerificationLevel::Test => 3,
        VerificationLevel::Static => 4,
        VerificationLevel::Deterministic => 5,
    };
    (authority_rank, verification_rank, a.created_at())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{Derivation, ProducerInfo};
    use crate::ids::{ContentHash, HashAlgo};
    use crate::validity::CaptureChannel;
    use std::str::FromStr;

    fn h(seed: u8) -> ContentHash {
        let d: Vec<u8> = (0..32).map(|i| seed.wrapping_add(i)).collect();
        ContentHash::from_bytes(HashAlgo::Blake3, &d).unwrap()
    }

    fn art(seed: u8) -> ArtifactId {
        ArtifactId::from_hash(h(seed))
    }

    fn snap(seed: u8) -> SnapshotId {
        SnapshotId::from_hash(h(seed))
    }

    fn att_id(seed: u8) -> AttestationId {
        AttestationId::from_hash(h(seed))
    }

    fn derivation() -> Derivation {
        Derivation::new(
            crate::ids::DerivationId::from_hash(h(90)),
            vec![CaptureChannel::PartiallyCaptured],
            ProducerInfo::new("harness", "0.1", None).unwrap(),
            0,
        )
    }

    fn att(
        seed: u8,
        artifact: ArtifactId,
        validity: Validity,
        authority: Authority,
        level: VerificationLevel,
        at: u64,
    ) -> ValidationAttestation {
        ValidationAttestation::for_derivation(
            att_id(seed),
            artifact,
            &derivation(),
            snap(70),
            None,
            validity,
            authority,
            level,
            None,
            vec![],
            at,
        )
    }

    #[test]
    fn capture_trust_is_propagated_from_derivation() {
        let a = att(
            1,
            art(80),
            Validity::Valid,
            Authority::Authoritative,
            VerificationLevel::Test,
            10,
        );
        assert_eq!(a.capture_trust(), CaptureTrust::Partial);
    }

    #[test]
    fn history_rejects_foreign_artifacts_and_duplicates() {
        let mut history = AttestationHistory::new(art(80));
        history
            .append(att(
                1,
                art(80),
                Validity::Valid,
                Authority::Provisional,
                VerificationLevel::Unverified,
                10,
            ))
            .unwrap();
        assert_eq!(
            history
                .append(att(
                    2,
                    art(81),
                    Validity::Valid,
                    Authority::Provisional,
                    VerificationLevel::Unverified,
                    11
                ))
                .unwrap_err(),
            DomainError::ArtifactMismatch {
                history: art(80).to_string(),
                attestation: art(81).to_string(),
            }
        );
        assert_eq!(
            history
                .append(att(
                    1,
                    art(80),
                    Validity::Stale,
                    Authority::Provisional,
                    VerificationLevel::Unverified,
                    12
                ))
                .unwrap_err(),
            DomainError::DuplicateAttestation {
                id: att_id(1).to_string(),
            }
        );
    }

    #[test]
    fn history_is_append_only() {
        let mut history = AttestationHistory::new(art(80));
        let first = att(
            1,
            art(80),
            Validity::Valid,
            Authority::Provisional,
            VerificationLevel::Unverified,
            10,
        );
        history.append(first.clone()).unwrap();
        let second = att(
            2,
            art(80),
            Validity::Valid,
            Authority::Authoritative,
            VerificationLevel::Deterministic,
            20,
        );
        history.append(second).unwrap();
        // The earlier attestation is exactly what it always was.
        assert_eq!(history.iter().next().unwrap(), &first);
        assert_eq!(
            history.iter().next().unwrap().authority(),
            Authority::Provisional
        );
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn out_of_order_append_rejected() {
        let mut history = AttestationHistory::new(art(80));
        history
            .append(att(
                1,
                art(80),
                Validity::Valid,
                Authority::Provisional,
                VerificationLevel::Unverified,
                100,
            ))
            .unwrap();
        assert_eq!(
            history
                .append(att(
                    2,
                    art(80),
                    Validity::Valid,
                    Authority::Provisional,
                    VerificationLevel::Unverified,
                    50
                ))
                .unwrap_err(),
            DomainError::AttestationOutOfOrder {
                latest: 100,
                rejected: 50,
            }
        );
    }

    #[test]
    fn strongest_prefers_authority_then_verification_then_time() {
        let mut history = AttestationHistory::new(art(80));
        history
            .append(att(
                1,
                art(80),
                Validity::Valid,
                Authority::Provisional,
                VerificationLevel::Deterministic,
                30,
            ))
            .unwrap();
        // Appended later in time, weaker verification — authority still wins.
        history
            .append(att(
                2,
                art(80),
                Validity::Valid,
                Authority::Authoritative,
                VerificationLevel::Unverified,
                40,
            ))
            .unwrap();
        assert_eq!(history.strongest().unwrap().id(), att_id(2));

        let mut history2 = AttestationHistory::new(art(80));
        history2
            .append(att(
                3,
                art(80),
                Validity::Valid,
                Authority::Authoritative,
                VerificationLevel::Structural,
                10,
            ))
            .unwrap();
        history2
            .append(att(
                4,
                art(80),
                Validity::Valid,
                Authority::Authoritative,
                VerificationLevel::Test,
                20,
            ))
            .unwrap();
        assert_eq!(history2.strongest().unwrap().id(), att_id(4));
    }

    #[test]
    fn strongest_is_deterministic_on_full_ties() {
        let mut history = AttestationHistory::new(art(80));
        history
            .append(att(
                5,
                art(80),
                Validity::Valid,
                Authority::Provisional,
                VerificationLevel::Structural,
                10,
            ))
            .unwrap();
        history
            .append(att(
                6,
                art(80),
                Validity::Valid,
                Authority::Provisional,
                VerificationLevel::Structural,
                10,
            ))
            .unwrap();
        let strongest = history.strongest().unwrap().id();
        assert_eq!(strongest, att_id(6)); // lexicographic tiebreak
        assert_eq!(
            AttestationId::from_str(&strongest.to_string()).unwrap(),
            strongest
        );
    }

    #[test]
    fn empty_history_has_no_strongest() {
        let history = AttestationHistory::new(art(80));
        assert!(history.is_empty());
        assert!(history.strongest().is_none());
    }
}

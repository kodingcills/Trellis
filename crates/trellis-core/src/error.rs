//! Domain error type for construction-time invariant violations.

use std::fmt;

use crate::ids::HashAlgo;

/// Errors raised when a domain object would be constructed in an illegal
/// state. Construction is total otherwise: every `Ok` value satisfies the
/// frozen invariants by construction (spec §2.6, §93).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// A required text field was empty or whitespace-only.
    EmptyField(&'static str),
    /// An ID string lacked the `<algo>:<hex>` separator.
    MissingHashSeparator,
    /// An ID string used an unknown algorithm prefix.
    UnknownHashAlgo(String),
    /// Hex decoding failed for a digest of the given character length.
    InvalidHexDigest(usize),
    /// Digest length did not match the algorithm's requirement.
    DigestLength {
        /// Algorithm whose digest length was violated.
        algo: HashAlgo,
        /// Actual byte length supplied.
        got: usize,
    },
    /// A git OID was not exactly 20 bytes.
    GitOidLength(usize),
    /// A snapshot was constructed with itself as its own parent.
    SelfParent,
    /// A projection was constructed with an empty subject.
    EmptySubject,
    /// A projection's property disagreed with its kind.
    InconsistentProperty {
        /// The kind the projection was constructed with.
        kind: &'static str,
        /// The property that disagreed with it.
        property: &'static str,
    },
    /// An attestation for a different artifact was appended to a history.
    ArtifactMismatch {
        /// Artifact the history belongs to.
        history: String,
        /// Artifact the offending attestation references.
        attestation: String,
    },
    /// An attestation with an already-recorded ID was appended.
    DuplicateAttestation {
        /// The duplicated attestation ID.
        id: String,
    },
    /// An attestation was appended with a creation time earlier than the
    /// latest entry, which would corrupt the append-order history.
    AttestationOutOfOrder {
        /// Creation time of the latest existing attestation.
        latest: u64,
        /// Creation time of the rejected attestation.
        rejected: u64,
    },
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomainError::EmptyField(name) => write!(f, "required field `{name}` is empty"),
            DomainError::MissingHashSeparator => {
                write!(f, "content id missing `<algo>:<hex>` separator")
            }
            DomainError::UnknownHashAlgo(prefix) => write!(f, "unknown hash algorithm `{prefix}`"),
            DomainError::InvalidHexDigest(len) => {
                write!(f, "invalid hex digest ({len} chars)")
            }
            DomainError::DigestLength { algo, got } => {
                write!(
                    f,
                    "digest length {} for {} (expected {})",
                    got,
                    algo,
                    algo.digest_len()
                )
            }
            DomainError::GitOidLength(got) => write!(f, "git oid length {got} (expected 20)"),
            DomainError::SelfParent => write!(f, "snapshot cannot be its own parent"),
            DomainError::EmptySubject => write!(f, "projection subject is empty"),
            DomainError::InconsistentProperty { kind, property } => {
                write!(f, "property `{property}` inconsistent with kind `{kind}`")
            }
            DomainError::ArtifactMismatch {
                history,
                attestation,
            } => {
                write!(
                    f,
                    "attestation for artifact {attestation} appended to history of {history}"
                )
            }
            DomainError::DuplicateAttestation { id } => {
                write!(f, "attestation {id} already recorded")
            }
            DomainError::AttestationOutOfOrder { latest, rejected } => {
                write!(
                    f,
                    "attestation created_at {rejected} precedes latest {latest}"
                )
            }
        }
    }
}

impl std::error::Error for DomainError {}

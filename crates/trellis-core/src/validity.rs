//! Epistemic dimensions (spec §11): validity, authority, capture trust,
//! verification level — four orthogonal questions, never collapsed into one
//! enum — plus internal engine evaluation states.

/// **Validity** — is the artifact still true in the target world?
///
/// Lives on a [`crate::attestation::ValidationAttestation`], never on the
/// artifact. `UNKNOWN` is the honest answer when evidence is missing; it
/// always denies auto-reuse (spec §15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Validity {
    /// All tracked dependency projections retain their relevant values, or a
    /// verifier re-established the contract.
    Valid,
    /// The artifact's validity contract no longer holds.
    Stale,
    /// Trellis lacks evidence to call the artifact valid or stale.
    Unknown,
}

/// **Authority** — how authoritative was the evidence that established the
/// attestation?
///
/// Lives on a [`crate::attestation::ValidationAttestation`]. World drift
/// never weakens the authority of an existing attestation; it makes that
/// attestation *not yet applicable* to the new world (spec §12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Authority {
    /// Established by a freeze against an authoritative semantic snapshot.
    Authoritative,
    /// Established mid-task against the provisional live overlay; may be
    /// strengthened later only by appending a stronger attestation.
    Provisional,
}

/// **CaptureTrust** — how completely were the observation channels of a
/// derivation captured?
///
/// Originates on the [`crate::artifact::Derivation`] and propagates into
/// attestations (spec §11). Trust is inherited from the capture channel,
/// never claimed by the artifact (spec §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaptureTrust {
    /// Weakest. The derivation relied on observations Trellis never
    /// captured. Default policy: no auto-reuse regardless of class.
    Unobserved,
    /// Dependencies were agent-declared, not captured.
    DeclaredOnly,
    /// Mixed channels; some observations unrecorded.
    Partial,
    /// Strongest. Every dependency-relevant observation channel
    /// participating in the derivation was captured with sufficient
    /// semantics for the artifact's reuse policy.
    Complete,
}

/// An observation channel that participated in a derivation.
///
/// `COMPLETE` capture is defined by *evidence about channels*, not by tool
/// naming (spec §13): any fully-intercepted channel — a `trellis.*` tool or
/// a wrapped shell capturing command, cwd, environment dependencies, output,
/// and source snapshot — can reach [`CaptureTrust::Complete`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureChannel {
    /// A trellis-aware structural tool; semantically captured.
    TrellisTool,
    /// A fully intercepted external channel (e.g. wrapped shell tool).
    FullyIntercepted,
    /// A channel captured only partially; some observations unrecorded.
    PartiallyCaptured,
    /// Dependencies agent-declared, nothing captured.
    DeclaredOnly,
    /// Observations Trellis never captured at all.
    Unobserved,
}

impl CaptureChannel {
    /// The capture trust this channel alone can attest to.
    #[must_use]
    pub const fn trust(self) -> CaptureTrust {
        match self {
            CaptureChannel::TrellisTool | CaptureChannel::FullyIntercepted => {
                CaptureTrust::Complete
            }
            CaptureChannel::PartiallyCaptured => CaptureTrust::Partial,
            CaptureChannel::DeclaredOnly => CaptureTrust::DeclaredOnly,
            CaptureChannel::Unobserved => CaptureTrust::Unobserved,
        }
    }
}

/// Derive the capture trust of a derivation from its channels: the trust is
/// the **weakest** channel that participated. One unobserved channel makes
/// the whole derivation `UNOBSERVED` (spec §13). No channels at all means
/// nothing was captured — also `UNOBSERVED`.
#[must_use]
pub fn capture_trust_from_channels(channels: &[CaptureChannel]) -> CaptureTrust {
    let Some(first) = channels.first() else {
        return CaptureTrust::Unobserved;
    };
    let mut weakest = first.trust();
    for channel in &channels[1..] {
        let t = channel.trust();
        if t < weakest {
            weakest = t;
        }
    }
    weakest
}

/// **VerificationLevel** — how strong is the verification attached to the
/// attestation? Ordered from weakest to strongest (spec §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VerificationLevel {
    /// Nothing attached.
    Unverified,
    /// Backed by recorded evidence references, no deterministic verifier.
    EvidenceBacked,
    /// A structural query verifies the claim.
    Structural,
    /// A test (e.g. property test or unit test) verifies the claim.
    Test,
    /// A static analysis verifies the claim.
    Static,
    /// Established deterministically; the strongest level.
    Deterministic,
}

/// Internal engine evaluation states (spec §11, §18).
///
/// These describe the invalidation machinery's view of a projection or
/// artifact *during evaluation*. They are **not** part of the public truth
/// model and must never appear on artifacts or attestations — the public
/// model is [`Validity`] + [`Authority`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineState {
    /// All tracked dependency values retained.
    Green,
    /// A dependency may have changed; the new value is not yet evaluated.
    Dirty,
    /// A dependency changed, but the artifact may remain semantically true.
    Verify,
    /// The validity contract no longer holds.
    Red,
}

/// The floor reuse policy from spec §15: auto-reuse requires `VALID`
/// validity, `AUTHORITATIVE` authority, and capture trust better than
/// `UNOBSERVED`.
///
/// This is a **floor, not the whole policy**: artifact class, relevance, and
/// consumer trust policy (spec §15) may deny reuse further, but never relax
/// these conditions.
#[must_use]
pub fn default_auto_reuse(
    validity: Validity,
    authority: Authority,
    capture_trust: CaptureTrust,
) -> bool {
    matches!(validity, Validity::Valid)
        && matches!(authority, Authority::Authoritative)
        && !matches!(capture_trust, CaptureTrust::Unobserved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_level_is_ordered_weakest_to_strongest() {
        assert!(VerificationLevel::Deterministic > VerificationLevel::Static);
        assert!(VerificationLevel::Static > VerificationLevel::Test);
        assert!(VerificationLevel::Test > VerificationLevel::Structural);
        assert!(VerificationLevel::Structural > VerificationLevel::EvidenceBacked);
        assert!(VerificationLevel::EvidenceBacked > VerificationLevel::Unverified);
    }

    #[test]
    fn capture_trust_orders_weakest_to_strongest() {
        assert!(CaptureTrust::Complete > CaptureTrust::Partial);
        assert!(CaptureTrust::Partial > CaptureTrust::DeclaredOnly);
        assert!(CaptureTrust::DeclaredOnly > CaptureTrust::Unobserved);
    }

    #[test]
    fn trust_is_weakest_participating_channel() {
        let all_good = [
            CaptureChannel::TrellisTool,
            CaptureChannel::FullyIntercepted,
            CaptureChannel::TrellisTool,
        ];
        assert_eq!(
            capture_trust_from_channels(&all_good),
            CaptureTrust::Complete
        );

        let one_unobserved = [CaptureChannel::TrellisTool, CaptureChannel::Unobserved];
        assert_eq!(
            capture_trust_from_channels(&one_unobserved),
            CaptureTrust::Unobserved
        );

        let declared_and_partial = [
            CaptureChannel::DeclaredOnly,
            CaptureChannel::PartiallyCaptured,
        ];
        assert_eq!(
            capture_trust_from_channels(&declared_and_partial),
            CaptureTrust::DeclaredOnly
        );
    }

    #[test]
    fn empty_channels_capture_nothing() {
        // A derivation with no recorded observation channels captured
        // nothing: it must receive the weakest trust, never a free `COMPLETE`.
        assert_eq!(capture_trust_from_channels(&[]), CaptureTrust::Unobserved);
    }

    #[test]
    fn default_reuse_floor_truth_table() {
        use Authority::*;
        use CaptureTrust::*;
        use Validity::*;

        assert!(default_auto_reuse(Valid, Authoritative, Complete));
        assert!(default_auto_reuse(Valid, Authoritative, Partial));
        assert!(default_auto_reuse(Valid, Authoritative, DeclaredOnly));

        assert!(!default_auto_reuse(Valid, Provisional, Complete));
        assert!(!default_auto_reuse(Unknown, Authoritative, Complete));
        assert!(!default_auto_reuse(Stale, Authoritative, Complete));
        assert!(!default_auto_reuse(Valid, Authoritative, Unobserved));
    }
}

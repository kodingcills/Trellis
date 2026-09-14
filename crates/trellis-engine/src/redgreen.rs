//! Red/green invalidation with value-equality cutoff and causal
//! explanation (M6, spec §18, §33).
//!
//! Given a reconciled changed-file set and a target snapshot, the
//! transition runs the frozen §18 sequence over persisted state:
//!
//! 1. discover reevaluation candidates from recorded observations (M5);
//! 2. reevaluate each candidate projection through a
//!    [`ReevaluationSource`];
//! 3. a projection whose reevaluated canonical value equals every
//!    compared prior observation is **green** — dependents are never
//!    dirtied (the value-equality cutoff, §18 step 12);
//! 4. a projection whose value differs from any compared prior
//!    observation is **red** — every artifact declaring a dependency on
//!    it has its validity contract evaluated and the outcome is appended
//!    as a [`ValidationAttestation`] (§18 steps 5-9);
//! 5. the whole evaluation is returned as an [`TransitionReport`]: the
//!    exact cause chain naming each projection, its old and new values,
//!    and the affected dependents (§28 item 7).
//!
//! Epistemic discipline (§11): internal evaluation states live only
//! inside this module's report; artifacts and attestations carry only the
//! public [`Validity`]/[`Authority`]/[`VerificationLevel`] model. Nothing
//! in this module mutates an artifact or an existing attestation — every
//! validity transition is an append (§12).

use std::collections::{BTreeMap, BTreeSet};

use trellis_core::attestation::{EvidenceRef, ValidationAttestation};
use trellis_core::coverage::{Completeness, CompletenessEvaluator, CoverageCertificate, Universe};
use trellis_core::ids::{
    ArtifactId, AttestationId, ContentHash, HashAlgo, ProjectionId, ProjectionObservationId,
    SnapshotId, Timestamp, VerifierId,
};
use trellis_core::observation_id::canonical_observation_id;
use trellis_core::projection::{Projection, ProjectionObservation};
use trellis_core::validity::{Authority, Validity, VerificationLevel};
use trellis_source::reconcile::ChangedSet;

use crate::discovery::{recorded_from_store, reevaluation_candidates};
use crate::observe::syntactic_value_canonical;
use trellis_program::prelude::PythonSyntaxIndex;

/// Transition failures. All explicit; nothing is silently dropped (§8).
#[derive(Debug)]
pub enum TransitionError {
    /// Persistence failure.
    Store(trellis_store::StoreError),
    /// Observation-production failure (kind mismatch).
    Observe(crate::observe::ObserveError),
}

impl From<trellis_store::StoreError> for TransitionError {
    fn from(e: trellis_store::StoreError) -> Self {
        TransitionError::Store(e)
    }
}

impl From<crate::observe::ObserveError> for TransitionError {
    fn from(e: crate::observe::ObserveError) -> Self {
        TransitionError::Observe(e)
    }
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransitionError::Store(e) => write!(f, "store: {e}"),
            TransitionError::Observe(e) => write!(f, "observe: {e}"),
        }
    }
}

impl std::error::Error for TransitionError {}

/// A reevaluated projection value plus the epistemic class of the source
/// that proved it. `authoritative = false` marks provisional evidence
/// (e.g. a mid-task overlay, M9); such evidence can never establish
/// authoritative validity (§9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluated {
    value: String,
    authoritative: bool,
}

impl Evaluated {
    /// A proven value with its evidence class.
    #[must_use]
    pub fn new(value: impl Into<String>, authoritative: bool) -> Self {
        Self {
            value: value.into(),
            authoritative,
        }
    }

    /// The canonical value rendering.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Whether the evidence class is authoritative.
    #[must_use]
    pub const fn is_authoritative(&self) -> bool {
        self.authoritative
    }
}

/// The completeness context a transition is evaluated under (§8): the
/// explicit eligible universe, the backend's coverage certificate for
/// this snapshot, and the indexer identity. Absent → the transition
/// behaves exactly as M6 defined it (no binding checks, no completeness
/// verdicts); present, it governs every completeness-sensitive kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageContext {
    /// The eligible universe this transition's world state declares.
    pub universe: Universe,
    /// The backend's coverage capabilities over that universe.
    pub certificate: CoverageCertificate,
    /// Indexer identity/version/config (§8.1: persisted per binding).
    pub indexer: String,
}

/// Where a projection's current value comes from. Production ships the
/// syntactic backend ([`SyntacticSource`]); semantic backends are
/// injected by the caller — the engine itself never synthesizes semantic
/// values (§8: uncertainty is never an empty set, and a syntax backend is
/// never allowed to answer semantic queries).
pub trait ReevaluationSource {
    /// The projection's canonical value at the current world state, or
    /// `None` when this source has no proven evidence for it. `None`
    /// never means "empty": it means "no evidence this round", and the
    /// transition treats it as unobserved rather than as a change.
    /// # Errors
    /// Source-internal failures.
    fn evaluate(&self, projection: &Projection) -> Result<Option<Evaluated>, TransitionError>;
}

/// The production syntactic backend: answers the four syntactic
/// projection kinds from a parsed [`PythonSyntaxIndex`] and declines
/// everything else (semantic kinds return `None`, never a synthesized
/// set — §8, M3 boundary).
pub struct SyntacticSource<'a> {
    index: &'a PythonSyntaxIndex,
}

impl<'a> SyntacticSource<'a> {
    /// Wrap an index built over the current source tree.
    #[must_use]
    pub fn new(index: &'a PythonSyntaxIndex) -> Self {
        Self { index }
    }
}

impl ReevaluationSource for SyntacticSource<'_> {
    fn evaluate(&self, projection: &Projection) -> Result<Option<Evaluated>, TransitionError> {
        match projection.kind() {
            ProjectionKind::FileContent
            | ProjectionKind::Definition
            | ProjectionKind::Signature
            | ProjectionKind::Imports => Ok(syntactic_value_canonical(self.index, projection)?
                .map(|value| Evaluated::new(value, true))),
            _ => Ok(None),
        }
    }
}

use trellis_core::projection::ProjectionKind;

/// Canonical identity of the built-in set-continuity contract
/// (`STRUCTURAL_SET` artifacts): valid iff every reevaluated dependency
/// value equals the value pinned by the artifact's latest attestation.
pub const SET_EQUALITY_CONTRACT: &str = "trellis.contract/set-equality.v1";

/// Canonical identity of the built-in absence contract (`FACT` artifacts
/// carrying an absence proposition): valid iff every reevaluated
/// dependency value is the canonical empty-set encoding.
pub const ABSENCE_FACT_CONTRACT: &str = "trellis.contract/absence-fact.v1";

/// The verifier identity for a built-in contract name (BLAKE3 over the
/// name — same derivation the harness uses when seeding base
/// attestations, so contract selection round-trips through persistence).
#[must_use]
pub fn contract_verifier_id(name: &str) -> VerifierId {
    VerifierId::from_hash(ContentHash::compute(HashAlgo::Blake3, name.as_bytes()))
}

fn digest_of(canonical: &str) -> ContentHash {
    ContentHash::compute(HashAlgo::Blake3, canonical.as_bytes())
}

/// Internal evaluation outcome of one candidate projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionOutcomeKind {
    /// Reevaluated value equals every compared prior observation:
    /// green — dependents are not dirtied (§18 step 12 cutoff).
    Unchanged,
    /// Reevaluated value differs from at least one compared prior
    /// observation: red — dependent artifacts are re-contracted.
    Changed,
    /// The source had no proven evidence this round: nothing was
    /// recorded and nothing changed (absence of evidence is not a
    /// change, §8).
    Unobserved,
}

/// The explain record for one reevaluated projection (§28 item 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionOutcome {
    projection: Projection,
    compared: Vec<ProjectionObservationId>,
    prior_digest: Option<ContentHash>,
    new_value: Option<String>,
    new_digest: Option<ContentHash>,
    outcome: ProjectionOutcomeKind,
    /// The completeness verdict under which a completeness-sensitive
    /// projection was evaluated this round (§8.2); `None` for other
    /// kinds and when no coverage context was supplied.
    completeness: Option<Completeness>,
    /// Whether the completeness standing changed relative to the
    /// persisted binding (§8.1 dirtying) even though the value may be
    /// unchanged. Such outcomes enter the artifact pass without being
    /// value changes.
    completeness_dirty: bool,
    affected_artifacts: Vec<ArtifactId>,
}

impl ProjectionOutcome {
    /// The evaluated projection key.
    #[must_use]
    pub fn projection(&self) -> &Projection {
        &self.projection
    }

    /// Prior observations the new value was compared against.
    #[must_use]
    pub fn compared(&self) -> &[ProjectionObservationId] {
        &self.compared
    }

    /// The digest the projection's latest attestation evidence pinned
    /// before this transition, when one was found among the compared
    /// observations.
    #[must_use]
    pub const fn prior_digest(&self) -> Option<&ContentHash> {
        self.prior_digest.as_ref()
    }

    /// The reevaluated canonical value, when proven.
    #[must_use]
    pub fn new_value(&self) -> Option<&str> {
        self.new_value.as_deref()
    }

    /// Digest of the reevaluated canonical value.
    #[must_use]
    pub const fn new_digest(&self) -> Option<&ContentHash> {
        self.new_digest.as_ref()
    }

    /// The internal evaluation outcome (never persisted on artifacts).
    #[must_use]
    pub const fn outcome(&self) -> ProjectionOutcomeKind {
        self.outcome
    }

    /// The completeness verdict for completeness-sensitive kinds
    /// evaluated under a coverage context (§8.2).
    #[must_use]
    pub const fn completeness(&self) -> Option<Completeness> {
        self.completeness
    }

    /// Whether the completeness standing changed this round (§8.1).
    #[must_use]
    pub const fn completeness_dirty(&self) -> bool {
        self.completeness_dirty
    }

    /// Artifacts whose validity contract was re-evaluated because of
    /// this projection's value change.
    #[must_use]
    pub fn affected_artifacts(&self) -> &[ArtifactId] {
        &self.affected_artifacts
    }
}

/// How an artifact's validity was settled by the transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractVerdict {
    /// The contract holds under the reevaluated dependencies.
    Holds,
    /// The contract fails under the reevaluated dependencies.
    Fails,
    /// The contract could not be evaluated (no verifier, or the latest
    /// attestation pins no comparable evidence): conservative UNKNOWN
    /// appended — never auto-reuse (§15, §18 step 9).
    Undecidable,
}

impl From<ContractVerdict> for Validity {
    fn from(v: ContractVerdict) -> Self {
        match v {
            ContractVerdict::Holds => Validity::Valid,
            ContractVerdict::Fails => Validity::Stale,
            ContractVerdict::Undecidable => Validity::Unknown,
        }
    }
}

/// The explain record for one artifact's re-contracted validity (§28
/// item 7): which dependency changed, the pinned prior value, the new
/// value, and the attestation appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactOutcome {
    artifact: ArtifactId,
    dependency_evaluations: Vec<DependencyEvaluation>,
    verdict: ContractVerdict,
    appended: Option<Validity>,
    attestation: Option<AttestationId>,
}

impl ArtifactOutcome {
    /// The re-contracted artifact.
    #[must_use]
    pub fn artifact(&self) -> ArtifactId {
        self.artifact
    }

    /// Per-dependency before/after evaluation.
    #[must_use]
    pub fn dependency_evaluations(&self) -> &[DependencyEvaluation] {
        &self.dependency_evaluations
    }

    /// The internal contract verdict (never persisted on the artifact).
    #[must_use]
    pub const fn verdict(&self) -> ContractVerdict {
        self.verdict
    }

    /// The public validity appended by this transition, if any.
    #[must_use]
    pub const fn appended(&self) -> Option<Validity> {
        self.appended
    }

    /// The appended attestation's identity.
    #[must_use]
    pub const fn attestation(&self) -> Option<AttestationId> {
        self.attestation
    }
}

/// One dependency's before/after comparison inside an artifact outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEvaluation {
    projection: ProjectionId,
    changed: bool,
    authoritative: bool,
    /// The completeness verdict under which this dependency's fresh
    /// value was established (completeness-sensitive kinds only, §8).
    completeness: Option<Completeness>,
    /// Whether the completeness standing changed relative to the
    /// persisted binding even though the value may be unchanged (§8.1).
    completeness_dirty: bool,
    pinned_digest: Option<ContentHash>,
    new_digest: Option<ContentHash>,
    new_value: Option<String>,
}

impl DependencyEvaluation {
    /// The dependent projection.
    #[must_use]
    pub const fn projection(&self) -> &ProjectionId {
        &self.projection
    }

    /// Whether the value changed relative to the pinned prior value.
    #[must_use]
    pub const fn changed(&self) -> bool {
        self.changed
    }

    /// Whether the fresh evidence is authoritative.
    #[must_use]
    pub const fn is_authoritative(&self) -> bool {
        self.authoritative
    }

    /// The completeness verdict of this dependency's fresh evidence.
    #[must_use]
    pub const fn completeness(&self) -> Option<Completeness> {
        self.completeness
    }

    /// Whether the completeness standing changed relative to the
    /// persisted binding (§8.1).
    #[must_use]
    pub const fn completeness_dirty(&self) -> bool {
        self.completeness_dirty
    }

    /// The value the artifact's latest attestation pinned for this
    /// dependency, when found.
    #[must_use]
    pub const fn pinned_digest(&self) -> Option<&ContentHash> {
        self.pinned_digest.as_ref()
    }

    /// The reevaluated value digest.
    #[must_use]
    pub const fn new_digest(&self) -> Option<&ContentHash> {
        self.new_digest.as_ref()
    }

    /// The reevaluated canonical value, when proven.
    #[must_use]
    pub fn new_value(&self) -> Option<&str> {
        self.new_value.as_deref()
    }
}

/// The full causal record of one transition run (§28 item 7): every
/// reevaluated projection with old/new values, every re-contracted
/// artifact, and every appended attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionReport {
    target_snapshot: SnapshotId,
    projections: Vec<ProjectionOutcome>,
    artifacts: Vec<ArtifactOutcome>,
}

impl TransitionReport {
    /// The snapshot the transition evaluated against.
    #[must_use]
    pub const fn target_snapshot(&self) -> SnapshotId {
        self.target_snapshot
    }

    /// Per-projection outcomes in canonical projection-id order.
    #[must_use]
    pub fn projections(&self) -> &[ProjectionOutcome] {
        &self.projections
    }

    /// Per-artifact outcomes in canonical artifact-id order.
    #[must_use]
    pub fn artifacts(&self) -> &[ArtifactOutcome] {
        &self.artifacts
    }
}

/// One §18 transition over persisted state.
pub struct Transition<'a> {
    store: &'a mut trellis_store::Store,
    source: &'a dyn ReevaluationSource,
    changed: ChangedSet,
    target_snapshot: SnapshotId,
    now: Timestamp,
    coverage: Option<CoverageContext>,
}

impl<'a> Transition<'a> {
    /// Prepare a transition: persistence handle, reevaluation sources,
    /// the reconciled changed-file set (step 1 is the caller's reconcile
    /// against the working tree — the engine never touches the
    /// filesystem), the target snapshot, and the transition's base
    /// timestamp (appends within the run advance deterministically).
    #[must_use]
    pub fn new(
        store: &'a mut trellis_store::Store,
        source: &'a dyn ReevaluationSource,
        changed: ChangedSet,
        target_snapshot: SnapshotId,
        now: Timestamp,
    ) -> Self {
        Self {
            store,
            source,
            changed,
            target_snapshot,
            now,
            coverage: None,
        }
    }

    /// Evaluate under an explicit completeness context (§8). When set,
    /// every completeness-sensitive projection binds to (Q, U, C): a
    /// universe/coverage/indexer change relative to the persisted
    /// binding dirties the observation for reevaluation — even with an
    /// empty changed set (catches indexer bumps) — and the certificate
    /// verdict governs absence/set claims. When absent, the transition
    /// behaves exactly as M6 defined it.
    #[must_use]
    pub fn with_coverage(mut self, coverage: CoverageContext) -> Self {
        self.coverage = Some(coverage);
        self
    }

    /// Run the transition (§18 steps 2-12). Idempotent for identical
    /// inputs: observation identities are canonical and attestation
    /// identities are content-derived, so re-running the same transition
    /// appends nothing new.
    ///
    /// # Errors
    /// Persistence or source failures; corrupt persisted state fails
    /// closed (never silently skipped).
    pub fn run(self) -> Result<TransitionReport, TransitionError> {
        let recorded = recorded_from_store(self.store)?;
        let candidates = reevaluation_candidates(&self.changed, &recorded);
        let candidate_ids: BTreeSet<ProjectionObservationId> =
            candidates.ids().into_iter().collect();

        // §8.1 dirtying: a universe/coverage/indexer change relative to
        // the persisted (Q, U, C) binding re-dirties the observation for
        // reevaluation — independent of the file-level changed set (an
        // indexer bump with an untouched tree must still reevaluate).
        // It triggers reevaluation, never immediate downstream
        // destruction; downstream effects follow the normal red/green
        // evaluation of the reevaluated value (§8.1).
        let mut forced: BTreeSet<ProjectionId> = BTreeSet::new();
        if let Some(ctx) = &self.coverage {
            let universe_digest = ctx.universe.digest();
            let coverage_digest = ctx.certificate.digest();
            let mut seen: BTreeSet<ProjectionId> = BTreeSet::new();
            for rec in &recorded {
                let pid = rec.observation.projection();
                if !rec.projection.kind().is_completeness_sensitive() || !seen.insert(pid) {
                    continue;
                }
                let dirty = match self.store.completeness_binding(&pid) {
                    Err(trellis_store::StoreError::NotFound(_)) => true,
                    Err(e) => return Err(e.into()),
                    Ok((_, u, c, indexer, _)) => {
                        u != universe_digest || c != coverage_digest || indexer != ctx.indexer
                    }
                };
                if dirty {
                    forced.insert(pid);
                }
            }
        }

        let by_id: BTreeMap<ProjectionObservationId, &crate::discovery::RecordedObservation> =
            recorded
                .iter()
                .map(|r| (r.observation.id(), r))
                .filter(|(_, r)| {
                    candidate_ids.contains(&r.observation.id())
                        || forced.contains(&r.observation.projection())
                })
                .collect();

        let mut by_projection: BTreeMap<ProjectionId, Vec<&crate::discovery::RecordedObservation>> =
            BTreeMap::new();
        for obs in by_id.values() {
            by_projection
                .entry(obs.observation.projection())
                .or_default()
                .push(obs);
        }

        // (value, digest, authoritative) per changed projection for the
        // artifact pass.
        let mut changed_values: BTreeMap<ProjectionId, Evaluated> = BTreeMap::new();
        let mut new_observations: BTreeMap<ProjectionId, ProjectionObservation> = BTreeMap::new();
        let mut projections: Vec<ProjectionOutcome> = Vec::new();

        for (pid, obs) in &by_projection {
            let projection = &obs[0].projection;
            let evaluated = self.source.evaluate(projection)?;
            let compared: Vec<ProjectionObservationId> =
                obs.iter().map(|o| o.observation.id()).collect();

            // §8.2: the completeness verdict for this round, evaluated
            // by the backend-independent evaluator against the supplied
            // certificate. `None` when no coverage context was supplied
            // (M6 behavior) or the kind makes no absence claims.
            let completeness = match (
                &self.coverage,
                projection.kind().is_completeness_sensitive(),
            ) {
                (Some(ctx), true) => Some(CompletenessEvaluator::evaluate(
                    projection.kind(),
                    &ctx.certificate,
                )),
                _ => None,
            };

            // §8.1: a completeness-standing change (coverage downgrade
            // or upgrade vs the persisted binding) dirties the CLAIM for
            // reevaluation even when the underlying value is identical —
            // downstream effects then follow the normal red/green
            // contract evaluation, never immediate destruction.
            let claim_dirty = match completeness {
                Some(verdict) => {
                    let bound = match self.store.completeness_binding(pid) {
                        Ok((_, _, _, _, bound)) => Some(bound),
                        Err(trellis_store::StoreError::NotFound(_)) => None,
                        Err(e) => return Err(e.into()),
                    };
                    bound != Some(verdict)
                }
                None => false,
            };
            let outcome = match evaluated {
                Some(ev) => {
                    let new_digest = digest_of(ev.value());
                    let value_changed = obs
                        .iter()
                        .any(|o| o.observation.value_digest() != &new_digest);
                    let new_observation = ProjectionObservation::new(
                        canonical_observation_id(pid, &new_digest, &self.target_snapshot),
                        *pid,
                        self.target_snapshot,
                        new_digest,
                        None,
                    );
                    // Persist the (Q, U, C) binding for
                    // completeness-sensitive kinds (§8.1). Unbound here
                    // means the projection was never seeded with a
                    // binding — the dirtying rule above marks it as a
                    // candidate next round regardless, so this only
                    // records what this round established.
                    if let Some(ctx) = &self.coverage {
                        if projection.kind().is_completeness_sensitive() {
                            self.store.put_completeness_binding(
                                pid,
                                &self.target_snapshot,
                                &ctx.universe.digest(),
                                &ctx.certificate.digest(),
                                &ctx.indexer,
                                completeness.unwrap_or(Completeness::Unknown),
                            )?;
                        }
                    }
                    let changed = value_changed || claim_dirty;
                    // Anchor is preserved from the recorded chain: the
                    // projection's discovery anchor is an evaluation-time
                    // fact of the same projection, so re-recording with
                    // it cannot conflict (record_observation_record
                    // enforces same-anchor idempotency).
                    self.store.record_observation_record(
                        projection,
                        &new_observation,
                        &obs[0].anchor_path,
                    )?;
                    if changed {
                        changed_values.insert(*pid, ev.clone());
                    }
                    new_observations.insert(*pid, new_observation);
                    let outcome = if changed {
                        ProjectionOutcomeKind::Changed
                    } else {
                        ProjectionOutcomeKind::Unchanged
                    };
                    ProjectionOutcome {
                        projection: (*projection).clone(),
                        compared,
                        prior_digest: Some(obs[0].observation.value_digest().to_owned()),
                        new_value: Some(ev.value().to_string()),
                        new_digest: Some(new_digest),
                        outcome,
                        completeness,
                        completeness_dirty: claim_dirty && !value_changed,
                        affected_artifacts: Vec::new(),
                    }
                }
                None => ProjectionOutcome {
                    projection: (*projection).clone(),
                    compared,
                    prior_digest: Some(obs[0].observation.value_digest().to_owned()),
                    new_value: None,
                    new_digest: None,
                    outcome: ProjectionOutcomeKind::Unobserved,
                    completeness,
                    completeness_dirty: claim_dirty,
                    affected_artifacts: Vec::new(),
                },
            };
            projections.push(outcome);
        }

        // Artifact pass: every artifact depending on a changed
        // projection is re-contracted (§18 steps 5-9). Unchanged and
        // unobserved projections never dirty their dependents — the
        // value-equality cutoff — EXCEPT a completeness-standing change
        // (§8.1): the claim itself was dirtied, so its dependents are
        // re-evaluated (the contract then honestly downgrades to UNKNOWN
        // when the reevaluated values would otherwise hold).
        let mut artifacts: Vec<ArtifactOutcome> = Vec::new();
        let mut append_seq: u64 = 0;
        for outcome in projections
            .iter_mut()
            .filter(|o| o.outcome == ProjectionOutcomeKind::Changed || o.completeness_dirty)
        {
            let pid = outcome.projection.id();
            let dependents = self.store.artifacts_by_dependency(&pid)?;
            outcome.affected_artifacts = dependents.clone();
            for aid in dependents {
                // An Unobserved projection has no fresh observation this
                // round; the contract evaluation then honestly degrades
                // to Undecidable (missing fresh evidence, §15).
                let evaluation = match new_observations.get(&pid) {
                    Some(new_observation) => DependencyEvaluation {
                        projection: pid,
                        changed: outcome.outcome == ProjectionOutcomeKind::Changed,
                        authoritative: changed_values
                            .get(&pid)
                            .is_some_and(Evaluated::is_authoritative),
                        completeness: outcome.completeness,
                        completeness_dirty: outcome.completeness_dirty,
                        pinned_digest: None,
                        new_digest: Some(*new_observation.value_digest()),
                        new_value: changed_values.get(&pid).map(|e| e.value().to_string()),
                    },
                    None => DependencyEvaluation {
                        projection: pid,
                        changed: false,
                        authoritative: false,
                        completeness: outcome.completeness,
                        completeness_dirty: outcome.completeness_dirty,
                        pinned_digest: None,
                        new_digest: None,
                        new_value: None,
                    },
                };
                if let Some(existing) = artifacts.iter_mut().find(|ao| ao.artifact == aid) {
                    existing.dependency_evaluations.push(evaluation);
                } else {
                    artifacts.push(ArtifactOutcome {
                        artifact: aid,
                        dependency_evaluations: vec![evaluation],
                        verdict: ContractVerdict::Undecidable,
                        appended: None,
                        attestation: None,
                    });
                }
            }
        }

        for ao in artifacts.iter_mut() {
            let artifact = self.store.get_artifact(&ao.artifact)?;
            let history = self.store.attestation_history(&ao.artifact)?;
            let latest = history.iter().last();

            // Resolve the pinned prior value per dependency from the
            // latest attestation's observation evidence.
            for dep in ao.dependency_evaluations.iter_mut() {
                if let Some(att) = latest {
                    dep.pinned_digest = att.evidence().iter().rev().find_map(|e| match e {
                        EvidenceRef::Observation(oid) => {
                            let obs = self.store.get_projection_observation(oid).ok()?;
                            (obs.projection() == *dep.projection())
                                .then(|| obs.value_digest().to_owned())
                        }
                        _ => None,
                    });
                }
                dep.changed = match (&dep.pinned_digest, &dep.new_digest) {
                    (Some(pinned), Some(new)) => pinned != new,
                    _ => true,
                };
            }

            let (verdict, authority) = self.evaluate_contract(&artifact, latest, ao)?;
            let validity = Validity::from(verdict);
            let level = if verdict == ContractVerdict::Undecidable {
                VerificationLevel::Unverified
            } else {
                VerificationLevel::Structural
            };
            let verifier = match verdict {
                ContractVerdict::Undecidable => None,
                ContractVerdict::Holds | ContractVerdict::Fails => {
                    Some(self.contract_id_for(&artifact, latest))
                }
            };
            let evidence: Vec<EvidenceRef> = ao
                .dependency_evaluations
                .iter()
                .filter_map(|d| new_observations.get(&d.projection.clone()))
                .map(|o| EvidenceRef::Observation(o.id()))
                .collect();
            let created_at = self.now + append_seq;
            append_seq += 1;
            let att = ValidationAttestation::for_derivation(
                attestation_identity(
                    &ao.artifact,
                    &self.target_snapshot,
                    validity,
                    authority,
                    level,
                    verifier,
                    &evidence,
                    created_at,
                ),
                ao.artifact,
                artifact.derivation(),
                self.target_snapshot,
                None,
                validity,
                authority,
                level,
                verifier,
                evidence,
                created_at,
            );
            let already = history.iter().any(|a| a.id() == att.id());
            if !already {
                self.store.append_attestation(&att)?;
            }
            ao.verdict = verdict;
            ao.appended = if already { None } else { Some(validity) };
            ao.attestation = Some(att.id());
        }

        projections.sort_by(|a, b| a.projection.id().cmp(&b.projection.id()));
        Ok(TransitionReport {
            target_snapshot: self.target_snapshot,
            projections,
            artifacts,
        })
    }

    fn contract_id_for(
        &self,
        artifact: &trellis_core::artifact::ArtifactEnvelope,
        latest: Option<&ValidationAttestation>,
    ) -> VerifierId {
        latest
            .and_then(|att| att.verifier())
            .unwrap_or_else(|| match artifact.kind() {
                trellis_core::artifact::ArtifactKind::StructuralSet => {
                    contract_verifier_id(SET_EQUALITY_CONTRACT)
                }
                _ => contract_verifier_id(ABSENCE_FACT_CONTRACT),
            })
    }

    /// §18 steps 6-9: evaluate the artifact's validity contract against
    /// the reevaluated dependencies. Returns the internal verdict and
    /// the evidence authority (weakest participating source class).
    fn evaluate_contract(
        &self,
        artifact: &trellis_core::artifact::ArtifactEnvelope,
        latest: Option<&ValidationAttestation>,
        ao: &ArtifactOutcome,
    ) -> Result<(ContractVerdict, Authority), TransitionError> {
        let deps: BTreeSet<ProjectionId> = artifact.dependencies().iter().copied().collect();
        let touched: Vec<&DependencyEvaluation> = ao
            .dependency_evaluations
            .iter()
            .filter(|d| deps.contains(d.projection()))
            .collect();
        if touched.is_empty() {
            return Ok((ContractVerdict::Undecidable, Authority::Provisional));
        }

        // Undecidable when evidence is missing: no prior attestation, a
        // dependency without a pinned value, or no fresh value this
        // round. Conservative UNKNOWN, never auto-reuse (§15).
        let decidable = latest.is_some()
            && touched.iter().all(|d| {
                d.pinned_digest.is_some() && d.new_digest.is_some() && d.new_value.is_some()
            });
        if !decidable {
            return Ok((ContractVerdict::Undecidable, Authority::Provisional));
        }

        let verifier = latest.and_then(|att| att.verifier());
        let verdict = match verifier {
            Some(v) if v == contract_verifier_id(SET_EQUALITY_CONTRACT) => {
                let all_equal = touched.iter().all(|d| d.pinned_digest == d.new_digest);
                if all_equal {
                    ContractVerdict::Holds
                } else {
                    ContractVerdict::Fails
                }
            }
            Some(v) if v == contract_verifier_id(ABSENCE_FACT_CONTRACT) => {
                let all_absent = touched
                    .iter()
                    .all(|d| d.new_value.as_deref().unwrap_or_default().is_empty());
                if all_absent {
                    ContractVerdict::Holds
                } else {
                    ContractVerdict::Fails
                }
            }
            _ => ContractVerdict::Undecidable,
        };
        // Coverage downgrade (§8.2): a completeness-sensitive dependency
        // evaluated under UNKNOWN can no longer establish absence or set
        // claims. If the reevaluated values would otherwise hold, the
        // honest standing is UNKNOWN (never authoritative absence); a
        // claim that would fail on the values alone stays failed.
        let verdict = if touched
            .iter()
            .any(|d| d.completeness == Some(Completeness::Unknown))
        {
            match verdict {
                ContractVerdict::Holds => ContractVerdict::Undecidable,
                other => other,
            }
        } else {
            verdict
        };
        let authoritative = touched.iter().all(|d| d.is_authoritative());
        let authority = if authoritative {
            Authority::Authoritative
        } else {
            Authority::Provisional
        };
        Ok((verdict, authority))
    }
}

/// Content-derived attestation identity: BLAKE3 over every field that
/// distinguishes one validity event from another. Identical transitions
/// therefore produce identical attestations (idempotent appends), and
/// different runs are distinct events.
#[allow(clippy::too_many_arguments)]
fn attestation_identity(
    artifact: &ArtifactId,
    snapshot: &SnapshotId,
    validity: Validity,
    authority: Authority,
    level: VerificationLevel,
    verifier: Option<VerifierId>,
    evidence: &[EvidenceRef],
    created_at: Timestamp,
) -> AttestationId {
    let mut material = String::new();
    material.push_str("trellis.attestation-event.v1");
    for part in [
        artifact.to_string(),
        snapshot.to_string(),
        format!("{validity:?}"),
        format!("{authority:?}"),
        format!("{level:?}"),
        verifier.map(|v| v.to_string()).unwrap_or_default(),
    ] {
        material.push('\u{1f}');
        material.push_str(&part);
    }
    for e in evidence {
        material.push('\u{1f}');
        material.push_str(&match e {
            EvidenceRef::Blob(b) => format!("blob:{b}"),
            EvidenceRef::Observation(o) => format!("obs:{o}"),
            EvidenceRef::PriorAttestation(a) => format!("att:{a}"),
        });
    }
    material.push('\u{1f}');
    material.push_str(&created_at.to_string());
    AttestationId::from_hash(ContentHash::compute(HashAlgo::Blake3, material.as_bytes()))
}

/// A semantic source backed by a frozen authoritative snapshot's
/// normalized graph (M8): Callers/Implementations values come from the
/// frozen SCIP graph; other kinds delegate to the syntactic backend.
/// This is production wiring for `Transition` once a freeze has run.
pub struct FrozenSemanticSource<'a> {
    syntactic: SyntacticSource<'a>,
    graph: &'a trellis_scip::ScipGraph,
}

impl<'a> FrozenSemanticSource<'a> {
    /// Compose the frozen graph with the syntactic index.
    #[must_use]
    pub fn new(graph: &'a trellis_scip::ScipGraph, index: &'a PythonSyntaxIndex) -> Self {
        Self {
            syntactic: SyntacticSource::new(index),
            graph,
        }
    }
}

impl ReevaluationSource for FrozenSemanticSource<'_> {
    fn evaluate(&self, projection: &Projection) -> Result<Option<Evaluated>, TransitionError> {
        match projection.kind() {
            ProjectionKind::Callers => Ok(Some(Evaluated::new(
                self.graph.callers_value(projection.subject().canonical()),
                true,
            ))),
            ProjectionKind::Implementations => Ok(Some(Evaluated::new(
                self.graph
                    .implementations_value(projection.subject().canonical()),
                true,
            ))),
            _ => self.syntactic.evaluate(projection),
        }
    }
}

/// A semantic source backed by the frozen SCIP graph **plus live
/// syntactic evidence over the current tree** (M9, spec §9.2): the
/// provisional live overlay for mid-task navigation.
///
/// Composition: for semantic kinds, the overlay re-resolves call sites
/// textually over the current tree (tree-sitter layer) and unions the
/// result with the frozen graph's members restricted to files NOT in
/// the current delta — a caller added after the freeze is visible, a
/// caller whose defining file was removed disappears, and frozen
/// members from untouched files are retained. Evidence class is
/// **provisional** (§9.2: overlay results can never be authoritative;
/// completeness-sensitive authority is impossible here — the overlay's
/// certificate capabilities are Unproven by construction).
pub struct OverlaySemanticSource<'a> {
    syntactic: SyntacticSource<'a>,
    graph: &'a trellis_scip::ScipGraph,
    /// Files the reconcile delta touched (added/modified): their frozen
    /// members are superseded by live evaluation.
    delta_paths: std::collections::BTreeSet<String>,
    /// Live textual call-graph over the current tree (caller member →
    /// dotted callee). Produced by the harness/backend layer; the
    /// overlay composes it over the frozen graph.
    live_calls: BTreeMap<String, Vec<String>>,
}

impl<'a> OverlaySemanticSource<'a> {
    /// Compose the frozen graph with the live delta view.
    #[must_use]
    pub fn new(
        graph: &'a trellis_scip::ScipGraph,
        index: &'a PythonSyntaxIndex,
        delta_paths: impl IntoIterator<Item = impl Into<String>>,
        live_calls: BTreeMap<String, Vec<String>>,
    ) -> Self {
        Self {
            syntactic: SyntacticSource::new(index),
            graph,
            delta_paths: delta_paths.into_iter().map(Into::into).collect(),
            live_calls,
        }
    }
}

impl ReevaluationSource for OverlaySemanticSource<'_> {
    fn evaluate(&self, projection: &Projection) -> Result<Option<Evaluated>, TransitionError> {
        match projection.kind() {
            ProjectionKind::Callers => {
                let subject = projection.subject().canonical();
                // Frozen members from files NOT touched by the delta.
                // Delta paths normalize exactly as the SCIP ingest
                // normalizes document paths: `auth/__init__.py` IS
                // module `auth` (never `auth.__init__`) — the same
                // `/__init__` strip the ingest applies. Mismatches here
                // would retain stale frozen callers (a missed
                // invalidation, §2.1).
                let delta_modules: std::collections::BTreeSet<String> = self
                    .delta_paths
                    .iter()
                    .filter_map(|p| {
                        p.strip_suffix(".py").map(|s| {
                            let dotted = s.replace('/', ".");
                            dotted
                                .strip_suffix(".__init__")
                                .unwrap_or(&dotted)
                                .to_string()
                        })
                    })
                    .collect();
                let mut members: Vec<String> = self
                    .graph
                    .callers
                    .get(subject)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|m| {
                        let def_module = m.rsplit_once('.').map_or(m.as_str(), |(mo, _)| mo);
                        !delta_modules.contains(def_module)
                    })
                    .collect();
                // Live members from the current tree.
                members.extend(self.live_calls.get(subject).cloned().unwrap_or_default());
                members.sort();
                members.dedup();
                Ok(Some(Evaluated::new(members.join("; "), false)))
            }
            ProjectionKind::Implementations => {
                // Inheritance edges come only from a semantic backend;
                // the overlay cannot re-derive them syntactically, so
                // the frozen value is served (best-effort navigation)
                // and marked provisional.
                Ok(Some(Evaluated::new(
                    self.graph
                        .implementations_value(projection.subject().canonical()),
                    false,
                )))
            }
            _ => self.syntactic.evaluate(projection),
        }
    }
}

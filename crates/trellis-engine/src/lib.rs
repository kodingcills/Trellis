//! trellis-engine — conservative dirty-candidate discovery (spec §18 steps
//! 1-3, M5).
//!
//! M5 answers exactly one question: given a source/program change, **which
//! stored ProjectionObservations MAY be affected?** It never decides that
//! an artifact is semantically stale (M6), never executes verifiers, never
//! performs semantic equality propagation.
//!
//! Soundness rule (spec §2.1): candidate discovery may over-approximate —
//! an unnecessary reevaluation is acceptable — but must never omit a
//! potentially affected projection, because that would allow stale reuse
//! without reevaluation. Three concepts stay distinct in this API:
//! *candidate affected* (here) ≠ *projection value changed* (M6
//! reevaluation) ≠ *artifact stale* (M6 contract evaluation).

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod discovery;
pub mod observe;
pub mod redgreen;

/// Everything a consumer of the dependency engine typically needs.
pub mod prelude {
    pub use crate::discovery::{
        recorded_from_store, reevaluation_candidates, RecordedObservation, ReevaluationCandidates,
        SymbolLocation,
    };
    pub use crate::observe::{
        observe_definition, observe_file_digest, observe_imports, observe_signature,
        record_unit_index,
    };
    pub use crate::redgreen::{
        contract_verifier_id, ContractVerdict, DependencyEvaluation, Evaluated, ProjectionOutcome,
        ProjectionOutcomeKind, ReevaluationSource, SyntacticSource, Transition, TransitionError,
        TransitionReport, ABSENCE_FACT_CONTRACT, SET_EQUALITY_CONTRACT,
    };
}

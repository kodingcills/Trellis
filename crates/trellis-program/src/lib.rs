//! trellis-program — backend-independent program index (spec §9.3, M3).
//!
//! The pipeline: source snapshot (M1) → Python source units → tree-sitter
//! parse → normalized Trellis program representation → deterministic
//! syntax-grounded projection values.
//!
//! **Semantic boundary (frozen, M3):** tree-sitter proves syntactic facts
//! about a *single unit* — definitions, normalized signatures, imports,
//! parse status. It does NOT resolve cross-file semantics; queries for
//! references/callers/implementations/subclasses return
//! [`index::Answer::Unsupported`], never an empty set. A semantic provider
//! (SCIP adapter, M8) plugs in behind the same [`index::ProgramIndex`]
//! trait without changing projection semantics.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod index;
pub mod model;
pub mod python;

/// Everything a consumer of the program layer typically needs.
pub mod prelude {
    pub use crate::index::{Answer, ProgramIndex, Unsupported};
    pub use crate::model::{
        DefKind, Definition, Import, ModuleId, Param, ParamKind, ParseStatus, Signature,
        SourceSpan, SymbolPath,
    };
    pub use crate::python::PythonSyntaxIndex;
}

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod unit_tests;

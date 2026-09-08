//! Backend-independent program index abstraction (spec §9.3).
//!
//! The trait distinguishes **proven** results from **unsupported** ones at
//! the type level: a semantic cross-file query answered by a backend
//! without semantic-resolution capability returns
//! [`Answer::Unsupported`], never an empty set. An empty set can only be
//! produced by [`Answer::Proven`] — and then it is an authoritative
//! absence from a provider whose certificate claims the required
//! capabilities (spec §8).

/// Outcome of a projection query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer<T> {
    /// The backend proved this value under its declared capabilities.
    Proven(T),
    /// The backend cannot answer this query. Carries the reason; the
    /// absence of a value here must NEVER be interpreted as an empty set
    /// or an absence claim (spec §8: uncertainty is UNKNOWN, not absence).
    Unsupported(Unsupported),
}

impl<T> Answer<T> {
    /// The proven value, if any.
    #[must_use]
    pub fn proven(self) -> Option<T> {
        match self {
            Answer::Proven(v) => Some(v),
            Answer::Unsupported(_) => None,
        }
    }

    /// Whether the query was answered with a proven value.
    #[must_use]
    pub const fn is_proven(&self) -> bool {
        matches!(self, Answer::Proven(_))
    }
}

/// Why a backend could not answer a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    /// The query requires cross-file semantic resolution (references,
    /// callers, implementations, precise subclasses). A syntax-only
    /// backend can never prove these.
    RequiresSemanticResolution,
    /// The backend does not cover the requested unit (e.g. out of the
    /// indexed universe).
    UnitNotIndexed(String),
    /// The unit failed to parse, so extraction is incomplete: results that
    /// depend on its full contents cannot be proven. Never confuse this
    /// with an empty result — the true set is unknown (spec §8).
    ParseIncomplete(String),
    /// The backend lacks a required capability for another reason.
    MissingCapability(&'static str),
}

/// Backend-independent program index (spec §9.3).
///
/// Two query families, never conflated:
///
/// - **Syntactic facts** (definition, signature, imports, parse status):
///   provable deterministically from a parse of one source unit.
/// - **Semantic cross-file facts** (references, callers, implementations,
///   precise subclasses): require a semantic provider. A syntax-only
///   backend must return [`Answer::Unsupported`].
pub trait ProgramIndex {
    /// Parse status of a source unit.
    fn parse_status(&self, module: &crate::model::ModuleId) -> Answer<crate::model::ParseStatus>;

    /// Syntactic definition lookup by canonical symbol path. `None` is
    /// proven syntactic absence **only for cleanly parsed units**; failed
    /// parses return Unsupported (spec §8).
    fn definition(
        &self,
        symbol: &crate::model::SymbolPath,
    ) -> Answer<Option<crate::model::Definition>>;

    /// Normalized signature of a function/method/async function.
    fn signature(
        &self,
        symbol: &crate::model::SymbolPath,
    ) -> Answer<Option<crate::model::Signature>>;

    /// All definitions declared in a module, in deterministic order
    /// (source position). Unsupported for units whose parse failed —
    /// extraction from incomplete evidence is never proven (spec §8).
    fn definitions_in(
        &self,
        module: &crate::model::ModuleId,
    ) -> Answer<Vec<crate::model::Definition>>;

    /// Syntactic imports of a module, in source order. Unsupported for
    /// failed-parse units (extraction could be incomplete, spec §8).
    fn imports(&self, module: &crate::model::ModuleId) -> Answer<Vec<crate::model::Import>>;

    /// Map a reconcile changed-file set (canonical relative paths) to the
    /// syntactically affected symbols. Proven only when every changed
    /// Python unit parsed cleanly; a failed-parse unit in the set makes
    /// the candidate set provably incomplete and downgrades the whole
    /// answer to [`Unsupported::ParseIncomplete`] (over-invalidation is
    /// safe; a missed symbol is not, spec §2.1).
    fn changed_symbols(&self, changed_paths: &[String]) -> Answer<Vec<crate::model::SymbolPath>>;

    /// Content digest of a tracked Python file — the `file(path)`
    /// projection (spec §20). Deterministic and content-derived; `None`
    /// only for a canonical `.py` path absent from the indexed tree.
    /// Non-Python or non-canonical paths are outside the indexed universe
    /// and return Unsupported (not proven absence, spec §8).
    fn file_digest(&self, path: &str) -> Answer<Option<trellis_core::ids::ContentHash>>;

    /// Semantic: all references to a symbol. **Semantic cross-file.**
    fn references(
        &self,
        symbol: &crate::model::SymbolPath,
    ) -> Answer<Vec<crate::model::SymbolPath>>;

    /// Semantic: all symbols that call a symbol. **Semantic cross-file.**
    fn callers(&self, symbol: &crate::model::SymbolPath) -> Answer<Vec<crate::model::SymbolPath>>;

    /// Semantic: all classes implementing a trait/interface/protocol.
    /// **Semantic cross-file.**
    fn implementations(
        &self,
        symbol: &crate::model::SymbolPath,
    ) -> Answer<Vec<crate::model::SymbolPath>>;

    /// Semantic: precise subclass relation. **Semantic cross-file.**
    fn subclasses(
        &self,
        symbol: &crate::model::SymbolPath,
    ) -> Answer<Vec<crate::model::SymbolPath>>;
}

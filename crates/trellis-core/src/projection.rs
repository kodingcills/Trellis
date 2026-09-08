//! Dependency projections — functions over world state whose values matter
//! to artifacts (spec §7, §20). The most important type in the architecture.

use std::fmt;

use crate::error::DomainError;
use crate::ids::{BlobId, ContentHash, ProjectionId, ProjectionObservationId, SnapshotId};

/// The property a projection queries. Kept deliberately aligned with
/// [`ProjectionKind`] so the two can never disagree (invariant enforced by
/// construction and by [`Projection::validate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Property {
    /// Content of a file.
    Content,
    /// Definition of a symbol.
    Definition,
    /// Signature of a symbol.
    Signature,
    /// The set of references to a symbol.
    ReferenceSet,
    /// The set of callers of a symbol.
    CallerSet,
    /// The set of implementations of a trait/protocol/interface.
    ImplementationSet,
    /// The import set of a module.
    ImportSet,
    /// The set of subclasses of a symbol.
    SubclassSet,
    /// Result of a repository search.
    SearchResult,
    /// Value of a configuration key.
    ConfigValue,
    /// Version of a selected tool.
    ToolVersion,
}

impl Property {
    /// The unique property consistent with `kind`, or `None` if the pair is
    /// inconsistent. Every constructor routes through this, so a
    /// constructed `Projection` can never carry a mismatched pair.
    #[must_use]
    pub fn for_kind(kind: ProjectionKind) -> Option<Property> {
        Some(match kind {
            ProjectionKind::FileContent => Property::Content,
            ProjectionKind::Definition => Property::Definition,
            ProjectionKind::Signature => Property::Signature,
            ProjectionKind::References => Property::ReferenceSet,
            ProjectionKind::Callers => Property::CallerSet,
            ProjectionKind::Implementations => Property::ImplementationSet,
            ProjectionKind::Imports => Property::ImportSet,
            ProjectionKind::Subclasses => Property::SubclassSet,
            ProjectionKind::RepositorySearch => Property::SearchResult,
            ProjectionKind::ConfigValue => Property::ConfigValue,
            ProjectionKind::ToolVersion => Property::ToolVersion,
        })
    }

    /// The kind this property belongs to.
    #[must_use]
    pub const fn kind(self) -> ProjectionKind {
        match self {
            Property::Content => ProjectionKind::FileContent,
            Property::Definition => ProjectionKind::Definition,
            Property::Signature => ProjectionKind::Signature,
            Property::ReferenceSet => ProjectionKind::References,
            Property::CallerSet => ProjectionKind::Callers,
            Property::ImplementationSet => ProjectionKind::Implementations,
            Property::ImportSet => ProjectionKind::Imports,
            Property::SubclassSet => ProjectionKind::Subclasses,
            Property::SearchResult => ProjectionKind::RepositorySearch,
            Property::ConfigValue => ProjectionKind::ConfigValue,
            Property::ToolVersion => ProjectionKind::ToolVersion,
        }
    }
}

/// The kind of a dependency projection (spec §20 query DSL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectionKind {
    /// Content of a tracked file.
    FileContent,
    /// Definition of a symbol.
    Definition,
    /// Signature of a symbol.
    Signature,
    /// References to a symbol within a scope.
    References,
    /// Callers of a symbol within a scope.
    Callers,
    /// Implementations of a trait/protocol/interface within a scope.
    Implementations,
    /// Imports of a module within a scope.
    Imports,
    /// Subclasses of a symbol within a scope.
    Subclasses,
    /// Repository text/pattern search within a scope.
    RepositorySearch,
    /// A selected configuration value.
    ConfigValue,
    /// A selected tool/runtime version.
    ToolVersion,
}

impl ProjectionKind {
    /// Canonical keyword used in rendering and (later) canonical keys.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            ProjectionKind::FileContent => "file",
            ProjectionKind::Definition => "definition",
            ProjectionKind::Signature => "signature",
            ProjectionKind::References => "references",
            ProjectionKind::Callers => "callers",
            ProjectionKind::Implementations => "implementations",
            ProjectionKind::Imports => "imports",
            ProjectionKind::Subclasses => "subclasses",
            ProjectionKind::RepositorySearch => "repository_search",
            ProjectionKind::ConfigValue => "config_value",
            ProjectionKind::ToolVersion => "tool_version",
        }
    }

    /// Whether queries of this kind can assert absence/completeness claims
    /// and therefore bind to a universe + coverage certificate (spec §8).
    /// Populated per projection kind as the completeness policies land (M7);
    /// callers, references, implementations, and repository searches are.
    #[must_use]
    pub const fn is_completeness_sensitive(self) -> bool {
        matches!(
            self,
            ProjectionKind::Callers
                | ProjectionKind::References
                | ProjectionKind::Implementations
                | ProjectionKind::Subclasses
                | ProjectionKind::RepositorySearch
        )
    }
}

/// The subject a projection queries, in canonical form. Normalization rules
/// (paths, symbol identifiers) are finalized in M1/M3; M0 only guarantees
/// non-emptiness.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Subject {
    /// A tracked file path.
    File(String),
    /// A canonical program symbol identifier.
    Symbol(String),
    /// A module identifier.
    Module(String),
    /// A search pattern.
    Text(String),
    /// A configuration key.
    ConfigKey(String),
    /// A tool or runtime name.
    Tool(String),
}

impl Subject {
    fn validate(&self) -> Result<(), DomainError> {
        let empty = match self {
            Subject::File(s)
            | Subject::Symbol(s)
            | Subject::Module(s)
            | Subject::Text(s)
            | Subject::ConfigKey(s)
            | Subject::Tool(s) => s.trim().is_empty(),
        };
        if empty {
            Err(DomainError::EmptySubject)
        } else {
            Ok(())
        }
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Subject::File(s) | Subject::Symbol(s) | Subject::Module(s) => f.write_str(s),
            Subject::Text(s) => write!(f, "\"{s}\""),
            Subject::ConfigKey(s) | Subject::Tool(s) => f.write_str(s),
        }
    }
}

/// The scope a projection is evaluated over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// A single file.
    File,
    /// A single module.
    Module,
    /// A package (project-level unit).
    Package,
    /// The whole repository. Used by absence claims; binds the universe
    /// (spec §8).
    Repository,
}

/// A dependency projection: kind + subject + property + scope (spec §7).
///
/// `property` is kept consistent with `kind` by construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Projection {
    kind: ProjectionKind,
    subject: Subject,
    property: Property,
    scope: Scope,
}

impl Projection {
    /// General constructor. Returns an error if the subject is empty or the
    /// property is inconsistent with the kind.
    pub fn new(
        kind: ProjectionKind,
        subject: Subject,
        property: Property,
        scope: Scope,
    ) -> Result<Self, DomainError> {
        subject.validate()?;
        if property.kind() != kind {
            return Err(DomainError::InconsistentProperty {
                kind: kind.keyword(),
                property: property.kind().keyword(),
            });
        }
        Ok(Self {
            kind,
            subject,
            property,
            scope,
        })
    }

    fn simple(kind: ProjectionKind, subject: Subject, scope: Scope) -> Self {
        let property = Property::for_kind(kind).expect("every kind has a property");
        Self {
            kind,
            subject,
            property,
            scope,
        }
    }

    /// `file(path)`
    pub fn file(path: impl Into<String>) -> Result<Self, DomainError> {
        let s = Subject::File(path.into());
        s.validate()?;
        Ok(Self::simple(ProjectionKind::FileContent, s, Scope::File))
    }

    /// `definition(symbol)`
    pub fn definition(symbol: impl Into<String>) -> Result<Self, DomainError> {
        let s = Subject::Symbol(symbol.into());
        s.validate()?;
        Ok(Self::simple(
            ProjectionKind::Definition,
            s,
            Scope::Repository,
        ))
    }

    /// `signature(symbol)`
    pub fn signature(symbol: impl Into<String>) -> Result<Self, DomainError> {
        let s = Subject::Symbol(symbol.into());
        s.validate()?;
        Ok(Self::simple(
            ProjectionKind::Signature,
            s,
            Scope::Repository,
        ))
    }

    /// `references(symbol, scope)`
    pub fn references(symbol: impl Into<String>, scope: Scope) -> Result<Self, DomainError> {
        let s = Subject::Symbol(symbol.into());
        s.validate()?;
        Ok(Self::simple(ProjectionKind::References, s, scope))
    }

    /// `callers(symbol, scope)`
    pub fn callers(symbol: impl Into<String>, scope: Scope) -> Result<Self, DomainError> {
        let s = Subject::Symbol(symbol.into());
        s.validate()?;
        Ok(Self::simple(ProjectionKind::Callers, s, scope))
    }

    /// `implementations(symbol, scope)`
    pub fn implementations(symbol: impl Into<String>, scope: Scope) -> Result<Self, DomainError> {
        let s = Subject::Symbol(symbol.into());
        s.validate()?;
        Ok(Self::simple(ProjectionKind::Implementations, s, scope))
    }

    /// `imports(module, scope)`
    pub fn imports(module: impl Into<String>, scope: Scope) -> Result<Self, DomainError> {
        let s = Subject::Module(module.into());
        s.validate()?;
        Ok(Self::simple(ProjectionKind::Imports, s, scope))
    }

    /// `subclasses(symbol, scope)`
    pub fn subclasses(symbol: impl Into<String>, scope: Scope) -> Result<Self, DomainError> {
        let s = Subject::Symbol(symbol.into());
        s.validate()?;
        Ok(Self::simple(ProjectionKind::Subclasses, s, scope))
    }

    /// `repository_search(pattern, scope)`
    pub fn repository_search(
        pattern: impl Into<String>,
        scope: Scope,
    ) -> Result<Self, DomainError> {
        let s = Subject::Text(pattern.into());
        s.validate()?;
        Ok(Self::simple(ProjectionKind::RepositorySearch, s, scope))
    }

    /// `config_value(key)`
    pub fn config_value(key: impl Into<String>) -> Result<Self, DomainError> {
        let s = Subject::ConfigKey(key.into());
        s.validate()?;
        Ok(Self::simple(
            ProjectionKind::ConfigValue,
            s,
            Scope::Repository,
        ))
    }

    /// `tool_version(tool)`
    pub fn tool_version(tool: impl Into<String>) -> Result<Self, DomainError> {
        let s = Subject::Tool(tool.into());
        s.validate()?;
        Ok(Self::simple(
            ProjectionKind::ToolVersion,
            s,
            Scope::Repository,
        ))
    }

    /// The projection kind.
    #[must_use]
    pub const fn kind(&self) -> ProjectionKind {
        self.kind
    }

    /// The queried subject.
    #[must_use]
    pub const fn subject(&self) -> &Subject {
        &self.subject
    }

    /// The queried property (always consistent with [`Self::kind`]).
    #[must_use]
    pub const fn property(&self) -> Property {
        self.property
    }

    /// The evaluation scope.
    #[must_use]
    pub const fn scope(&self) -> Scope {
        self.scope
    }

    /// Construction-time invariant check. Succeeds for every value
    /// produced by a constructor; public for completeness.
    pub fn validate(&self) -> Result<(), DomainError> {
        self.subject.validate()?;
        if self.property.kind() != self.kind {
            return Err(DomainError::InconsistentProperty {
                kind: self.kind.keyword(),
                property: self.property.kind().keyword(),
            });
        }
        Ok(())
    }
}

impl fmt::Display for Projection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}({}, scope={:?})",
            self.kind.keyword(),
            self.subject,
            self.scope
        )
    }
}

/// One observation of a projection's value at one snapshot (spec §7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionObservation {
    id: ProjectionObservationId,
    projection: ProjectionId,
    snapshot: SnapshotId,
    value_digest: ContentHash,
    canonical_value: Option<BlobId>,
}

impl ProjectionObservation {
    /// Construct an observation. `canonical_value` references the CAS blob
    /// holding the canonical serialized value, if the value was
    /// materialized (spec §11 granularity policy).
    #[must_use]
    pub const fn new(
        id: ProjectionObservationId,
        projection: ProjectionId,
        snapshot: SnapshotId,
        value_digest: ContentHash,
        canonical_value: Option<BlobId>,
    ) -> Self {
        Self {
            id,
            projection,
            snapshot,
            value_digest,
            canonical_value,
        }
    }

    /// Identity of this observation.
    #[must_use]
    pub const fn id(&self) -> ProjectionObservationId {
        self.id
    }

    /// The observed projection.
    #[must_use]
    pub const fn projection(&self) -> ProjectionId {
        self.projection
    }

    /// The snapshot the value was observed at.
    #[must_use]
    pub const fn snapshot(&self) -> SnapshotId {
        self.snapshot
    }

    /// Digest of the canonical projection value.
    #[must_use]
    pub const fn value_digest(&self) -> &ContentHash {
        &self.value_digest
    }

    /// CAS blob holding the canonical value, if materialized.
    #[must_use]
    pub const fn canonical_value(&self) -> Option<BlobId> {
        self.canonical_value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::HashAlgo;
    use std::str::FromStr;

    fn h(seed: u8) -> ContentHash {
        let d: Vec<u8> = (0..32).map(|i| seed.wrapping_add(i)).collect();
        ContentHash::from_bytes(HashAlgo::Blake3, &d).unwrap()
    }

    #[test]
    fn callers_constructor_binds_kind_property_scope() {
        let p = Projection::callers("app.auth.verify", Scope::Repository).unwrap();
        assert_eq!(p.kind(), ProjectionKind::Callers);
        assert_eq!(p.property(), Property::CallerSet);
        assert_eq!(p.scope(), Scope::Repository);
        assert!(p.kind().is_completeness_sensitive());
        assert_eq!(p.to_string(), "callers(app.auth.verify, scope=Repository)");
    }

    #[test]
    fn empty_subject_rejected() {
        assert_eq!(
            Projection::callers("   ", Scope::Repository).unwrap_err(),
            DomainError::EmptySubject
        );
        assert!(Projection::file("").is_err());
    }

    #[test]
    fn inconsistent_property_rejected() {
        let err = Projection::new(
            ProjectionKind::Callers,
            Subject::Symbol("x".into()),
            Property::Signature,
            Scope::Repository,
        )
        .unwrap_err();
        assert!(matches!(err, DomainError::InconsistentProperty { .. }));
    }

    #[test]
    fn file_content_is_not_completeness_sensitive() {
        let p = Projection::file("src/auth.py").unwrap();
        assert!(!p.kind().is_completeness_sensitive());
    }

    #[test]
    fn observation_holds_digest_and_optional_blob() {
        let obs = ProjectionObservation::new(
            ProjectionObservationId::from_hash(h(1)),
            ProjectionId::from_hash(h(2)),
            SnapshotId::from_hash(h(3)),
            h(4),
            Some(BlobId::from_hash(h(5))),
        );
        assert_eq!(obs.value_digest(), &h(4));
        assert!(obs.canonical_value().is_some());
        assert_eq!(
            ProjectionObservationId::from_str(&obs.id().to_string()).unwrap(),
            obs.id()
        );
    }
}

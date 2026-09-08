//! Snapshots — explicit, on-demand coherent freezes of the world state
//! (spec §6).

use crate::error::DomainError;
use crate::ids::{
    EnvironmentFingerprintId, GitOid, ManifestId, RepositoryId, SemanticSnapshotId, SnapshotId,
    Timestamp,
};

/// One version of the world Trellis has indexed.
///
/// A snapshot is a **coherent freeze**, not a copy: it references a
/// working-tree manifest and whatever semantic index the freeze carries.
/// Snapshots are created on demand — at task boundaries, explicit
/// `trellis freeze`, or when a requested authoritative decision depends on
/// semantic projections whose current semantic snapshot is not provably
/// compatible with the current source state (spec §6 freeze rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    id: SnapshotId,
    repository_id: RepositoryId,
    git_commit: Option<GitOid>,
    working_tree_manifest: ManifestId,
    semantic_snapshot: Option<SemanticSnapshotId>,
    environment: EnvironmentFingerprintId,
    parent: Option<SnapshotId>,
    created_at: Timestamp,
}

impl Snapshot {
    /// Construct a snapshot.
    ///
    /// # Invariants
    /// - A snapshot is never its own parent.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SnapshotId,
        repository_id: RepositoryId,
        git_commit: Option<GitOid>,
        working_tree_manifest: ManifestId,
        semantic_snapshot: Option<SemanticSnapshotId>,
        environment: EnvironmentFingerprintId,
        parent: Option<SnapshotId>,
        created_at: Timestamp,
    ) -> Result<Self, DomainError> {
        if parent == Some(id) {
            return Err(DomainError::SelfParent);
        }
        Ok(Self {
            id,
            repository_id,
            git_commit,
            working_tree_manifest,
            semantic_snapshot,
            environment,
            parent,
            created_at,
        })
    }

    /// Identity of this snapshot.
    #[must_use]
    pub const fn id(&self) -> SnapshotId {
        self.id
    }

    /// Repository this snapshot indexes.
    #[must_use]
    pub const fn repository_id(&self) -> RepositoryId {
        self.repository_id
    }

    /// Git commit this snapshot corresponds to, if any. Informational only:
    /// the working-tree manifest is authoritative in v0.1 (spec §6).
    #[must_use]
    pub const fn git_commit(&self) -> Option<GitOid> {
        self.git_commit
    }

    /// Manifest identifying the exact tracked file state of the freeze.
    #[must_use]
    pub const fn working_tree_manifest(&self) -> ManifestId {
        self.working_tree_manifest
    }

    /// Authoritative semantic snapshot carried by this freeze, if any.
    /// Present once the SCIP freeze adapter exists (spec §9.1, M8).
    #[must_use]
    pub const fn semantic_snapshot(&self) -> Option<SemanticSnapshotId> {
        self.semantic_snapshot
    }

    /// Declared environment fingerprint (spec §49: explicit inputs only).
    #[must_use]
    pub const fn environment(&self) -> EnvironmentFingerprintId {
        self.environment
    }

    /// Parent freeze, for incremental change analysis.
    #[must_use]
    pub const fn parent(&self) -> Option<SnapshotId> {
        self.parent
    }

    /// Creation time (unix epoch millis, UTC).
    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ContentHash;
    use std::str::FromStr;

    fn id(seed: u8) -> SnapshotId {
        let d: Vec<u8> = (0..32).map(|i| seed.wrapping_add(i)).collect();
        SnapshotId::from_hash(ContentHash::from_bytes(crate::ids::HashAlgo::Blake3, &d).unwrap())
    }

    fn other_id(seed: u8) -> ManifestId {
        let d: Vec<u8> = (0..32).map(|i| seed.wrapping_add(i)).collect();
        ManifestId::from_hash(ContentHash::from_bytes(crate::ids::HashAlgo::Blake3, &d).unwrap())
    }

    #[test]
    fn self_parent_rejected() {
        let s = id(1);
        let err = Snapshot::new(
            s,
            RepositoryId::from_hash(s.hash().to_owned()),
            None,
            other_id(2),
            None,
            env(),
            Some(s),
            0,
        )
        .unwrap_err();
        assert_eq!(err, DomainError::SelfParent);
    }

    fn env() -> EnvironmentFingerprintId {
        let d: Vec<u8> = (0..32).map(|i| 90 + i).collect();
        EnvironmentFingerprintId::from_hash(
            ContentHash::from_bytes(crate::ids::HashAlgo::Blake3, &d).unwrap(),
        )
    }

    #[test]
    fn valid_snapshot_roundtrips_fields() {
        let commit = GitOid::from_str("0123456789abcdef0123456789abcdef01234567").unwrap();
        let snap = Snapshot::new(
            id(1),
            RepositoryId::from_hash(id(1).hash().to_owned()),
            Some(commit),
            other_id(2),
            None,
            env(),
            None,
            42,
        )
        .unwrap();
        assert_eq!(snap.git_commit(), Some(commit));
        assert_eq!(snap.created_at(), 42);
        assert_eq!(snap.parent(), None);
        assert_eq!(snap.semantic_snapshot(), None);
    }
}

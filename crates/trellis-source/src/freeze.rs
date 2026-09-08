//! Snapshot freezing — coherent on-demand world freezes (spec §6).
//!
//! A freeze materializes a [`Snapshot`] from the current reconciled state:
//! manifest identity, declared environment, parent, and creation time are
//! canonically encoded and hashed into the snapshot identity.

use trellis_core::canonical::{write_bytes, write_str, write_u64};
use trellis_core::error::DomainError;
use trellis_core::ids::{ContentHash, GitOid, HashAlgo, RepositoryId, SnapshotId, Timestamp};
use trellis_core::snapshot::Snapshot;

use crate::environment::EnvironmentFingerprint;
use crate::manifest::Manifest;

/// Freeze the current world state into a snapshot.
///
/// The snapshot identity is computed over the canonical encoding of
/// `(manifest id, environment id, parent, git commit, created_at)`, so two
/// freezes of identical state at identical times share identity, while any
/// drift produces a new snapshot. `semantic_snapshot` is always `None` in
/// M1 — authoritative semantic snapshots arrive with the SCIP adapter (M8).
pub fn freeze(
    repository_id: RepositoryId,
    manifest: &Manifest,
    environment: &EnvironmentFingerprint,
    parent: Option<SnapshotId>,
    git_commit: Option<GitOid>,
    created_at: Timestamp,
) -> Result<Snapshot, DomainError> {
    let mut buf = Vec::new();
    write_str(&mut buf, "trellis.snapshot.v1");
    write_bytes(&mut buf, manifest.id().hash().digest());
    write_bytes(&mut buf, environment.id().hash().digest());
    match parent {
        Some(p) => write_bytes(&mut buf, p.hash().digest()),
        None => write_u64(&mut buf, 0),
    }
    match git_commit {
        Some(c) => write_bytes(&mut buf, c.as_bytes()),
        None => write_u64(&mut buf, 0),
    }
    write_u64(&mut buf, created_at);

    let id = SnapshotId::from_hash(ContentHash::compute(HashAlgo::Blake3, &buf));
    Snapshot::new(
        id,
        repository_id,
        git_commit,
        manifest.id(),
        None,
        environment.id(),
        parent,
        created_at,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::EnvironmentFingerprint;
    use crate::manifest::Manifest;
    use trellis_core::ids::RepositoryId;

    fn repo() -> RepositoryId {
        RepositoryId::from_hash(ContentHash::compute(HashAlgo::Blake3, b"repo:fixture"))
    }

    fn env() -> EnvironmentFingerprint {
        EnvironmentFingerprint::from_declared(&[("python".into(), "3.12".into())]).unwrap()
    }

    #[test]
    fn freeze_is_reproducible_for_identical_state() {
        let manifest = Manifest::from_entries(vec![]).unwrap();
        let e = env();
        let s1 = freeze(repo(), &manifest, &e, None, None, 1_000).unwrap();
        let s2 = freeze(repo(), &manifest, &e, None, None, 1_000).unwrap();
        assert_eq!(s1.id(), s2.id());
        assert_eq!(s1.working_tree_manifest(), manifest.id());
        assert_eq!(s1.semantic_snapshot(), None);
    }

    #[test]
    fn drift_produces_new_snapshot_identity() {
        let manifest_a = Manifest::from_entries(vec![]).unwrap();
        let manifest_b = Manifest::from_entries(vec![crate::manifest::ManifestEntry {
            path: "a.py".into(),
            digest: ContentHash::compute(HashAlgo::Blake3, b"alpha"),
        }])
        .unwrap();
        let e = env();
        let s1 = freeze(repo(), &manifest_a, &e, None, None, 1_000).unwrap();
        let s2 = freeze(repo(), &manifest_b, &e, None, None, 1_000).unwrap();
        assert_ne!(s1.id(), s2.id());

        let s3 = freeze(repo(), &manifest_a, &e, None, None, 2_000).unwrap();
        assert_ne!(s1.id(), s3.id(), "freeze time participates in identity");
    }

    #[test]
    fn self_parent_still_guarded_through_freeze() {
        let manifest = Manifest::from_entries(vec![]).unwrap();
        let e = env();
        let parent = freeze(repo(), &manifest, &e, None, None, 1).unwrap();
        // A frozen snapshot can never list itself as its own parent: the id
        // is computed from inputs that exclude the resulting snapshot id.
        let child = freeze(repo(), &manifest, &e, Some(parent.id()), None, 2).unwrap();
        assert_eq!(child.parent(), Some(parent.id()));
    }
}

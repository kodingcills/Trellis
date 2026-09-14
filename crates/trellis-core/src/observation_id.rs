//! Canonical observation identity (spec §19): the single derivation for
//! recording and verification of ProjectionObservation ids.

use crate::canonical::{write_bytes, write_str};
use crate::ids::{ContentHash, HashAlgo, ProjectionId, ProjectionObservationId, SnapshotId};

/// Derive the canonical observation id from its identity fields
/// `(projection id, value digest, snapshot id)`, domain-separated and
/// versioned. Persisted ids that do not re-derive are corrupt state.
#[must_use]
pub fn canonical_observation_id(
    projection: &ProjectionId,
    value_digest: &ContentHash,
    snapshot: &SnapshotId,
) -> ProjectionObservationId {
    let mut buf = Vec::new();
    write_str(&mut buf, "trellis.projection-observation.v1");
    write_bytes(&mut buf, projection.hash().digest());
    write_bytes(&mut buf, value_digest.digest());
    write_bytes(&mut buf, snapshot.hash().digest());
    ProjectionObservationId::from_hash(ContentHash::compute(HashAlgo::Blake3, &buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(seed: u8) -> ContentHash {
        let d: Vec<u8> = (0..32).map(|i| seed.wrapping_add(i)).collect();
        ContentHash::from_bytes(HashAlgo::Blake3, &d).unwrap()
    }

    #[test]
    fn derivation_is_deterministic_and_field_sensitive() {
        let p = ProjectionId::from_hash(h(1));
        let v = h(2);
        let s = SnapshotId::from_hash(h(3));
        assert_eq!(
            canonical_observation_id(&p, &v, &s),
            canonical_observation_id(&p, &v, &s)
        );
        assert_ne!(
            canonical_observation_id(&p, &v, &s),
            canonical_observation_id(&p, &h(4), &s)
        );
        assert_ne!(
            canonical_observation_id(&p, &v, &s),
            canonical_observation_id(&ProjectionId::from_hash(h(9)), &v, &s)
        );
        assert_ne!(
            canonical_observation_id(&p, &v, &s),
            canonical_observation_id(&p, &v, &SnapshotId::from_hash(h(5)))
        );
        assert!(canonical_observation_id(&p, &v, &s)
            .to_string()
            .starts_with("blake3:"));
    }
}

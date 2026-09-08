//! Environment fingerprints — explicitly declared inputs only (spec §49).
//!
//! The ambient machine is never hashed indiscriminately. A fingerprint
//! contains exactly the entries the caller declares; environment variables
//! are **deny-by-default** (nothing in this module reads the process
//! environment), and only digests of declared values are retained — raw
//! values are dropped after hashing so secrets cannot leak into stored
//! state.

use trellis_core::canonical::{write_bytes, write_str, write_u64};
use trellis_core::ids::{ContentHash, EnvironmentFingerprintId, HashAlgo};

/// A declared environment fingerprint (spec §49).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentFingerprint {
    id: EnvironmentFingerprintId,
    /// Declared entries sorted by key. Values are stored as digests only.
    entries: Vec<(String, ContentHash)>,
}

impl EnvironmentFingerprint {
    /// Build a fingerprint from explicitly declared `(key, value)` pairs.
    ///
    /// # Invariants (spec §19)
    /// - Only declared inputs participate; nothing ambient is consulted.
    /// - Keys must be non-empty and unique.
    /// - Values are hashed (BLAKE3) and the raw values are discarded.
    ///
    /// # Panics-free secrets policy
    /// Declared values are **hashed into the fingerprint identity**, and
    /// spec §19 forbids hashing secrets. Therefore callers must never
    /// declare secret values (credentials, tokens, private keys) here:
    /// declare only non-sensitive, allowlisted environment facts (tool
    /// versions, platform, lockfile digests). There is deliberately no
    /// mechanism to smuggle in ambient variables.
    pub fn from_declared(declared: &[(String, String)]) -> Result<Self, EnvironmentError> {
        if declared.is_empty() {
            return Err(EnvironmentError::Empty);
        }
        let mut keys: Vec<&String> = declared.iter().map(|(k, _)| k).collect();
        keys.sort();
        for pair in keys.windows(2) {
            if pair[0] == pair[1] {
                return Err(EnvironmentError::DuplicateKey(pair[0].clone()));
            }
        }
        for (k, _) in declared {
            if k.trim().is_empty() {
                return Err(EnvironmentError::EmptyKey);
            }
        }

        let mut entries: Vec<(String, ContentHash)> = declared
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    ContentHash::compute(HashAlgo::Blake3, v.as_bytes()),
                )
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut buf = Vec::new();
        write_str(&mut buf, "trellis.env.v1");
        write_u64(&mut buf, entries.len() as u64);
        for (k, digest) in &entries {
            write_str(&mut buf, k);
            write_bytes(&mut buf, digest.digest());
        }
        let id = EnvironmentFingerprintId::from_hash(ContentHash::compute(HashAlgo::Blake3, &buf));
        Ok(Self { id, entries })
    }

    /// Content identity of this fingerprint.
    #[must_use]
    pub const fn id(&self) -> EnvironmentFingerprintId {
        self.id
    }

    /// Declared entries sorted by key. Values appear **only as digests** —
    /// raw declared values are never retained (spec §49).
    #[must_use]
    pub fn entries(&self) -> &[(String, ContentHash)] {
        &self.entries
    }
}

/// Environment fingerprint construction errors.
#[derive(Debug, PartialEq, Eq)]
pub enum EnvironmentError {
    /// No entries declared — a fingerprint must assert *something*.
    Empty,
    /// The same key was declared twice.
    DuplicateKey(String),
    /// A key was empty or whitespace.
    EmptyKey,
}

impl std::fmt::Display for EnvironmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvironmentError::Empty => write!(f, "environment fingerprint has no declared entries"),
            EnvironmentError::DuplicateKey(k) => write!(f, "duplicate declared key `{k}`"),
            EnvironmentError::EmptyKey => write!(f, "declared environment key is empty"),
        }
    }
}

impl std::error::Error for EnvironmentError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_declared_inputs_yield_same_identity() {
        let a = EnvironmentFingerprint::from_declared(&[
            ("python".to_string(), "3.12.7".to_string()),
            ("platform".to_string(), "linux-amd64".to_string()),
        ])
        .unwrap();
        let b = EnvironmentFingerprint::from_declared(&[
            ("platform".to_string(), "linux-amd64".to_string()),
            ("python".to_string(), "3.12.7".to_string()),
        ])
        .unwrap();
        assert_eq!(a.id(), b.id(), "declaration order must not matter");
        assert!(a.id().to_string().starts_with("blake3:"));
    }

    #[test]
    fn different_values_yield_different_identity() {
        let a = EnvironmentFingerprint::from_declared(&[("python".into(), "3.12".into())]).unwrap();
        let b = EnvironmentFingerprint::from_declared(&[("python".into(), "3.13".into())]).unwrap();
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn raw_values_are_not_retained() {
        let secret = "super-secret-value";
        let fp = EnvironmentFingerprint::from_declared(&[("token".into(), secret.into())]).unwrap();
        let rendered = format!("{fp:?}");
        assert!(
            !rendered.contains(secret),
            "raw value leaked into stored state"
        );
        // Only digests are exposed.
        assert!(fp.entries()[0].1.to_string().starts_with("blake3:"));
    }

    #[test]
    fn duplicate_and_empty_keys_rejected() {
        assert_eq!(
            EnvironmentFingerprint::from_declared(&[
                ("a".into(), "1".into()),
                ("a".into(), "2".into()),
            ])
            .unwrap_err(),
            EnvironmentError::DuplicateKey("a".into())
        );
        assert_eq!(
            EnvironmentFingerprint::from_declared(&[("  ".into(), "1".into())]).unwrap_err(),
            EnvironmentError::EmptyKey
        );
        assert_eq!(
            EnvironmentFingerprint::from_declared(&[]).unwrap_err(),
            EnvironmentError::Empty
        );
    }
}

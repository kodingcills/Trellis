//! Content identities and typed identifiers (spec §19, §4).
//!
//! Every persistent domain object is identified by a content hash whose
//! algorithm is encoded into the ID (`blake3:…`) so future migration is
//! possible. M0 accepts digests as opaque 32-byte values; M1 wires the real
//! BLAKE3 hashing and canonical serialization behind these same types.

use std::fmt;
use std::str::FromStr;

use crate::error::DomainError;

/// Hash algorithm used for a content identity.
///
/// The algorithm is part of the identity itself (spec §19): IDs render as
/// `<algo>:<hex>` and never silently change algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HashAlgo {
    /// BLAKE3, 256-bit digest. The only algorithm in v0.1.
    Blake3,
}

impl HashAlgo {
    /// Canonical lowercase prefix used in rendered IDs.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            HashAlgo::Blake3 => "blake3",
        }
    }

    /// Digest length in bytes for this algorithm.
    #[must_use]
    pub const fn digest_len(self) -> usize {
        match self {
            HashAlgo::Blake3 => 32,
        }
    }

    /// Parse an algorithm from its canonical prefix.
    #[must_use]
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "blake3" => Some(HashAlgo::Blake3),
            _ => None,
        }
    }
}

impl fmt::Display for HashAlgo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.prefix())
    }
}

/// A content identity: algorithm tag + raw digest.
///
/// Immutable by construction; equality and ordering are over the full
/// `(algo, digest)` pair, so IDs from different algorithms never collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash {
    algo: HashAlgo,
    digest: [u8; 32],
}

impl ContentHash {
    /// Hash a canonical byte sequence with the given algorithm and wrap the
    /// digest (spec §19: identities are computed over Trellis-canonical
    /// bytes, never over third-party serializations).
    #[must_use]
    pub fn compute(algo: HashAlgo, data: &[u8]) -> Self {
        match algo {
            HashAlgo::Blake3 => {
                let digest = blake3::hash(data);
                Self {
                    algo,
                    digest: *digest.as_bytes(),
                }
            }
        }
    }

    /// Wrap a raw digest. Returns `None` if the length does not match the
    /// algorithm's digest length.
    #[must_use]
    pub fn from_bytes(algo: HashAlgo, digest: &[u8]) -> Option<Self> {
        if digest.len() != algo.digest_len() {
            return None;
        }
        let mut buf = [0u8; 32];
        buf[..digest.len()].copy_from_slice(digest);
        Some(Self { algo, digest: buf })
    }

    /// The algorithm of this content identity.
    #[must_use]
    pub const fn algo(&self) -> HashAlgo {
        self.algo
    }

    /// The raw digest bytes (length `algo().digest_len()`).
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algo.prefix(), hex_encode(&self.digest))
    }
}

impl FromStr for ContentHash {
    type Err = crate::error::DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, hex) = s.split_once(':').ok_or(DomainError::MissingHashSeparator)?;
        let algo = HashAlgo::from_prefix(prefix)
            .ok_or_else(|| crate::error::DomainError::UnknownHashAlgo(prefix.to_owned()))?;
        let digest =
            hex_decode(hex).ok_or(crate::error::DomainError::InvalidHexDigest(hex.len()))?;
        Self::from_bytes(algo, &digest).ok_or(crate::error::DomainError::DigestLength {
            algo,
            got: digest.len(),
        })
    }
}

/// Incremental content hasher over canonical bytes. Use for payloads too
/// large to buffer (files); equivalent to [`ContentHash::compute`] on the
/// concatenated bytes.
#[derive(Debug)]
pub struct ContentHasher {
    algo: HashAlgo,
    inner: blake3::Hasher,
}

impl ContentHasher {
    /// Start hashing with the given algorithm.
    #[must_use]
    pub fn new(algo: HashAlgo) -> Self {
        let inner = match algo {
            HashAlgo::Blake3 => blake3::Hasher::new(),
        };
        Self { algo, inner }
    }

    /// Feed the next chunk of canonical bytes.
    pub fn update(&mut self, data: &[u8]) -> &mut Self {
        self.inner.update(data);
        self
    }

    /// Finish and wrap the digest.
    #[must_use]
    pub fn finish(&self) -> ContentHash {
        let digest = self.inner.finalize();
        ContentHash {
            algo: self.algo,
            digest: *digest.as_bytes(),
        }
    }
}

/// A git object ID (SHA-1, 20 bytes). Distinct from [`ContentHash`]: git
/// object identity is git's, not Trellis's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitOid([u8; 20]);

impl GitOid {
    /// Wrap raw git SHA-1 bytes. Returns `None` unless exactly 20 bytes.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 20 {
            return None;
        }
        let mut buf = [0u8; 20];
        buf.copy_from_slice(bytes);
        Some(Self(buf))
    }

    /// Raw SHA-1 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl fmt::Display for GitOid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex_encode(&self.0))
    }
}

impl FromStr for GitOid {
    type Err = crate::error::DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex_decode(s).ok_or(crate::error::DomainError::InvalidHexDigest(s.len()))?;
        Self::from_bytes(&bytes).ok_or(crate::error::DomainError::GitOidLength(bytes.len()))
    }
}

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(ContentHash);

        impl $name {
            /// Wrap an existing content hash into this typed ID.
            #[must_use]
            pub const fn from_hash(hash: ContentHash) -> Self {
                Self(hash)
            }

            /// The underlying content identity.
            #[must_use]
            pub const fn hash(&self) -> &ContentHash {
                &self.0
            }
        }

        impl From<ContentHash> for $name {
            fn from(hash: ContentHash) -> Self {
                Self(hash)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = crate::error::DomainError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                ContentHash::from_str(s).map(Self)
            }
        }
    };
}

typed_id!(
    /// Identity of an immutable artifact value (spec §10).
    ArtifactId
);
typed_id!(
    /// Identity of a derivation — how an artifact was produced (spec §16).
    DerivationId
);
typed_id!(
    /// Identity of a single validation attestation (spec §12).
    AttestationId
);
typed_id!(
    /// Identity of a snapshot — a coherent frozen world state (spec §6).
    SnapshotId
);
typed_id!(
    /// Identity of a working-tree manifest backing a snapshot (spec §6).
    ManifestId
);
typed_id!(
    /// Identity of an authoritative semantic snapshot produced by a freeze
    /// (spec §9.1).
    SemanticSnapshotId
);
typed_id!(
    /// Identity of a repository within the local store.
    RepositoryId
);
typed_id!(
    /// Identity of an explicitly declared environment fingerprint (spec §49).
    EnvironmentFingerprintId
);
typed_id!(
    /// Identity of an immutable CAS blob (spec §19).
    BlobId
);
typed_id!(
    /// Identity of a dependency projection (spec §7).
    ProjectionId
);
typed_id!(
    /// Identity of one projection observation at one snapshot (spec §7).
    ProjectionObservationId
);
typed_id!(
    /// Identity of a verifier — the implementation of a validity contract
    /// (spec §16).
    VerifierId
);

/// Unix epoch milliseconds. All timestamps in the domain model are UTC.
pub type Timestamp = u64;

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn digest(seed: u8) -> Vec<u8> {
        (0..32).map(|i| seed.wrapping_add(i)).collect()
    }

    #[test]
    fn content_hash_display_parse_roundtrip() {
        let hash = ContentHash::from_bytes(HashAlgo::Blake3, &digest(0x2a)).unwrap();
        let rendered = hash.to_string();
        assert!(rendered.starts_with("blake3:"));
        assert_eq!(rendered.len(), "blake3:".len() + 64);
        let parsed = ContentHash::from_str(&rendered).unwrap();
        assert_eq!(parsed, hash);
    }

    #[test]
    fn uppercase_hex_is_accepted_and_rendered_lowercase() {
        let hash = ContentHash::from_bytes(HashAlgo::Blake3, &digest(1)).unwrap();
        let upper = format!("blake3:{}", hex_encode(&digest(1)).to_uppercase());
        let parsed = ContentHash::from_str(&upper).unwrap();
        assert_eq!(parsed.to_string(), hash.to_string());
    }

    #[test]
    fn rejects_bad_ids() {
        assert!(ContentHash::from_str("no-separator").is_err());
        assert!(ContentHash::from_str("sha256:00").is_err());
        assert!(ContentHash::from_str("blake3:zz").is_err());
        assert!(ContentHash::from_str("blake3:00").is_err()); // wrong length
        assert!(ContentHash::from_str("blake3:0").is_err()); // odd length
    }

    #[test]
    fn typed_ids_do_not_unify() {
        let hash = ContentHash::from_bytes(HashAlgo::Blake3, &digest(7)).unwrap();
        let artifact = ArtifactId::from_hash(hash);
        let snapshot = SnapshotId::from_hash(hash);
        // Same bytes, distinct types: type-level separation is the point.
        assert_eq!(artifact.hash(), snapshot.hash());
        // Both round-trip through their own type.
        assert_eq!(
            ArtifactId::from_str(&artifact.to_string()).unwrap(),
            artifact
        );
        assert_eq!(
            SnapshotId::from_str(&snapshot.to_string()).unwrap(),
            snapshot
        );
    }

    #[test]
    fn digest_length_mismatch_rejected() {
        let short = vec![0u8; 16];
        assert!(ContentHash::from_bytes(HashAlgo::Blake3, &short).is_none());
    }

    #[test]
    fn compute_is_deterministic_and_input_sensitive() {
        let a = ContentHash::compute(HashAlgo::Blake3, b"canonical bytes");
        let b = ContentHash::compute(HashAlgo::Blake3, b"canonical bytes");
        let c = ContentHash::compute(HashAlgo::Blake3, b"canonical bytes ");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.algo(), HashAlgo::Blake3);
    }

    #[test]
    fn streaming_hasher_matches_one_shot() {
        let algo = HashAlgo::Blake3;
        let mut hasher = ContentHasher::new(algo);
        hasher.update(b"chunk one");
        hasher.update(b"chunk two");
        let streamed = hasher.finish();
        let one_shot = ContentHash::compute(algo, b"chunk onechunk two");
        assert_eq!(streamed, one_shot);
    }

    #[test]
    fn known_blake3_vector() {
        // BLAKE3 of empty input, official test vector (first 8 hex chars
        // 0af15ee2 across the full digest af1349b9...).
        let empty = ContentHash::compute(HashAlgo::Blake3, b"");
        let rendered = empty.to_string();
        assert!(
            rendered.starts_with("blake3:af1349b9"),
            "unexpected blake3(empty) digest: {rendered}"
        );
    }

    #[test]
    fn git_oid_roundtrip() {
        let bytes: Vec<u8> = (0..20).collect();
        let oid = GitOid::from_bytes(&bytes).unwrap();
        assert_eq!(oid.to_string().len(), 40);
        assert_eq!(GitOid::from_str(&oid.to_string()).unwrap(), oid);
        assert!(GitOid::from_bytes(&bytes[..19]).is_none());
        assert!(GitOid::from_str("zz").is_err());
    }
}

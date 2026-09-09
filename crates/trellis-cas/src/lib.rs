//! trellis-cas — local content-addressed store (spec §19).
//!
//! Write protocol: canonical bytes → BLAKE3 → temp file → fsync → atomic
//! rename into the CAS. Metadata commits happen in `trellis-store`, never
//! before the blob is durably placed. Reads verify content against the
//! requested hash — corruption is always an explicit error, never silent
//! (spec §2.1).

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use trellis_core::ids::{BlobId, ContentHash, HashAlgo};

/// CAS identity for canonical bytes: the `blake3:…` BlobId.
#[must_use]
pub fn blob_id_of(bytes: &[u8]) -> BlobId {
    BlobId::from_hash(ContentHash::compute(HashAlgo::Blake3, bytes))
}

/// Errors surfaced at the CAS boundary. All explicit; corruption is never
/// silently repaired or ignored (spec §2.1, §19).
#[derive(Debug)]
pub enum CasError {
    /// Underlying filesystem error.
    Io(PathBuf, io::Error),
    /// The blob's content does not hash to its requested identity.
    CorruptBlob(BlobId),
    /// The blob is not present in the store.
    MissingBlob(BlobId),
    /// The store root is not a directory (or cannot be created).
    RootInvalid(PathBuf, io::Error),
}

impl std::fmt::Display for CasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CasError::Io(p, e) => write!(f, "cas io error at {}: {e}", p.display()),
            CasError::CorruptBlob(id) => write!(f, "corrupt blob {id}"),
            CasError::MissingBlob(id) => write!(f, "missing blob {id}"),
            CasError::RootInvalid(p, e) => write!(f, "invalid CAS root {}: {e}", p.display()),
        }
    }
}

impl std::error::Error for CasError {}

/// Local filesystem content-addressed store rooted at a directory
/// (typically `.trellis/cas`). Layout: `<root>/<digest[..2]>/<digest>`.
/// Blobs are immutable after publication; temp files under `<root>/tmp`
/// are never exposed as committed objects.
pub struct FsCas {
    root: PathBuf,
}

impl FsCas {
    /// Open (or initialize) a CAS rooted at `root`.
    ///
    /// # Errors
    /// [`CasError::RootInvalid`] when the root cannot be created/read.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, CasError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| CasError::RootInvalid(root.clone(), e))?;
        fs::create_dir_all(root.join("tmp")).map_err(|e| CasError::RootInvalid(root.clone(), e))?;
        Ok(Self { root })
    }

    fn blob_path(&self, id: &BlobId) -> PathBuf {
        let hex = id.hash().to_string();
        let digest_hex = hex.strip_prefix("blake3:").unwrap_or(&hex);
        self.root.join(&digest_hex[..2]).join(digest_hex)
    }

    /// Temporary-file location for a put (never exposed as committed).
    fn tmp_path_for(&self, id: &BlobId) -> PathBuf {
        let hex = id.hash().to_string();
        let short = hex.strip_prefix("blake3:").unwrap_or(&hex)[..16].to_string();
        self.root
            .join("tmp")
            .join(format!("put-{}-{short}", std::process::id()))
    }

    /// Write canonical bytes into the CAS. Idempotent: same bytes → same
    /// BlobId, payload stored exactly once. Protocol: temp file → fsync →
    /// atomic rename into the committed path.
    ///
    /// # Errors
    /// [`CasError::Io`] on filesystem failures.
    pub fn put(&self, bytes: &[u8]) -> Result<BlobId, CasError> {
        let id = blob_id_of(bytes);
        let final_path = self.blob_path(&id);
        if final_path.exists() {
            return Ok(id); // idempotent: content already durable
        }
        let tmp_path = self.tmp_path_for(&id);
        let mut file =
            fs::File::create(&tmp_path).map_err(|e| CasError::Io(tmp_path.clone(), e))?;
        file.write_all(bytes)
            .map_err(|e| CasError::Io(tmp_path.clone(), e))?;
        file.sync_all()
            .map_err(|e| CasError::Io(tmp_path.clone(), e))?;
        drop(file);
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).map_err(|e| CasError::Io(parent.to_path_buf(), e))?;
        }
        fs::rename(&tmp_path, &final_path).map_err(|e| CasError::Io(final_path.clone(), e))?;
        Ok(id)
    }

    /// Fetch a blob's exact bytes, verifying content against the requested
    /// identity (corruption is an explicit error).
    ///
    /// # Errors
    /// [`CasError::MissingBlob`], [`CasError::CorruptBlob`], [`CasError::Io`].
    pub fn get(&self, id: &BlobId) -> Result<Vec<u8>, CasError> {
        let path = self.blob_path(id);
        let bytes = fs::read(&path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => CasError::MissingBlob(*id),
            _ => CasError::Io(path.clone(), e),
        })?;
        if blob_id_of(&bytes) != *id {
            return Err(CasError::CorruptBlob(*id));
        }
        Ok(bytes)
    }

    /// Whether the blob exists (no content verification).
    #[must_use]
    pub fn contains(&self, id: &BlobId) -> bool {
        self.blob_path(id).exists()
    }

    /// The store root directory.
    #[must_use]
    pub fn root_path(&self) -> &Path {
        &self.root
    }

    /// Verify content of an existing blob against its identity.
    ///
    /// # Errors
    /// [`CasError::MissingBlob`] if absent, [`CasError::CorruptBlob`] if
    /// content does not hash to the id.
    pub fn verify(&self, id: &BlobId) -> Result<(), CasError> {
        self.get(id).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn same_bytes_same_id_different_bytes_different_id() {
        let a = blob_id_of(b"alpha");
        let a2 = blob_id_of(b"alpha");
        let b = blob_id_of(b"beta");
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert!(a.to_string().starts_with("blake3:"));
    }

    #[test]
    fn put_is_idempotent_and_roundtrips_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cas = FsCas::open(dir.path()).unwrap();
        let bytes = b"canonical payload bytes";
        let id1 = cas.put(bytes).unwrap();
        let id2 = cas.put(bytes).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(cas.get(&id1).unwrap(), bytes);
        // Exactly one committed object (tmp empty).
        assert_eq!(count_files(dir.path()), 1);
    }

    fn count_files(root: &Path) -> usize {
        let mut n = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                } else {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn corrupted_blob_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let cas = FsCas::open(dir.path()).unwrap();
        let id = cas.put(b"integrity").unwrap();
        let path = cas.blob_path(&id);
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
        match cas.get(&id) {
            Err(CasError::CorruptBlob(_)) => {}
            other => panic!("expected CorruptBlob, got {other:?}"),
        }
    }

    #[test]
    fn missing_blob_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let cas = FsCas::open(dir.path()).unwrap();
        let id = blob_id_of(b"never-stored");
        assert!(!cas.contains(&id));
        assert!(matches!(cas.get(&id), Err(CasError::MissingBlob(_))));
    }

    #[test]
    fn temp_writes_never_exposed_as_committed() {
        let dir = tempfile::tempdir().unwrap();
        let cas = FsCas::open(dir.path()).unwrap();
        // Simulate a crashed put: bytes in tmp only.
        let id = blob_id_of(b"orphan");
        let tmp = cas.tmp_path_for(&id);
        fs::create_dir_all(tmp.parent().unwrap()).unwrap();
        fs::write(tmp, b"orphan").unwrap();
        assert!(
            !cas.contains(&id),
            "tmp content must not be a committed blob"
        );
        assert!(matches!(cas.get(&id), Err(CasError::MissingBlob(_))));
    }

    #[test]
    fn different_bytes_never_share_an_id() {
        let dir = tempfile::tempdir().unwrap();
        let cas = FsCas::open(dir.path()).unwrap();
        let a = cas.put(b"alpha").unwrap();
        let b = cas.put(b"beta").unwrap();
        assert_ne!(a, b);
        assert_ne!(cas.get(&a).unwrap(), cas.get(&b).unwrap());
    }
}

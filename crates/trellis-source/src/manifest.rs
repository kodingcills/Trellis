//! Working-tree manifests — one version of the tracked file state (spec §6).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use trellis_core::canonical::write_str;
use trellis_core::ids::{ContentHash, ContentHasher, HashAlgo, ManifestId};

/// Directory names excluded from manifest walks, at any depth. These are
/// build/VCS/control-plane artifacts, never repository source. Overridable
/// via [`ManifestOptions`].
pub const DEFAULT_EXCLUDE_DIRS: &[&str] = &[".git", ".agent", ".trellis", "target", "node_modules"];

/// Options for building a working-tree manifest.
#[derive(Debug, Clone)]
pub struct ManifestOptions {
    /// Repository root to walk.
    pub root: PathBuf,
    /// Directory names to exclude at any depth (defaults to
    /// [`DEFAULT_EXCLUDE_DIRS`]).
    pub exclude_dirs: Vec<String>,
}

impl ManifestOptions {
    /// Options for `root` with the default exclusion set.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            exclude_dirs: DEFAULT_EXCLUDE_DIRS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

/// One manifest entry: a normalized relative path and its content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Path relative to the repository root, `/`-separated, canonical form.
    pub path: String,
    /// BLAKE3 digest of the file content.
    pub digest: ContentHash,
}

/// The tracked file state of one snapshot: entries sorted by path, bound to
/// a content identity over the canonical encoding (spec §6, §19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    id: ManifestId,
    entries: Vec<ManifestEntry>,
}

impl Manifest {
    /// Build a manifest from pre-hashed entries. Entries are sorted by path;
    /// duplicates of the same path with the **same** digest collapse, while
    /// conflicting digests for one path are a hard error. The manifest
    /// identity is computed over the canonical encoding. Paths must be
    /// relative and normalized (see [`normalize_rel_path`]).
    pub fn from_entries(mut entries: Vec<ManifestEntry>) -> Result<Self, ManifestError> {
        for entry in &entries {
            if normalize_rel_path(Path::new(&entry.path)).as_deref() != Some(entry.path.as_str()) {
                return Err(ManifestError::NonCanonicalPath(entry.path.clone()));
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        for pair in entries.windows(2) {
            if pair[0].path == pair[1].path && pair[0].digest != pair[1].digest {
                return Err(ManifestError::ConflictingDigest(pair[0].path.clone()));
            }
        }
        entries.dedup_by(|a, b| a.path == b.path);

        let mut buf = Vec::new();
        write_str(&mut buf, "trellis.manifest.v1");
        trellis_core::canonical::write_u64(&mut buf, entries.len() as u64);
        for entry in &entries {
            write_str(&mut buf, &entry.path);
            trellis_core::canonical::write_bytes(&mut buf, entry.digest.digest());
        }
        let id = ManifestId::from_hash(ContentHash::compute(HashAlgo::Blake3, &buf));
        Ok(Self { id, entries })
    }

    /// Content identity of this manifest.
    #[must_use]
    pub const fn id(&self) -> ManifestId {
        self.id
    }

    /// Entries, sorted by path.
    #[must_use]
    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }
}

/// Manifest construction errors.
#[derive(Debug)]
pub enum ManifestError {
    /// An entry path was not in canonical relative form.
    NonCanonicalPath(String),
    /// The same path appeared with two different content digests. This is a
    /// provenance conflict that must never be silently resolved (spec §2.1:
    /// correctness dominates reuse; a silent drop could hide a change).
    ConflictingDigest(String),
    /// The repository root could not be read.
    RootUnreadable(PathBuf, io::Error),
    /// A file could not be read while hashing.
    FileUnreadable(PathBuf, io::Error),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::NonCanonicalPath(p) => {
                write!(f, "manifest path not canonical: {p}")
            }
            ManifestError::ConflictingDigest(p) => {
                write!(
                    f,
                    "conflicting digests for path `{p}` — resolve instead of deduplicating"
                )
            }
            ManifestError::RootUnreadable(p, e) => {
                write!(f, "cannot read root {}: {e}", p.display())
            }
            ManifestError::FileUnreadable(p, e) => write!(f, "cannot read {}: {e}", p.display()),
        }
    }
}

impl std::error::Error for ManifestError {}

/// Normalize a relative path to canonical manifest form: `/`-separated,
/// no leading `./`, no empty or `.` components. Returns `None` for
/// absolute paths, `..` components, **non-UTF-8 components**, or paths
/// that normalize to nothing. Non-UTF-8 names are rejected (not lossily
/// coerced) so distinct files can never collapse to one canonical path
/// (spec §2.1) (spec §19: normalized paths).
#[must_use]
pub fn normalize_rel_path(path: &Path) -> Option<String> {
    if path.is_absolute() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_str()?),
            std::path::Component::CurDir => continue,
            _ => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Walk the working tree and hash every eligible file (BLAKE3, streaming).
///
/// Determinism: directories are traversed and files are hashed in path
/// order; symlinks are skipped entirely (they are neither tracked content
/// nor safe from cycles in v0.1 — recorded in DECISIONS). Excluded
/// directory names prune the walk at any depth.
pub fn build_manifest(options: &ManifestOptions) -> Result<Manifest, ManifestError> {
    if !options.root.is_dir() {
        return Err(ManifestError::RootUnreadable(
            options.root.clone(),
            io::Error::new(io::ErrorKind::NotFound, "not a directory"),
        ));
    }
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(&options.root, &options.exclude_dirs, &mut files)
        .map_err(|(path, e)| ManifestError::FileUnreadable(path, e))?;

    let mut entries = Vec::with_capacity(files.len());
    for file in &files {
        let rel = normalize_rel_path(file.strip_prefix(&options.root).unwrap_or(file))
            .ok_or_else(|| ManifestError::NonCanonicalPath(file.display().to_string()))?;
        let digest = hash_file(file).map_err(|e| ManifestError::FileUnreadable(file.clone(), e))?;
        entries.push(ManifestEntry { path: rel, digest });
    }
    Manifest::from_entries(entries)
}

fn collect_files(
    dir: &Path,
    excludes: &[String],
    out: &mut Vec<PathBuf>,
) -> Result<(), (PathBuf, io::Error)> {
    let read = fs::read_dir(dir).map_err(|e| (dir.to_path_buf(), e))?;
    for entry in read {
        let entry = entry.map_err(|e| (dir.to_path_buf(), e))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = entry.file_type().map_err(|e| (path.clone(), e))?;
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if excludes.iter().any(|e| e == &name) {
                continue;
            }
            collect_files(&path, excludes, out)?;
        } else if meta.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> io::Result<ContentHash> {
    let mut hasher = ContentHasher::new(HashAlgo::Blake3);
    let mut file = fs::File::open(path)?;
    io::copy(&mut file, &mut hasher_as_writer(&mut hasher))?;
    Ok(hasher.finish())
}

/// Adapt [`ContentHasher::update`] to `io::Write` for [`io::copy`].
struct HasherWriter<'a>(&'a mut ContentHasher);

impl std::io::Write for HasherWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn hasher_as_writer(h: &mut ContentHasher) -> HasherWriter<'_> {
    HasherWriter(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(root: &Path, rel: &str, content: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn manifest_is_sorted_and_stable_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "b.py", b"beta");
        write_file(dir.path(), "src/a.py", b"alpha");
        write_file(dir.path(), "a.txt", b"aaa");

        let m1 = build_manifest(&ManifestOptions::new(dir.path())).unwrap();
        let m2 = build_manifest(&ManifestOptions::new(dir.path())).unwrap();

        let paths: Vec<&str> = m1.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "b.py", "src/a.py"]);
        assert_eq!(m1.id(), m2.id(), "same content must yield same manifest id");
        assert!(m1.id().to_string().starts_with("blake3:"));
    }

    #[test]
    fn content_change_changes_entry_digest_but_identical_content_collapses() {
        let d1 = tempfile::tempdir().unwrap();
        write_file(d1.path(), "x.py", b"v1");
        let m1 = build_manifest(&ManifestOptions::new(d1.path())).unwrap();

        let d2 = tempfile::tempdir().unwrap();
        write_file(d2.path(), "x.py", b"v2");
        let m2 = build_manifest(&ManifestOptions::new(d2.path())).unwrap();

        let d3 = tempfile::tempdir().unwrap();
        write_file(d3.path(), "x.py", b"v2");
        let m3 = build_manifest(&ManifestOptions::new(d3.path())).unwrap();

        assert_ne!(m1.entries()[0].digest, m2.entries()[0].digest);
        assert_ne!(m1.id(), m2.id());
        assert_eq!(m2.id(), m3.id());
    }

    #[test]
    fn default_excludes_prune_the_walk() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "src/real.py", b"real");
        write_file(dir.path(), "target/artifact.bin", b"junk");
        write_file(dir.path(), ".git/HEAD", b"ref: refs/heads/main");
        write_file(dir.path(), "nested/target/junk2.bin", b"junk2");

        let m = build_manifest(&ManifestOptions::new(dir.path())).unwrap();
        let paths: Vec<&str> = m.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["src/real.py"]);
    }

    #[test]
    fn symlinks_are_skipped() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            write_file(dir.path(), "real.txt", b"real");
            std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("link.txt"))
                .unwrap();
            let m = build_manifest(&ManifestOptions::new(dir.path())).unwrap();
            let paths: Vec<&str> = m.entries().iter().map(|e| e.path.as_str()).collect();
            assert_eq!(paths, vec!["real.txt"]);
        }
    }

    #[test]
    fn non_canonical_paths_rejected() {
        assert_eq!(
            normalize_rel_path(Path::new("/abs/path")),
            None,
            "absolute paths are not canonical"
        );
        assert_eq!(
            normalize_rel_path(Path::new("./a/b.py")).as_deref(),
            Some("a/b.py")
        );
        assert_eq!(
            normalize_rel_path(Path::new("a/b.py")).as_deref(),
            Some("a/b.py")
        );
        assert_eq!(normalize_rel_path(Path::new("a/../b.py")), None);
    }

    fn entry_of(path: &str, content: &[u8]) -> ManifestEntry {
        ManifestEntry {
            path: path.to_string(),
            digest: ContentHash::compute(HashAlgo::Blake3, content),
        }
    }

    #[test]
    fn conflicting_digests_for_one_path_rejected() {
        let err = Manifest::from_entries(vec![entry_of("a.py", b"one"), entry_of("a.py", b"two")])
            .unwrap_err();
        assert!(
            matches!(err, ManifestError::ConflictingDigest(ref p) if p == "a.py"),
            "conflicting digests must never be silently deduplicated"
        );
        // Identical duplicates collapse safely.
        let ok = Manifest::from_entries(vec![entry_of("a.py", b"one"), entry_of("a.py", b"one")])
            .unwrap();
        assert_eq!(ok.entries().len(), 1);
    }

    #[test]
    fn non_utf8_paths_rejected_not_lossily_coerced() {
        use std::ffi::OsStr;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bad = Path::new(OsStr::from_bytes(b"bad\xff name.py"));
            assert_eq!(normalize_rel_path(bad), None);
        }
        // The contract: non-UTF-8 components normalize to None, which the
        // walker converts into a loud NonCanonicalPath error — distinct
        // files can never collapse to one canonical path. (macOS APFS
        // refuses to create such files at the OS layer, so the full-walk
        // variant is exercised on filesystems that permit them.)
    }

    #[cfg(unix)]
    #[test]
    fn tracked_file_swapped_to_symlink_signals_removed() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "real.txt", b"real");
        let stored = build_manifest(&ManifestOptions::new(dir.path())).unwrap();

        fs::remove_file(dir.path().join("real.txt")).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", dir.path().join("real.txt")).unwrap();

        let changed = crate::reconcile::reconcile_against_working_tree(
            &stored,
            &ManifestOptions::new(dir.path()),
        )
        .unwrap();
        // Symlinks are excluded from manifests (DECISIONS: symlink policy);
        // the swap must therefore surface as a removal, never silence.
        assert_eq!(changed.removed, vec!["real.txt"]);
        assert!(changed.modified.is_empty());
    }
}

//! Lazy reconciliation — dirtiness discovered at query/freeze time (spec §5).
//!
//! There is no watcher and no daemon. A stored manifest represents the
//! world as of a snapshot; reconciliation compares it against the current
//! working tree and produces the **exact** changed-file set. Deterministic
//! and side-effect free: identical inputs always produce identical output.

use std::collections::BTreeMap;

use crate::manifest::{build_manifest, Manifest, ManifestOptions};

/// The exact changed-file set between a stored manifest and the current
/// tree. All vectors are sorted by path. A rename appears as
/// removal + addition (spec §33 M1: add/modify/delete).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedSet {
    /// Paths present now but absent from the stored manifest.
    pub added: Vec<String>,
    /// Paths present in both with different content digests.
    pub modified: Vec<String>,
    /// Paths present in the stored manifest but absent now.
    pub removed: Vec<String>,
}

impl ChangedSet {
    /// Whether the working tree is unchanged relative to the manifest.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.removed.is_empty()
    }

    /// Total number of changed paths.
    #[must_use]
    pub fn len(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }
}

/// Diff two manifests. Pure: deterministic, side-effect free, and exact —
/// identical trees reconcile to an empty changed set (M1 acceptance).
#[must_use]
pub fn reconcile(stored: &Manifest, current: &Manifest) -> ChangedSet {
    let old: BTreeMap<&str, _> = stored
        .entries()
        .iter()
        .map(|e| (e.path.as_str(), &e.digest))
        .collect();
    let new: BTreeMap<&str, _> = current
        .entries()
        .iter()
        .map(|e| (e.path.as_str(), &e.digest))
        .collect();

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut removed = Vec::new();

    for (path, digest) in &new {
        match old.get(path) {
            None => added.push((*path).to_string()),
            Some(old_digest) => {
                if old_digest != digest {
                    modified.push((*path).to_string());
                }
            }
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            removed.push((*path).to_string());
        }
    }

    ChangedSet {
        added,
        modified,
        removed,
    }
}

/// Reconcile a stored manifest against the working tree at `options.root`
/// right now: builds the current manifest (lazy, at call time) and diffs.
pub fn reconcile_against_working_tree(
    stored: &Manifest,
    options: &ManifestOptions,
) -> Result<ChangedSet, crate::manifest::ManifestError> {
    let current = build_manifest(options)?;
    Ok(reconcile(stored, &current))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ManifestEntry;
    use std::fs;

    fn entry(path: &str, seed: u8) -> ManifestEntry {
        ManifestEntry {
            path: path.to_string(),
            digest: ContentHash::compute(HashAlgo::Blake3, &[seed; 32]),
        }
    }

    use trellis_core::ids::{ContentHash, HashAlgo};

    #[test]
    fn identical_manifests_reconcile_to_empty() {
        let m = Manifest::from_entries(vec![entry("a.py", 1), entry("src/b.py", 2)]).unwrap();
        let changed = reconcile(&m, &m);
        assert!(changed.is_empty());
        assert_eq!(changed.len(), 0);
    }

    #[test]
    fn detects_add_modify_remove() {
        let stored = Manifest::from_entries(vec![
            entry("a.py", 1),
            entry("src/b.py", 2),
            entry("old.py", 3),
        ])
        .unwrap();
        let current = Manifest::from_entries(vec![
            entry("a.py", 1),     // unchanged
            entry("src/b.py", 9), // modified
            entry("new.py", 4),   // added
        ])
        .unwrap();

        let changed = reconcile(&stored, &current);
        assert_eq!(changed.added, vec!["new.py"]);
        assert_eq!(changed.modified, vec!["src/b.py"]);
        assert_eq!(changed.removed, vec!["old.py"]);
    }

    #[test]
    fn reconcile_against_tree_is_lazy_and_exact() {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, data: &[u8]| {
            let p = dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, data).unwrap();
        };
        write("a.py", b"one");
        write("b.py", b"two");
        let stored = build_manifest(&ManifestOptions::new(dir.path())).unwrap();

        // Mutate: modify a.py, add c.py, delete b.py.
        write("a.py", b"one-changed");
        write("c.py", b"three");
        fs::remove_file(dir.path().join("b.py")).unwrap();

        let changed =
            reconcile_against_working_tree(&stored, &ManifestOptions::new(dir.path())).unwrap();
        assert_eq!(changed.added, vec!["c.py"]);
        assert_eq!(changed.modified, vec!["a.py"]);
        assert_eq!(changed.removed, vec!["b.py"]);

        // Determinism: a second reconciliation yields the identical result.
        let again =
            reconcile_against_working_tree(&stored, &ManifestOptions::new(dir.path())).unwrap();
        assert_eq!(changed, again);
    }

    #[test]
    fn two_independently_built_identical_trees_reconcile_to_empty() {
        let d1 = tempfile::tempdir().unwrap();
        let d2 = tempfile::tempdir().unwrap();
        for dir in [d1.path(), d2.path()] {
            fs::create_dir_all(dir.join("src")).unwrap();
            fs::write(dir.join("a.py"), b"same").unwrap();
            fs::write(dir.join("src/b.py"), b"same too").unwrap();
        }
        let m1 = build_manifest(&ManifestOptions::new(d1.path())).unwrap();
        let m2 = build_manifest(&ManifestOptions::new(d2.path())).unwrap();
        assert_ne!(d1.path(), d2.path(), "different roots, identical content");
        let changed = reconcile(&m1, &m2);
        assert!(changed.is_empty());
        assert_eq!(m1.id(), m2.id());
    }

    #[test]
    fn rename_manifests_as_delete_plus_add() {
        let stored = Manifest::from_entries(vec![entry("old_name.py", 1)]).unwrap();
        let current = Manifest::from_entries(vec![entry("new_name.py", 1)]).unwrap();
        let changed = reconcile(&stored, &current);
        assert_eq!(changed.removed, vec!["old_name.py"]);
        assert_eq!(changed.added, vec!["new_name.py"]);
        assert!(changed.modified.is_empty());
    }
}

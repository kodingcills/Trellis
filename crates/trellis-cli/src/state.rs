//! CLI-local state: tree loading, snapshot/manifest identity, event log,
//! and a sidecar registry (`cli_state.json`) mapping artifact keys to
//! store ids. The sidecar is CLI bookkeeping only — all domain state
//! (artifacts, attestations, observations, snapshots) stays in the
//! trellis-store database.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trellis_core::ids::{ContentHash, HashAlgo, SnapshotId, Timestamp};

pub type CliResult<T> = Result<T, String>;

pub fn digest_str(s: &str) -> ContentHash {
    ContentHash::compute(HashAlgo::Blake3, s.as_bytes())
}

pub fn now_ms() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as Timestamp)
        .unwrap_or(0)
}

/// All tracked .py files under the repo root, relative + `/`-separated.
pub fn read_tree(repo: &Path) -> CliResult<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) -> CliResult<()> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if path.is_dir() {
                if name == "__pycache__" || name.starts_with('.') {
                    continue;
                }
                walk(&path, &rel, out)?;
            } else if name.ends_with(".py") {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                out.insert(rel, content);
            }
        }
        Ok(())
    }
    walk(repo, "", &mut out)?;
    Ok(out)
}

/// Canonical module name for a relative .py path (same rule as the
/// engine harness: drop .py, drop trailing /__init__, join with dots).
pub fn module_name(path: &str) -> String {
    let stem = path.strip_suffix(".py").unwrap_or(path);
    let stem = stem.strip_suffix("/__init__").unwrap_or(stem);
    stem.replace('/', ".")
}

pub fn module_path(module: &str) -> String {
    format!("{}.py", module.replace('.', "/"))
}

/// Snapshot identity over the canonical tree encoding (path\0content\n).
pub fn snapshot_id_for(tree: &BTreeMap<String, String>) -> SnapshotId {
    let mut canonical = String::new();
    for (path, content) in tree {
        canonical.push_str(path);
        canonical.push('\0');
        canonical.push_str(content);
        canonical.push('\n');
    }
    SnapshotId::from_hash(digest_str(&canonical))
}

/// Sidecar registry next to the store database.
#[derive(Default, Clone)]
pub struct CliState {
    pub artifacts: BTreeMap<String, ArtifactEntry>,
    pub manifest_entries: Vec<(String, String)>,
    pub head: Option<String>,
}

#[derive(Clone)]
pub struct ArtifactEntry {
    pub id: String,
    pub kind: String,
}

impl CliState {
    pub fn load(store_path: &Path) -> CliState {
        let path = sidecar_path(store_path);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(value) => {
                    let mut artifacts = BTreeMap::new();
                    if let Some(map) = value["artifacts"].as_object() {
                        for (k, v) in map {
                            artifacts.insert(
                                k.clone(),
                                ArtifactEntry {
                                    id: v["id"].as_str().unwrap_or_default().to_string(),
                                    kind: v["kind"].as_str().unwrap_or_default().to_string(),
                                },
                            );
                        }
                    }
                    let manifest_entries = value["manifest"]["entries"]
                        .as_array()
                        .map(|entries| {
                            entries
                                .iter()
                                .filter_map(|e| {
                                    Some((
                                        e["path"].as_str()?.to_string(),
                                        e["digest"].as_str()?.to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let head = value["head"].as_str().map(str::to_string);
                    CliState {
                        artifacts,
                        manifest_entries,
                        head,
                    }
                }
                Err(_) => CliState::default(),
            },
            Err(_) => CliState::default(),
        }
    }

    pub fn save(&self, store_path: &Path) -> CliResult<()> {
        let path = sidecar_path(store_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let artifacts: serde_json::Map<String, serde_json::Value> = self
            .artifacts
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::json!({"id": v.id, "kind": v.kind})))
            .collect();
        let value = serde_json::json!({
            "head": self.head,
            "artifacts": artifacts,
            "manifest": { "entries": self.manifest_entries.iter()
                .map(|(p, d)| serde_json::json!({"path": p, "digest": d}))
                .collect::<Vec<_>>() },
        });
        std::fs::write(&path, value.to_string()).map_err(|e| e.to_string())
    }
}

pub fn sidecar_path(store_path: &Path) -> PathBuf {
    store_path.with_file_name("cli_state.json")
}

pub fn events_path(store_path: &Path) -> PathBuf {
    store_path.with_file_name("events.jsonl")
}

/// Append one metrics event line to `<store-dir>/events.jsonl`.
pub fn emit_event(
    store_path: &Path,
    op: &str,
    key: Option<&str>,
    subject: Option<&str>,
    backend_or_verdict: &str,
    elapsed_us: u64,
) {
    emit_event_at(
        &events_path(store_path),
        op,
        key,
        subject,
        backend_or_verdict,
        elapsed_us,
    );
}

/// Append one metrics event line to an arbitrary JSONL path.
pub fn emit_event_at(
    path: &Path,
    op: &str,
    key: Option<&str>,
    subject: Option<&str>,
    backend_or_verdict: &str,
    elapsed_us: u64,
) {
    let line = serde_json::json!({
        "ts_ms": now_ms(),
        "op": op,
        "key": key,
        "subject": subject,
        "tag": backend_or_verdict,
        "elapsed_us": elapsed_us,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

//! trellis-cli integration tests: publish/retrieve round-trip, stale
//! rejection after mutation, post-settle pin guard, query freshness,
//! events ledger. Each test drives the CLI over a tempdir tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_bin() -> PathBuf {
    // current_exe = <target>/debug/deps/cli-<hash>; the deps parent is
    // the debug dir that also holds the built `trellis` binary.
    let exe = std::env::current_exe().expect("test exe");
    let deps = exe.parent().expect("deps dir");
    let debug = deps.parent().expect("debug dir");
    debug.join("trellis")
}

struct World {
    dir: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("trellis-cli-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo").join("auth")).expect("auth dir");
        std::fs::write(
            dir.join("repo").join("auth").join("tokens.py"),
            "def refresh_token(user_id: str) -> str:\n    return user_id\n",
        )
        .expect("tokens.py");
        std::fs::write(
            dir.join("repo").join("auth").join("service.py"),
            "from auth.tokens import refresh_token\n\ndef login(user_id: str) -> str:\n    return refresh_token(user_id)\n",
        )
        .expect("service.py");
        std::fs::write(
            dir.join("repo").join("api.py"),
            "from auth.tokens import refresh_token as rt\n\ndef handle(user_id: str) -> str:\n    return rt(user_id)\n",
        )
        .expect("api.py");
        World { dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.join("repo")
    }

    fn store(&self) -> PathBuf {
        self.dir.join("store").join("metadata.db")
    }

    fn run(&self, args: &[&str]) -> (String, String, bool) {
        let out = Command::new(workspace_bin())
            .args(args)
            .output()
            .expect("cli runs");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.success(),
        )
    }

    fn init(&self) -> serde_json::Value {
        let (out, err, ok) = self.run(&[
            "init",
            "--repo",
            self.repo().to_str().unwrap(),
            "--store",
            self.store().to_str().unwrap(),
        ]);
        assert!(ok, "init failed: {err}");
        serde_json::from_str(out.trim()).expect("init json")
    }

    fn publish(&self, key: &str, value: &str) -> serde_json::Value {
        let (out, err, ok) = self.run(&[
            "publish",
            "--repo",
            self.repo().to_str().unwrap(),
            "--store",
            self.store().to_str().unwrap(),
            "--key",
            key,
            "--kind",
            "callers",
            "--value",
            value,
        ]);
        assert!(ok, "publish failed: {err}");
        serde_json::from_str(out.trim()).expect("publish json")
    }

    fn retrieve(&self, key: &str) -> serde_json::Value {
        let (out, err, ok) = self.run(&[
            "retrieve",
            "--repo",
            self.repo().to_str().unwrap(),
            "--store",
            self.store().to_str().unwrap(),
            "--key",
            key,
        ]);
        assert!(ok, "retrieve failed: {err}");
        serde_json::from_str(out.trim()).expect("retrieve json")
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn read_tree(repo: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).expect("readable") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if path.is_dir() {
                walk(&path, &rel, out);
            } else if name.ends_with(".py") {
                out.insert(rel, std::fs::read_to_string(&path).expect("content"));
            }
        }
    }
    walk(repo, "", &mut out);
    out
}

#[test]
fn init_publish_retrieve_round_trip_is_valid() {
    let w = World::new("roundtrip");
    w.init();
    w.publish(
        "auth.tokens.refresh_token",
        "auth.service.login; api.handle",
    );
    let verdict = w.retrieve("auth.tokens.refresh_token");
    assert_eq!(verdict["verdict"], "valid", "{verdict}");
    assert_eq!(verdict["value"], "auth.service.login; api.handle");
}

#[test]
fn mutation_breaks_validity_and_withholds_value() {
    let w = World::new("stale");
    w.init();
    w.publish(
        "auth.tokens.refresh_token",
        "auth.service.login; api.handle",
    );
    let before = w.retrieve("auth.tokens.refresh_token");
    assert_eq!(before["verdict"], "valid");

    std::fs::write(
        w.repo().join("auth").join("service.py"),
        "from auth.tokens import refresh_token\n\ndef login(user_id: str) -> str:\n    return refresh_token(user_id)\n\ndef logout(user_id: str) -> str:\n    return refresh_token(user_id)\n",
    )
    .expect("rewrite service.py");

    let after = w.retrieve("auth.tokens.refresh_token");
    assert_eq!(after["verdict"], "stale", "{after}");
    assert!(
        after.get("value").is_none(),
        "stale retrieve must not serve a value: {after}"
    );
    let explanation = after["explanation"].as_str().unwrap_or_default();
    assert!(
        explanation.contains("changed"),
        "explanation must name the changed dependency: {explanation}"
    );
}

#[test]
fn post_settle_valid_with_stale_pins_is_refused() {
    let w = World::new("pins");
    w.init();
    w.publish("auth.tokens.refresh_token", "auth.service.login");
    std::fs::write(
        w.repo().join("api.py"),
        "from auth.tokens import refresh_token as rt\n\ndef handle(user_id: str) -> str:\n    return rt(user_id)\n\ndef probe(user_id: str) -> str:\n    return rt(user_id)\n",
    )
    .expect("rewrite api.py");
    let first = w.retrieve("auth.tokens.refresh_token");
    assert_eq!(first["verdict"], "stale");
    let second = w.retrieve("auth.tokens.refresh_token");
    let verdict = second["verdict"].as_str().unwrap_or_default();
    assert_ne!(
        verdict, "valid",
        "post-settle re-established Valid must not serve stale pins: {second}"
    );
    assert!(second.get("value").is_none(), "{second}");
}

#[test]
fn republish_reestablishes_valid_with_fresh_value() {
    let w = World::new("republish");
    w.init();
    w.publish("auth.tokens.refresh_token", "auth.service.login");
    std::fs::write(
        w.repo().join("api.py"),
        "from auth.tokens import refresh_token as rt\n\ndef handle(user_id: str) -> str:\n    return rt(user_id)\n\ndef probe(user_id: str) -> str:\n    return rt(user_id)\n",
    )
    .expect("rewrite api.py");
    assert_eq!(w.retrieve("auth.tokens.refresh_token")["verdict"], "stale");
    w.publish(
        "auth.tokens.refresh_token",
        "auth.service.login; api.handle; api.probe",
    );
    let verdict = w.retrieve("auth.tokens.refresh_token");
    assert_eq!(verdict["verdict"], "valid", "{verdict}");
    assert_eq!(
        verdict["value"],
        "auth.service.login; api.handle; api.probe"
    );
}

#[test]
fn query_is_fresh_and_labeled_approximate() {
    let w = World::new("query");
    w.init();
    let (out, err, ok) = w.run(&[
        "query",
        "--repo",
        w.repo().to_str().unwrap(),
        "--kind",
        "callers",
        "--subject",
        "auth.tokens.refresh_token",
    ]);
    assert!(ok, "query failed: {err}");
    let value: serde_json::Value = serde_json::from_str(out.trim()).expect("query json");
    assert_eq!(value["backend"], "cli-textual-approx");
    let before = value["value"].as_str().unwrap_or_default().to_string();

    // A new file with a fresh call site must change the fresh query.
    std::fs::write(
        w.repo().join("webhooks.py"),
        "from auth.tokens import refresh_token\n\ndef on_refresh(user_id: str) -> str:\n    return refresh_token(user_id)\n",
    )
    .expect("webhooks.py");
    let (out, err, ok) = w.run(&[
        "query",
        "--repo",
        w.repo().to_str().unwrap(),
        "--kind",
        "callers",
        "--subject",
        "auth.tokens.refresh_token",
    ]);
    assert!(ok, "query 2 failed: {err}");
    let value: serde_json::Value = serde_json::from_str(out.trim()).expect("query json");
    let after = value["value"].as_str().unwrap_or_default();
    assert_ne!(after, before, "query must recompute from the current tree");
    assert!(after.contains("webhooks.on_refresh"), "{after}");
}

#[test]
fn events_ledger_records_operations() {
    let w = World::new("events");
    w.init();
    w.publish("auth.tokens.refresh_token", "auth.service.login");
    let _ = w.retrieve("auth.tokens.refresh_token");
    let events_path = w.dir.join("store").join("events.jsonl");
    let text = std::fs::read_to_string(&events_path).expect("events ledger");
    assert!(text.contains("\"publish\""), "{text}");
    assert!(text.contains("\"retrieve\""), "{text}");
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        lines.len() >= 2,
        "at least publish + retrieve events: {text}"
    );
}

#[test]
fn retrieve_unknown_key_is_explicit_not_silent() {
    let w = World::new("unknown");
    w.init();
    let verdict = w.retrieve("never.published");
    assert_eq!(verdict["verdict"], "unknown", "{verdict}");
    assert!(
        verdict["explanation"]
            .as_str()
            .unwrap_or_default()
            .contains("no artifact"),
        "{verdict}"
    );
}

#[test]
fn tree_reader_round_trips_fixture_layout() {
    let w = World::new("tree");
    let tree = read_tree(&w.repo());
    assert!(tree.contains_key("auth/tokens.py"));
    assert!(tree.contains_key("api.py"));
    assert_eq!(tree.len(), 3);
}

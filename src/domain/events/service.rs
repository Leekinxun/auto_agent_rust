use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub event: String,
    pub ts: f64,
    #[serde(default)]
    pub task: Value,
    #[serde(default)]
    pub worktree: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct EventService {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl EventService {
    pub fn new(repo_root: PathBuf) -> Result<Self> {
        let path = repo_root.join(".worktrees").join("events.jsonl");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create event dir {}", parent.display()))?;
        }
        if !path.exists() {
            fs::write(&path, "")
                .with_context(|| format!("failed to initialize {}", path.display()))?;
        }
        Ok(Self {
            path,
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn emit(
        &self,
        event: &str,
        task: Option<Value>,
        worktree: Option<Value>,
        error: Option<String>,
    ) -> Result<()> {
        let record = EventRecord {
            event: event.to_string(),
            ts: now_secs_f64(),
            task: task.unwrap_or_else(|| json!({})),
            worktree: worktree.unwrap_or_else(|| json!({})),
            error,
        };
        let line = serde_json::to_string(&record).context("failed to encode event record")?;
        let _guard = self.lock.lock().expect("event lock poisoned");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open {}", self.path.display()))?;
        writeln!(file, "{line}")
            .with_context(|| format!("failed to append {}", self.path.display()))?;
        Ok(())
    }

    pub fn list_recent(&self, limit: usize) -> Result<Vec<Value>> {
        let n = limit.clamp(1, 200);
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let lines = content.lines().collect::<Vec<_>>();
        let mut items = Vec::new();
        for line in lines
            .into_iter()
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            match serde_json::from_str::<Value>(line) {
                Ok(value) => items.push(value),
                Err(_) => items.push(json!({
                    "event": "parse_error",
                    "raw": line,
                })),
            }
        }
        Ok(items)
    }

    pub fn list_recent_pretty(&self, limit: usize) -> Result<String> {
        serde_json::to_string_pretty(&self.list_recent(limit)?)
            .context("failed to encode recent events")
    }
}

fn now_secs_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::EventService;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-events-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time before unix epoch")
                    .as_nanos()
            );
            let root = std::env::temp_dir().join(unique);
            fs::create_dir_all(&root).expect("create temp repo");
            Self { root }
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn emits_and_lists_recent_events() {
        let repo = TestRepo::new();
        let service = EventService::new(repo.root.clone()).unwrap();
        service
            .emit(
                "worktree.keep",
                Some(json!({"id": 1})),
                Some(json!({"name": "demo"})),
                None,
            )
            .unwrap();

        let items = service.list_recent(20).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["event"], json!("worktree.keep"));
        assert_eq!(items[0]["task"]["id"], json!(1));
        assert_eq!(items[0]["worktree"]["name"], json!("demo"));
    }

    #[test]
    fn reloads_events_from_disk() {
        let repo = TestRepo::new();
        let first = EventService::new(repo.root.clone()).unwrap();
        first
            .emit(
                "worktree.create.after",
                None,
                Some(json!({"name": "persisted"})),
                None,
            )
            .unwrap();

        let second = EventService::new(repo.root.clone()).unwrap();
        let items = second.list_recent(20).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["event"], json!("worktree.create.after"));
        assert_eq!(items[0]["worktree"]["name"], json!("persisted"));
    }
}

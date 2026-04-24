use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: u64,
    pub subject: String,
    #[serde(default)]
    pub description: String,
    pub status: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub worktree: String,
    #[serde(default, rename = "blockedBy")]
    pub blocked_by: Vec<u64>,
    #[serde(default)]
    pub blocks: Vec<u64>,
    pub created_at: f64,
    pub updated_at: f64,
}

#[derive(Clone)]
pub struct TaskService {
    dir: PathBuf,
    file_lock: Arc<Mutex<()>>,
    next_id: Arc<Mutex<u64>>,
}

impl TaskService {
    pub fn new(repo_root: PathBuf) -> Result<Self> {
        let dir = repo_root.join(".tasks");
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let max_id = max_task_id(&dir)?;
        Ok(Self {
            dir,
            file_lock: Arc::new(Mutex::new(())),
            next_id: Arc::new(Mutex::new(max_id + 1)),
        })
    }

    pub fn create(&self, subject: &str, description: &str) -> Result<String> {
        let subject = subject.trim();
        if subject.is_empty() {
            bail!("subject cannot be empty");
        }
        let mut next_id = self.next_id.lock().expect("task next_id lock poisoned");
        let now = now_secs_f64();
        let task = TaskRecord {
            id: *next_id,
            subject: subject.to_string(),
            description: description.to_string(),
            status: "pending".to_string(),
            owner: String::new(),
            worktree: String::new(),
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        self.save(&task)?;
        *next_id += 1;
        task_to_pretty_json(&task)
    }

    pub fn get(&self, task_id: u64) -> Result<String> {
        task_to_pretty_json(&self.load(task_id)?)
    }

    pub fn exists(&self, task_id: u64) -> bool {
        self.path(task_id).exists()
    }

    pub fn update(
        &self,
        task_id: u64,
        status: Option<&str>,
        owner: Option<&str>,
        add_blocked_by: &[u64],
        add_blocks: &[u64],
    ) -> Result<String> {
        let mut task = self.load(task_id)?;
        if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
            if !matches!(status, "pending" | "in_progress" | "completed" | "deleted") {
                bail!("Invalid status: {status}");
            }
            if status == "deleted" {
                fs::remove_file(self.path(task_id))
                    .with_context(|| format!("failed to delete task {task_id}"))?;
                return Ok(format!("Task {task_id} deleted"));
            }
            task.status = status.to_string();
            if status == "completed" {
                for mut other in self.list_records()? {
                    if other.id == task_id {
                        continue;
                    }
                    let original_len = other.blocked_by.len();
                    other.blocked_by.retain(|item| *item != task_id);
                    if other.blocked_by.len() != original_len {
                        self.save(&other)?;
                    }
                }
            }
        }
        if let Some(owner) = owner {
            task.owner = owner.to_string();
        }
        if !add_blocked_by.is_empty() {
            task.blocked_by = merge_ids(&task.blocked_by, add_blocked_by);
        }
        if !add_blocks.is_empty() {
            task.blocks = merge_ids(&task.blocks, add_blocks);
        }
        task.updated_at = now_secs_f64();
        self.save(&task)?;
        task_to_pretty_json(&task)
    }

    pub fn claim(&self, task_id: u64, owner: &str) -> Result<String> {
        let mut task = self.load(task_id)?;
        task.owner = owner.to_string();
        task.status = "in_progress".to_string();
        task.updated_at = now_secs_f64();
        self.save(&task)?;
        Ok(format!("Claimed task #{task_id} for {owner}"))
    }

    pub fn bind_worktree(
        &self,
        task_id: u64,
        worktree: &str,
        owner: Option<&str>,
    ) -> Result<String> {
        let mut task = self.load(task_id)?;
        task.worktree = worktree.to_string();
        if let Some(owner) = owner {
            task.owner = owner.to_string();
        }
        if task.status == "pending" {
            task.status = "in_progress".to_string();
        }
        task.updated_at = now_secs_f64();
        self.save(&task)?;
        task_to_pretty_json(&task)
    }

    pub fn unbind_worktree(&self, task_id: u64) -> Result<String> {
        let mut task = self.load(task_id)?;
        task.worktree.clear();
        task.updated_at = now_secs_f64();
        self.save(&task)?;
        task_to_pretty_json(&task)
    }

    pub fn list_all(&self) -> Result<String> {
        let tasks = self.list_records()?;
        if tasks.is_empty() {
            return Ok("No tasks.".to_string());
        }
        let lines = tasks
            .iter()
            .map(|task| {
                let marker = match task.status.as_str() {
                    "pending" => "[ ]",
                    "in_progress" => "[>]",
                    "completed" => "[x]",
                    _ => "[?]",
                };
                let owner = if task.owner.is_empty() {
                    String::new()
                } else {
                    format!(" owner={}", task.owner)
                };
                let wt = if task.worktree.is_empty() {
                    String::new()
                } else {
                    format!(" wt={}", task.worktree)
                };
                format!("{marker} #{}: {}{owner}{wt}", task.id, task.subject)
            })
            .collect::<Vec<_>>();
        Ok(lines.join("\n"))
    }

    pub fn find_claimable_task(&self) -> Result<Option<TaskRecord>> {
        Ok(self.list_records()?.into_iter().find(|task| {
            task.status == "pending" && task.owner.is_empty() && task.blocked_by.is_empty()
        }))
    }

    pub fn dispatch_public_tool(&self, name: &str, arguments: &Value) -> Option<String> {
        match name {
            "task_create" => Some(format_tool_result(
                required_string_arg(arguments, "subject", "task_create").and_then(|subject| {
                    Ok(self.create(subject, optional_string_arg(arguments, "description"))?)
                }),
            )),
            "task_list" => Some(format_tool_result(self.list_all())),
            "task_get" => Some(format_tool_result(
                required_u64_arg(arguments, "task_id", "task_get")
                    .and_then(|task_id| self.get(task_id)),
            )),
            "task_update" => Some(format_tool_result(
                required_u64_arg(arguments, "task_id", "task_update").and_then(|task_id| {
                    self.update(
                        task_id,
                        optional_non_empty_string_arg(arguments, "status"),
                        optional_string_value(arguments, "owner"),
                        &optional_u64_list(arguments, "add_blocked_by")?,
                        &optional_u64_list(arguments, "add_blocks")?,
                    )
                }),
            )),
            "task_bind_worktree" => Some(format_tool_result(
                required_u64_arg(arguments, "task_id", "task_bind_worktree").and_then(|task_id| {
                    self.bind_worktree(
                        task_id,
                        required_string_arg(arguments, "worktree", "task_bind_worktree")?,
                        optional_string_value(arguments, "owner"),
                    )
                }),
            )),
            _ => None,
        }
    }

    fn list_records(&self) -> Result<Vec<TaskRecord>> {
        let mut tasks = Vec::new();
        for entry in fs::read_dir(&self.dir)
            .with_context(|| format!("failed to read {}", self.dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.starts_with("task_") || !name.ends_with(".json") {
                continue;
            }
            let content = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let task = serde_json::from_str::<TaskRecord>(&content)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            tasks.push(task);
        }
        tasks.sort_by_key(|task| task.id);
        Ok(tasks)
    }

    fn load(&self, task_id: u64) -> Result<TaskRecord> {
        let path = self.path(task_id);
        if !path.exists() {
            bail!("Task {task_id} not found");
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))
    }

    fn save(&self, task: &TaskRecord) -> Result<()> {
        let _guard = self.file_lock.lock().expect("task file lock poisoned");
        let path = self.path(task.id);
        let encoded = serde_json::to_string_pretty(task).context("failed to encode task json")?;
        fs::write(&path, encoded).with_context(|| format!("failed to write {}", path.display()))
    }

    fn path(&self, task_id: u64) -> PathBuf {
        self.dir.join(format!("task_{task_id}.json"))
    }
}

pub fn public_task_tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "task_create",
                "description": "Create a new task on the shared task board.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "subject": { "type": "string" },
                        "description": { "type": "string" }
                    },
                    "required": ["subject"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_list",
                "description": "List all tasks with status, owner, and worktree binding.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_get",
                "description": "Get task details by ID.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer" }
                    },
                    "required": ["task_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_update",
                "description": "Update task status, owner, or dependencies.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "deleted"] },
                        "owner": { "type": "string" },
                        "add_blocked_by": { "type": "array", "items": { "type": "integer" } },
                        "add_blocks": { "type": "array", "items": { "type": "integer" } }
                    },
                    "required": ["task_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_bind_worktree",
                "description": "Bind a task to a worktree name.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer" },
                        "worktree": { "type": "string" },
                        "owner": { "type": "string" }
                    },
                    "required": ["task_id", "worktree"]
                }
            }
        }),
    ]
}

fn max_task_id(dir: &PathBuf) -> Result<u64> {
    let mut max_id = 0;
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if let Some(raw_id) = stem.strip_prefix("task_")
            && let Ok(id) = raw_id.parse::<u64>()
        {
            max_id = max_id.max(id);
        }
    }
    Ok(max_id)
}

fn task_to_pretty_json(task: &TaskRecord) -> Result<String> {
    serde_json::to_string_pretty(task).context("failed to encode task")
}

fn merge_ids(left: &[u64], right: &[u64]) -> Vec<u64> {
    left.iter()
        .chain(right.iter())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn required_string_arg<'a>(arguments: &'a Value, key: &str, tool_name: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{tool_name} requires a non-empty {key}"))
}

fn optional_string_arg<'a>(arguments: &'a Value, key: &str) -> &'a str {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn optional_string_value<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(Value::as_str)
}

fn optional_non_empty_string_arg<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    optional_string_value(arguments, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_u64_arg(arguments: &Value, key: &str, tool_name: &str) -> Result<u64> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("{tool_name} requires integer {key}"))
}

fn optional_u64_list(arguments: &Value, key: &str) -> Result<Vec<u64>> {
    let Some(value) = arguments.get(key) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        bail!("{key} must be an array");
    };
    items
        .iter()
        .map(|item| {
            item.as_u64()
                .with_context(|| format!("{key} items must be integers"))
        })
        .collect()
}

fn format_tool_result(result: Result<String>) -> String {
    result.unwrap_or_else(|error| format!("Error: {error}"))
}

fn now_secs_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::TaskService;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-tasks-{}-{}",
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
    fn creates_updates_and_binds_tasks() {
        let repo = TestRepo::new();
        let service = TaskService::new(repo.root.clone()).unwrap();

        let first = service.create("First task", "").unwrap();
        assert!(first.contains("\"id\": 1"));
        let second = service.create("Second task", "").unwrap();
        assert!(second.contains("\"id\": 2"));

        assert_eq!(
            service.claim(1, "lead").unwrap(),
            "Claimed task #1 for lead"
        );
        service.update(2, None, None, &[1], &[]).unwrap();
        service.bind_worktree(1, "demo", Some("lead")).unwrap();
        service
            .update(1, Some("completed"), None, &[], &[])
            .unwrap();
        let second_task = service.get(2).unwrap();

        assert!(!second_task.contains("\"blockedBy\": [\n    1"));
        assert!(
            service
                .list_all()
                .unwrap()
                .contains("#1: First task owner=lead wt=demo")
        );
    }

    #[test]
    fn reloads_persisted_tasks_and_advances_next_id() {
        let repo = TestRepo::new();
        let first_service = TaskService::new(repo.root.clone()).unwrap();
        first_service.create("Persisted task", "").unwrap();
        first_service
            .update(1, Some("in_progress"), Some("lead"), &[], &[])
            .unwrap();

        let second_service = TaskService::new(repo.root.clone()).unwrap();
        let persisted = second_service.get(1).unwrap();
        assert!(persisted.contains("\"status\": \"in_progress\""));
        assert!(persisted.contains("\"owner\": \"lead\""));

        let created = second_service.create("Second task", "").unwrap();
        assert!(created.contains("\"id\": 2"));
    }

    #[test]
    fn finds_first_claimable_task() {
        let repo = TestRepo::new();
        let service = TaskService::new(repo.root.clone()).unwrap();
        service.create("Blocked task", "").unwrap();
        service.create("Claimable task", "").unwrap();
        service.update(1, None, None, &[99], &[]).unwrap();

        let claimable = service.find_claimable_task().unwrap().unwrap();
        assert_eq!(claimable.id, 2);
        assert_eq!(claimable.subject, "Claimable task");
    }
}

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command as TokioCommand;
use tokio::time::{Duration, timeout};

use crate::domain::events::service::EventService;
use crate::domain::tasks::service::TaskService;

const MAX_COMMAND_OUTPUT_CHARS: usize = 50_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeRecord {
    pub name: String,
    pub path: String,
    pub branch: String,
    pub task_id: Option<u64>,
    pub status: String,
    pub created_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_at: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorktreeIndex {
    #[serde(default)]
    worktrees: Vec<WorktreeRecord>,
}

#[derive(Clone)]
pub struct WorktreeService {
    repo_root: PathBuf,
    dir: PathBuf,
    index_path: PathBuf,
    tasks: TaskService,
    events: EventService,
    lock: Arc<Mutex<()>>,
    git_available: bool,
    name_regex: Regex,
}

impl WorktreeService {
    pub fn new(repo_root: PathBuf, tasks: TaskService, events: EventService) -> Result<Self> {
        let dir = repo_root.join(".worktrees");
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let index_path = dir.join("index.json");
        if !index_path.exists() {
            let encoded = serde_json::to_string_pretty(&WorktreeIndex {
                worktrees: Vec::new(),
            })
            .context("failed to encode default worktree index")?;
            fs::write(&index_path, encoded)
                .with_context(|| format!("failed to initialize {}", index_path.display()))?;
        }
        Ok(Self {
            repo_root: repo_root.clone(),
            dir,
            index_path,
            tasks,
            events,
            lock: Arc::new(Mutex::new(())),
            git_available: is_git_repo(&repo_root),
            name_regex: Regex::new(r"^[A-Za-z0-9._-]{1,40}$").expect("valid worktree regex"),
        })
    }

    pub fn list_all(&self) -> Result<String> {
        let index = self.load_index()?;
        if index.worktrees.is_empty() {
            return Ok("No worktrees.".to_string());
        }
        Ok(index
            .worktrees
            .iter()
            .map(|wt| {
                format!(
                    "[{}] {} -> {} ({}){}",
                    wt.status,
                    wt.name,
                    wt.path,
                    wt.branch,
                    wt.task_id
                        .map(|task_id| format!(" task={task_id}"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub fn keep(&self, name: &str) -> Result<String> {
        let mut index = self.load_index()?;
        let mut kept = None;
        for item in &mut index.worktrees {
            if item.name == name {
                item.status = "kept".to_string();
                item.kept_at = Some(now_secs_f64());
                kept = Some(item.clone());
            }
        }
        let Some(kept) = kept else {
            return Ok(format!("Error: Unknown worktree '{name}'"));
        };
        self.save_index(&index)?;
        self.events.emit(
            "worktree.keep",
            None,
            Some(json!({ "name": name, "status": "kept" })),
            None,
        )?;
        serde_json::to_string_pretty(&kept).context("failed to encode kept worktree")
    }

    pub fn list_events(&self, limit: usize) -> Result<String> {
        self.events.list_recent_pretty(limit)
    }

    pub fn dispatch_public_tool_sync(&self, name: &str, arguments: &Value) -> Option<String> {
        match name {
            "worktree_list" => Some(format_tool_result(self.list_all())),
            "worktree_keep" => Some(format_tool_result(
                required_string_arg(arguments, "name", "worktree_keep")
                    .and_then(|name| self.keep(name)),
            )),
            "worktree_events" => Some(format_tool_result(
                optional_limit(arguments, "limit")
                    .and_then(|limit| self.list_events(limit.unwrap_or(20))),
            )),
            _ => None,
        }
    }

    pub async fn dispatch_public_tool_async(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Option<String> {
        match name {
            "worktree_create" => Some(format_tool_result(
                match required_string_arg(arguments, "name", "worktree_create") {
                    Ok(name) => {
                        self.create(
                            name,
                            optional_u64(arguments, "task_id"),
                            optional_string(arguments, "base_ref").unwrap_or("HEAD"),
                        )
                        .await
                    }
                    Err(error) => Err(error),
                },
            )),
            "worktree_status" => Some(format_tool_result(
                match required_string_arg(arguments, "name", "worktree_status") {
                    Ok(name) => self.status(name).await,
                    Err(error) => Err(error),
                },
            )),
            "worktree_run" => Some(format_tool_result(
                match (
                    required_string_arg(arguments, "name", "worktree_run"),
                    required_string_arg(arguments, "command", "worktree_run"),
                ) {
                    (Ok(name), Ok(command)) => self.run(name, command).await,
                    (Err(error), _) | (_, Err(error)) => Err(error),
                },
            )),
            "worktree_remove" => Some(format_tool_result(
                match required_string_arg(arguments, "name", "worktree_remove") {
                    Ok(name) => {
                        self.remove(
                            name,
                            optional_bool(arguments, "force"),
                            optional_bool(arguments, "complete_task"),
                        )
                        .await
                    }
                    Err(error) => Err(error),
                },
            )),
            _ => self.dispatch_public_tool_sync(name, arguments),
        }
    }

    async fn create(&self, name: &str, task_id: Option<u64>, base_ref: &str) -> Result<String> {
        self.validate_name(name)?;
        if self.find(name)?.is_some() {
            bail!("Worktree '{name}' already exists");
        }
        if let Some(task_id) = task_id
            && !self.tasks.exists(task_id)
        {
            bail!("Task {task_id} not found");
        }

        let path = self.dir.join(name);
        let branch = format!("wt/{name}");
        self.events.emit(
            "worktree.create.before",
            task_id.map(|id| json!({ "id": id })),
            Some(json!({ "name": name })),
            None,
        )?;

        match self
            .run_git(
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    &path.display().to_string(),
                    base_ref,
                ],
                120,
            )
            .await
        {
            Ok(_) => {
                let entry = WorktreeRecord {
                    name: name.to_string(),
                    path: path.display().to_string(),
                    branch,
                    task_id,
                    status: "active".to_string(),
                    created_at: now_secs_f64(),
                    removed_at: None,
                    kept_at: None,
                };
                let mut index = self.load_index()?;
                index.worktrees.push(entry.clone());
                self.save_index(&index)?;
                if let Some(task_id) = task_id {
                    let _ = self.tasks.bind_worktree(task_id, name, None)?;
                }
                self.events.emit(
                    "worktree.create.after",
                    None,
                    Some(serde_json::to_value(&entry).context("failed to encode worktree event")?),
                    None,
                )?;
                serde_json::to_string_pretty(&entry).context("failed to encode worktree")
            }
            Err(error) => {
                let message = error.to_string();
                let _ =
                    self.events
                        .emit("worktree.create.failed", None, None, Some(message.clone()));
                Err(error)
            }
        }
    }

    async fn status(&self, name: &str) -> Result<String> {
        let Some(wt) = self.find(name)? else {
            return Ok(format!("Error: Unknown worktree '{name}'"));
        };
        run_command_capture(
            "git",
            &["status", "--short", "--branch"],
            Some(PathBuf::from(wt.path)),
            60,
        )
        .await
    }

    async fn run(&self, name: &str, command: &str) -> Result<String> {
        if is_dangerous_shell_command(command) {
            return Ok("Error: Dangerous command blocked".to_string());
        }
        let Some(wt) = self.find(name)? else {
            return Ok(format!("Error: Unknown worktree '{name}'"));
        };
        run_command_capture("sh", &["-lc", command], Some(PathBuf::from(wt.path)), 300).await
    }

    async fn remove(&self, name: &str, force: bool, complete_task: bool) -> Result<String> {
        let Some(wt) = self.find(name)? else {
            return Ok(format!("Error: Unknown worktree '{name}'"));
        };
        self.events.emit(
            "worktree.remove.before",
            None,
            Some(json!({ "name": name })),
            None,
        )?;

        let mut args = vec!["worktree".to_string(), "remove".to_string()];
        if force {
            args.push("--force".to_string());
        }
        args.push(wt.path.clone());

        match self.run_git_owned(&args, 120).await {
            Ok(_) => {
                if complete_task && let Some(task_id) = wt.task_id {
                    let _ = self
                        .tasks
                        .update(task_id, Some("completed"), None, &[], &[])?;
                    let _ = self.tasks.unbind_worktree(task_id)?;
                }
                let mut index = self.load_index()?;
                for item in &mut index.worktrees {
                    if item.name == name {
                        item.status = "removed".to_string();
                        item.removed_at = Some(now_secs_f64());
                    }
                }
                self.save_index(&index)?;
                self.events.emit(
                    "worktree.remove.after",
                    None,
                    Some(json!({ "name": name, "status": "removed" })),
                    None,
                )?;
                Ok(format!("Removed worktree '{name}'"))
            }
            Err(error) => {
                let message = error.to_string();
                let _ =
                    self.events
                        .emit("worktree.remove.failed", None, None, Some(message.clone()));
                Err(error)
            }
        }
    }

    fn validate_name(&self, name: &str) -> Result<()> {
        if !self.name_regex.is_match(name) {
            bail!("Invalid worktree name. Use 1-40 chars: letters, numbers, ., _, -");
        }
        Ok(())
    }

    fn find(&self, name: &str) -> Result<Option<WorktreeRecord>> {
        Ok(self
            .load_index()?
            .worktrees
            .into_iter()
            .find(|wt| wt.name == name))
    }

    fn load_index(&self) -> Result<WorktreeIndex> {
        let content = fs::read_to_string(&self.index_path)
            .with_context(|| format!("failed to read {}", self.index_path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", self.index_path.display()))
    }

    fn save_index(&self, index: &WorktreeIndex) -> Result<()> {
        let _guard = self.lock.lock().expect("worktree lock poisoned");
        let encoded =
            serde_json::to_string_pretty(index).context("failed to encode worktree index")?;
        fs::write(&self.index_path, encoded)
            .with_context(|| format!("failed to write {}", self.index_path.display()))
    }

    async fn run_git(&self, args: &[&str], timeout_secs: u64) -> Result<String> {
        self.run_git_owned(
            &args.iter().map(|item| item.to_string()).collect::<Vec<_>>(),
            timeout_secs,
        )
        .await
    }

    async fn run_git_owned(&self, args: &[String], timeout_secs: u64) -> Result<String> {
        if !self.git_available {
            bail!("Not in a git repository.");
        }
        run_command_capture_owned("git", args, Some(self.repo_root.clone()), timeout_secs).await
    }
}

pub fn public_worktree_tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "worktree_create",
                "description": "Create a git worktree and optionally bind it to a task.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "task_id": { "type": "integer" },
                        "base_ref": { "type": "string" }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "worktree_list",
                "description": "List worktrees tracked in .worktrees/index.json.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "worktree_status",
                "description": "Show git status for one worktree.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "worktree_run",
                "description": "Run a shell command in a named worktree directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "command": { "type": "string" }
                    },
                    "required": ["name", "command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "worktree_remove",
                "description": "Remove a worktree and optionally mark its bound task completed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "force": { "type": "boolean" },
                        "complete_task": { "type": "boolean" }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "worktree_keep",
                "description": "Mark a worktree as kept without removing it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "worktree_events",
                "description": "List recent worktree/task lifecycle events.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer" }
                    }
                }
            }
        }),
    ]
}

fn is_git_repo(repo_root: &PathBuf) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(repo_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn run_command_capture(
    program: &str,
    args: &[&str],
    cwd: Option<PathBuf>,
    timeout_secs: u64,
) -> Result<String> {
    run_command_capture_owned(
        program,
        &args.iter().map(|item| item.to_string()).collect::<Vec<_>>(),
        cwd,
        timeout_secs,
    )
    .await
}

async fn run_command_capture_owned(
    program: &str,
    args: &[String],
    cwd: Option<PathBuf>,
    timeout_secs: u64,
) -> Result<String> {
    let mut command = TokioCommand::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = timeout(Duration::from_secs(timeout_secs), command.output())
        .await
        .with_context(|| format!("Timeout ({timeout_secs}s)"))?
        .with_context(|| format!("failed to spawn {program}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = truncate_for_command_output(format!("{}{}", stdout, stderr).trim().to_string());
    if !output.status.success() {
        bail!(
            "{}",
            if combined.is_empty() {
                "(no output)".to_string()
            } else {
                combined
            }
        );
    }
    Ok(if combined.is_empty() {
        if program == "git" && args.first().map(|item| item.as_str()) == Some("status") {
            "Clean worktree".to_string()
        } else {
            "(no output)".to_string()
        }
    } else {
        combined
    })
}

fn truncate_for_command_output(text: String) -> String {
    if text.chars().count() <= MAX_COMMAND_OUTPUT_CHARS {
        return text;
    }
    text.chars().take(MAX_COMMAND_OUTPUT_CHARS).collect()
}

fn is_dangerous_shell_command(command: &str) -> bool {
    ["rm -rf /", "sudo", "shutdown", "reboot"]
        .iter()
        .any(|token| command.contains(token))
}

fn required_string_arg<'a>(arguments: &'a Value, key: &str, tool_name: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{tool_name} requires a non-empty {key}"))
}

fn optional_string<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn optional_u64(arguments: &Value, key: &str) -> Option<u64> {
    arguments.get(key).and_then(Value::as_u64)
}

fn optional_bool(arguments: &Value, key: &str) -> bool {
    arguments.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn optional_limit(arguments: &Value, key: &str) -> Result<Option<usize>> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    let Some(raw) = value.as_u64() else {
        bail!("{key} must be an integer");
    };
    Ok(Some(raw as usize))
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
    use super::{WorktreeIndex, WorktreeRecord, WorktreeService};
    use crate::domain::events::service::EventService;
    use crate::domain::tasks::service::TaskService;
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
                "auto-claude-worktrees-{}-{}",
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

    #[tokio::test]
    async fn create_reports_error_when_not_git_repo() {
        let repo = TestRepo::new();
        let tasks = TaskService::new(repo.root.clone()).unwrap();
        let events = EventService::new(repo.root.clone()).unwrap();
        let service = WorktreeService::new(repo.root.clone(), tasks, events.clone()).unwrap();

        let result = service
            .dispatch_public_tool_async("worktree_create", &json!({ "name": "demo" }))
            .await
            .unwrap();

        assert!(result.contains("Error: Not in a git repository."));
        let recent = events.list_recent(20).unwrap();
        assert!(
            recent
                .iter()
                .any(|item| item["event"] == json!("worktree.create.failed"))
        );
    }

    #[test]
    fn reloads_persisted_worktree_index() {
        let repo = TestRepo::new();
        let tasks = TaskService::new(repo.root.clone()).unwrap();
        let events = EventService::new(repo.root.clone()).unwrap();
        let service =
            WorktreeService::new(repo.root.clone(), tasks.clone(), events.clone()).unwrap();

        service
            .save_index(&WorktreeIndex {
                worktrees: vec![WorktreeRecord {
                    name: "demo".to_string(),
                    path: repo.root.join(".worktrees/demo").display().to_string(),
                    branch: "wt/demo".to_string(),
                    task_id: Some(1),
                    status: "active".to_string(),
                    created_at: 1.0,
                    removed_at: None,
                    kept_at: None,
                }],
            })
            .unwrap();

        let reopened = WorktreeService::new(repo.root.clone(), tasks, events).unwrap();
        let listed = reopened.list_all().unwrap();
        assert!(listed.contains("[active] demo"));
        assert!(listed.contains("task=1"));
    }
}

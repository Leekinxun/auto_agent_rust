use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const MAX_OUTPUT_CHARS: usize = 50_000;
const MAX_NOTIFICATION_CHARS: usize = 500;

#[derive(Debug, Clone)]
struct BackgroundTask {
    status: String,
    command: String,
    result: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundNotification {
    pub task_id: String,
    pub status: String,
    pub result: String,
}

#[derive(Clone)]
pub struct BackgroundManager {
    workdir: PathBuf,
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
    notifications: Arc<Mutex<VecDeque<BackgroundNotification>>>,
    next_id: Arc<AtomicU64>,
}

impl BackgroundManager {
    pub fn new(workdir: PathBuf) -> Self {
        Self {
            workdir,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            notifications: Arc::new(Mutex::new(VecDeque::new())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn run(&self, command: &str, timeout_secs: u64) -> String {
        let command = command.trim();
        if command.is_empty() {
            return "Error: background_run requires a non-empty command".to_string();
        }
        if is_dangerous(command) {
            return "Error: Dangerous command blocked".to_string();
        }

        let task_id = format!("{:08x}", self.next_id.fetch_add(1, Ordering::SeqCst));
        {
            let mut tasks = self.tasks.lock().expect("background tasks lock poisoned");
            tasks.insert(
                task_id.clone(),
                BackgroundTask {
                    status: "running".to_string(),
                    command: command.to_string(),
                    result: None,
                },
            );
        }

        let tasks = self.tasks.clone();
        let notifications = self.notifications.clone();
        let workdir = self.workdir.clone();
        let task_id_clone = task_id.clone();
        let command_text = command.to_string();

        tokio::spawn(async move {
            let (status, result) =
                match execute_background_command(&workdir, &command_text, timeout_secs).await {
                    Ok(output) => ("completed".to_string(), output),
                    Err(error) => ("error".to_string(), error),
                };

            {
                let mut tasks = tasks.lock().expect("background tasks lock poisoned");
                if let Some(task) = tasks.get_mut(&task_id_clone) {
                    task.status = status.clone();
                    task.result = Some(result.clone());
                }
            }

            notifications
                .lock()
                .expect("background notifications lock poisoned")
                .push_back(BackgroundNotification {
                    task_id: task_id_clone,
                    status,
                    result: truncate(result, MAX_NOTIFICATION_CHARS),
                });
        });

        format!(
            "Background task {} started: {}",
            task_id,
            truncate(command.to_string(), 80)
        )
    }

    pub fn check(&self, task_id: Option<&str>) -> String {
        let tasks = self.tasks.lock().expect("background tasks lock poisoned");
        if let Some(task_id) = task_id.map(str::trim).filter(|value| !value.is_empty()) {
            return match tasks.get(task_id) {
                Some(task) => format!(
                    "[{}] {}",
                    task.status,
                    task.result
                        .clone()
                        .unwrap_or_else(|| "(running)".to_string())
                ),
                None => format!("Unknown: {task_id}"),
            };
        }

        if tasks.is_empty() {
            return "No bg tasks.".to_string();
        }

        tasks
            .iter()
            .map(|(task_id, task)| {
                format!(
                    "{task_id}: [{}] {}",
                    task.status,
                    truncate(task.command.clone(), 60)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn drain(&self) -> Vec<BackgroundNotification> {
        let mut notifications = self
            .notifications
            .lock()
            .expect("background notifications lock poisoned");
        notifications.drain(..).collect()
    }
}

async fn execute_background_command(
    workdir: &PathBuf,
    command: &str,
    timeout_secs: u64,
) -> std::result::Result<String, String> {
    let mut child = Command::new("sh");
    child
        .args(["-lc", command])
        .current_dir(workdir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = timeout(Duration::from_secs(timeout_secs), child.output())
        .await
        .map_err(|_| format!("Timeout ({timeout_secs}s)"))?
        .map_err(|error| error.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = truncate(
        format!("{}{}", stdout, stderr).trim().to_string(),
        MAX_OUTPUT_CHARS,
    );

    if !output.status.success() {
        return Err(if combined.is_empty() {
            "(no output)".to_string()
        } else {
            combined
        });
    }

    Ok(if combined.is_empty() {
        "(no output)".to_string()
    } else {
        combined
    })
}

fn is_dangerous(command: &str) -> bool {
    ["rm -rf /", "sudo", "shutdown", "reboot"]
        .iter()
        .any(|token| command.contains(token))
}

fn truncate(text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::BackgroundManager;

    #[tokio::test]
    async fn background_task_completes_and_notifies() {
        let manager = BackgroundManager::new(std::env::temp_dir());
        let started = manager.run("printf hello", 5);
        assert!(started.contains("Background task"));

        for _ in 0..20 {
            if !manager.drain().is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        panic!("background notification was not produced");
    }
}

use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

#[derive(Clone, Default)]
pub struct TodoManager {
    items: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoManager {
    pub fn update(&self, items: Vec<TodoItem>) -> Result<String> {
        let validated = validate_items(items)?;
        let mut guard = self.items.lock().expect("todo manager lock poisoned");
        *guard = validated;
        Ok(render_items(&guard))
    }

    pub fn has_open_items(&self) -> bool {
        self.items
            .lock()
            .expect("todo manager lock poisoned")
            .iter()
            .any(|item| item.status != "completed")
    }

    pub fn list_items(&self) -> Vec<TodoItem> {
        self.items
            .lock()
            .expect("todo manager lock poisoned")
            .clone()
    }

    #[allow(dead_code)]
    pub fn render(&self) -> String {
        render_items(&self.items.lock().expect("todo manager lock poisoned"))
    }
}

fn validate_items(items: Vec<TodoItem>) -> Result<Vec<TodoItem>> {
    if items.len() > 20 {
        bail!("Max 20 todos");
    }
    let mut validated = Vec::with_capacity(items.len());
    let mut in_progress = 0usize;
    for (index, item) in items.into_iter().enumerate() {
        let content = item.content.trim().to_string();
        if content.is_empty() {
            bail!("Item {index}: content required");
        }
        let status = item.status.trim().to_ascii_lowercase();
        if !matches!(status.as_str(), "pending" | "in_progress" | "completed") {
            bail!("Item {index}: invalid status '{status}'");
        }
        let active_form = item.active_form.trim().to_string();
        if active_form.is_empty() {
            bail!("Item {index}: activeForm required");
        }
        if status == "in_progress" {
            in_progress += 1;
        }
        validated.push(TodoItem {
            content,
            status,
            active_form,
        });
    }
    if in_progress > 1 {
        bail!("Only one in_progress allowed");
    }
    Ok(validated)
}

fn render_items(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "No todos.".to_string();
    }
    let mut lines = Vec::with_capacity(items.len() + 1);
    for item in items {
        let marker = match item.status.as_str() {
            "completed" => "[x]",
            "in_progress" => "[>]",
            "pending" => "[ ]",
            _ => "[?]",
        };
        let suffix = if item.status == "in_progress" {
            format!(" <- {}", item.active_form)
        } else {
            String::new()
        };
        lines.push(format!("{marker} {}{suffix}", item.content));
    }
    let done = items
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    lines.push(format!("\n({done}/{} completed)", items.len()));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{TodoItem, TodoManager};

    #[test]
    fn validates_and_renders_todos() {
        let manager = TodoManager::default();
        let output = manager
            .update(vec![
                TodoItem {
                    content: "Write tests".to_string(),
                    status: "in_progress".to_string(),
                    active_form: "Writing tests".to_string(),
                },
                TodoItem {
                    content: "Review".to_string(),
                    status: "pending".to_string(),
                    active_form: "Reviewing".to_string(),
                },
            ])
            .unwrap();

        assert!(output.contains("[>] Write tests <- Writing tests"));
        assert!(manager.has_open_items());
    }
}

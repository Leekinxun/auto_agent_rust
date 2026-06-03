use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use sha1::{Digest, Sha1};
use tokio::fs;

use crate::infra::llm::types::ChatMessage;

const DEFAULT_RESULT_SIZE_CHARS: usize = 100_000;
const DEFAULT_TURN_BUDGET_CHARS: usize = 200_000;
const DEFAULT_PREVIEW_SIZE_CHARS: usize = 1_500;
const RESULT_STORAGE_DIR: &str = ".tool-results";
const PERSISTED_OUTPUT_TAG: &str = "<persisted-output>";
const PERSISTED_OUTPUT_CLOSING_TAG: &str = "</persisted-output>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolContextBudget {
    pub result_size_chars: usize,
    pub turn_budget_chars: usize,
    pub preview_size_chars: usize,
}

impl Default for ToolContextBudget {
    fn default() -> Self {
        Self {
            result_size_chars: DEFAULT_RESULT_SIZE_CHARS,
            turn_budget_chars: DEFAULT_TURN_BUDGET_CHARS,
            preview_size_chars: DEFAULT_PREVIEW_SIZE_CHARS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolContextResult {
    pub model_content: String,
    pub persisted_path: Option<PathBuf>,
}

pub async fn prepare_tool_result_for_context(
    repo_root: &Path,
    tool_name: &str,
    tool_call_id: &str,
    content: String,
    budget: ToolContextBudget,
) -> ToolContextResult {
    if should_inline_tool_result(tool_name) || content.chars().count() <= budget.result_size_chars {
        return ToolContextResult {
            model_content: content,
            persisted_path: None,
        };
    }

    persist_large_tool_result(repo_root, tool_name, tool_call_id, &content, budget).await
}

pub async fn enforce_tool_turn_budget(
    repo_root: &Path,
    messages: &mut [ChatMessage],
    turn_tool_message_start: usize,
    budget: ToolContextBudget,
) {
    if turn_tool_message_start >= messages.len() {
        return;
    }

    let mut total_chars = messages[turn_tool_message_start..]
        .iter()
        .filter(|message| message.role == "tool")
        .map(|message| {
            message
                .content
                .as_deref()
                .unwrap_or_default()
                .chars()
                .count()
        })
        .sum::<usize>();
    if total_chars <= budget.turn_budget_chars {
        return;
    }

    let mut candidates = messages[turn_tool_message_start..]
        .iter()
        .enumerate()
        .filter_map(|(offset, message)| {
            if message.role != "tool" {
                return None;
            }
            let content = message.content.as_deref().unwrap_or_default();
            if content.contains(PERSISTED_OUTPUT_TAG) {
                return None;
            }
            Some((turn_tool_message_start + offset, content.chars().count()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.1.cmp(&left.1));

    for (index, original_chars) in candidates {
        if total_chars <= budget.turn_budget_chars {
            break;
        }
        let Some(content) = messages[index].content.clone() else {
            continue;
        };
        let tool_call_id = messages[index]
            .tool_call_id
            .clone()
            .unwrap_or_else(|| format!("tool-{index}"));
        let replacement = persist_large_tool_result(
            repo_root,
            "turn_budget",
            &tool_call_id,
            &content,
            ToolContextBudget {
                result_size_chars: 0,
                ..budget
            },
        )
        .await;
        let replacement_chars = replacement.model_content.chars().count();
        if replacement.model_content != content && replacement_chars < original_chars {
            messages[index].content = Some(replacement.model_content);
            total_chars = total_chars
                .saturating_sub(original_chars)
                .saturating_add(replacement_chars);
        }
    }
}

fn should_inline_tool_result(tool_name: &str) -> bool {
    // Avoid persist -> read_file -> persist loops. Local read_file already has its own cap.
    tool_name == "read_file"
}

async fn persist_large_tool_result(
    repo_root: &Path,
    tool_name: &str,
    tool_call_id: &str,
    content: &str,
    budget: ToolContextBudget,
) -> ToolContextResult {
    let preview = generate_preview(content, budget.preview_size_chars);
    let content_chars = content.chars().count();
    match write_tool_result(repo_root, tool_name, tool_call_id, content).await {
        Ok(path) => {
            let display_path = path.display().to_string();
            tracing::info!(
                tool = tool_name,
                tool_call_id,
                chars = content_chars,
                path = %display_path,
                "persisted oversized tool result before adding it to model context"
            );
            ToolContextResult {
                model_content: build_persisted_message(&preview, content_chars, &display_path),
                persisted_path: Some(path),
            }
        }
        Err(error) => {
            tracing::warn!(
                ?error,
                tool = tool_name,
                tool_call_id,
                chars = content_chars,
                "failed to persist oversized tool result; falling back to inline preview"
            );
            ToolContextResult {
                model_content: format!(
                    "{preview}\n\n[Truncated: tool response was {content_chars} characters. Full output could not be saved to workspace storage.]"
                ),
                persisted_path: None,
            }
        }
    }
}

async fn write_tool_result(
    repo_root: &Path,
    tool_name: &str,
    tool_call_id: &str,
    content: &str,
) -> Result<PathBuf> {
    let dir = repo_root.join(RESULT_STORAGE_DIR);
    fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create {}", dir.display()))?;
    let filename = build_result_filename(tool_name, tool_call_id);
    let path = dir.join(filename);
    fs::write(&path, content)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn build_result_filename(tool_name: &str, tool_call_id: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let safe_tool = sanitize_filename(tool_name);
    let safe_call = sanitize_filename(tool_call_id);
    let mut hasher = Sha1::new();
    hasher.update(tool_call_id.as_bytes());
    hasher.update(tool_name.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{timestamp}-{safe_tool}-{safe_call}-{}.txt", &digest[..10])
}

fn sanitize_filename(value: &str) -> String {
    let mut sanitized = value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .take(80)
        .collect::<String>();
    if sanitized.is_empty() {
        sanitized = "tool".to_string();
    }
    sanitized
}

fn generate_preview(content: &str, max_chars: usize) -> String {
    let char_count = content.chars().count();
    if char_count <= max_chars {
        return content.to_string();
    }
    let mut preview = content.chars().take(max_chars).collect::<String>();
    if let Some(index) = preview.rfind('\n')
        && index > max_chars / 2
    {
        preview.truncate(index + 1);
    }
    preview
}

fn build_persisted_message(preview: &str, original_chars: usize, file_path: &str) -> String {
    format!(
        "{PERSISTED_OUTPUT_TAG}\nThis tool result was too large ({original_chars} characters).\nFull output saved to: {file_path}\nUse the read_file tool with a targeted path/limit if you need specific sections of this output.\n\nPreview (first {} chars):\n{}\n...\n{PERSISTED_OUTPUT_CLOSING_TAG}",
        preview.chars().count(),
        preview.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ToolContextBudget, build_result_filename, enforce_tool_turn_budget, generate_preview,
        prepare_tool_result_for_context,
    };
    use crate::infra::llm::types::ChatMessage;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-tool-context-{}-{}",
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
    fn preview_truncates_at_newline_when_possible() {
        let preview = generate_preview("alpha\nbeta\ngamma", 11);
        assert_eq!(preview, "alpha\nbeta\n");
    }

    #[test]
    fn result_filename_is_workspace_safe() {
        let filename = build_result_filename("mcp weird/tool", "call ../evil");
        assert!(!filename.contains('/'));
        assert!(!filename.contains(".."));
        assert!(filename.ends_with(".txt"));
    }

    #[tokio::test]
    async fn persists_large_tool_result_and_returns_context_reference() {
        let repo = TestRepo::new();
        let content = "x".repeat(120);
        let result = prepare_tool_result_for_context(
            &repo.root,
            "mcp_big_tool",
            "call-1",
            content.clone(),
            ToolContextBudget {
                result_size_chars: 50,
                turn_budget_chars: 1_000,
                preview_size_chars: 20,
            },
        )
        .await;

        let path = result.persisted_path.expect("large result persisted");
        assert_eq!(fs::read_to_string(path).unwrap(), content);
        assert!(result.model_content.contains("<persisted-output>"));
        assert!(result.model_content.contains("Full output saved to:"));
        assert!(!result.model_content.contains(&"x".repeat(80)));
    }

    #[tokio::test]
    async fn keeps_read_file_inline_to_avoid_persist_loop() {
        let repo = TestRepo::new();
        let content = "x".repeat(120);
        let result = prepare_tool_result_for_context(
            &repo.root,
            "read_file",
            "call-1",
            content.clone(),
            ToolContextBudget {
                result_size_chars: 50,
                turn_budget_chars: 1_000,
                preview_size_chars: 20,
            },
        )
        .await;

        assert_eq!(result.model_content, content);
        assert!(result.persisted_path.is_none());
    }

    #[tokio::test]
    async fn enforces_turn_budget_by_persisting_largest_inline_result() {
        let repo = TestRepo::new();
        let mut messages = vec![
            ChatMessage::assistant(None, Vec::new()),
            ChatMessage::tool("small", "small"),
            ChatMessage::tool("large", "z".repeat(2_000)),
        ];

        enforce_tool_turn_budget(
            &repo.root,
            &mut messages,
            1,
            ToolContextBudget {
                result_size_chars: 1_000,
                turn_budget_chars: 600,
                preview_size_chars: 12,
            },
        )
        .await;

        assert_eq!(messages[1].content.as_deref(), Some("small"));
        let large = messages[2].content.as_deref().unwrap();
        assert!(large.contains("<persisted-output>"));
        assert!(large.contains("Full output saved to:"));
    }
}

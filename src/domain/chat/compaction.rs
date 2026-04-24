use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::json;

use crate::config::model::AppConfig;
use crate::infra::llm::client::LlmClient;
use crate::infra::llm::types::{ChatCompletionRequest, ChatMessage};

pub const TOKEN_THRESHOLD: usize = 60_000;
const AUTO_COMPACT_INPUT_CHARS: usize = 80_000;
const AUTO_COMPACT_MAX_TOKENS: u32 = 2_000;

pub fn public_compaction_tool_schemas() -> Vec<serde_json::Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "compress",
            "description": "Compress conversation history to free up context window.",
            "parameters": { "type": "object", "properties": {} }
        }
    })]
}

pub fn estimate_tokens(messages: &[ChatMessage]) -> usize {
    serde_json::to_string(messages).unwrap_or_default().len() / 4
}

pub fn microcompact(messages: &mut [ChatMessage]) {
    let tool_indexes = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == "tool")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    if tool_indexes.len() <= 3 {
        return;
    }

    for index in tool_indexes.iter().take(tool_indexes.len() - 3) {
        if let Some(content) = messages[*index].content.as_mut()
            && content.len() > 100
        {
            *content = "[cleared]".to_string();
        }
    }
}

pub async fn auto_compact(
    repo_root: &Path,
    config: &AppConfig,
    llm_client: &LlmClient,
    messages: Vec<ChatMessage>,
) -> Result<Vec<ChatMessage>> {
    if messages.is_empty() {
        return Ok(messages);
    }

    let system = messages[0].clone();
    let tail = messages.into_iter().skip(1).collect::<Vec<_>>();
    let transcript_path = write_transcript(repo_root, &tail).await?;
    let conv_text = truncate_chars(
        serde_json::to_string(&tail).context("failed to encode transcript json")?,
        AUTO_COMPACT_INPUT_CHARS,
    );
    let summary_prompt = format!("Summarize this conversation for continuity:\n{conv_text}");
    let summary_response = llm_client
        .chat(&ChatCompletionRequest {
            model: config.agent.model_id.clone(),
            messages: vec![ChatMessage::user(summary_prompt)],
            tools: None,
            stream: false,
            temperature: None,
            max_tokens: Some(AUTO_COMPACT_MAX_TOKENS),
            top_p: None,
        })
        .await
        .context("failed to generate compaction summary")?;

    let summary = summary_response
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .filter(|content| !content.trim().is_empty())
        .unwrap_or_else(|| "(no summary)".to_string());

    Ok(vec![
        system,
        ChatMessage::user(format!(
            "[Compressed. Transcript: {}]\n{}",
            transcript_path.display(),
            summary
        )),
        ChatMessage::assistant(
            Some("Understood. Continuing with summary context.".to_string()),
            Vec::new(),
        ),
    ])
}

async fn write_transcript(repo_root: &Path, messages: &[ChatMessage]) -> Result<PathBuf> {
    let transcript_dir = repo_root.join(".transcripts");
    tokio::fs::create_dir_all(&transcript_dir)
        .await
        .with_context(|| format!("failed to create {}", transcript_dir.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let path = transcript_dir.join(format!("transcript_{timestamp}.jsonl"));
    let mut lines = String::new();
    for message in messages {
        lines.push_str(
            &serde_json::to_string(message).context("failed to encode transcript message")?,
        );
        lines.push('\n');
    }
    tokio::fs::write(&path, lines)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn truncate_chars(text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::{estimate_tokens, microcompact};
    use crate::infra::llm::types::ChatMessage;

    #[test]
    fn estimates_tokens_from_json_size() {
        let messages = vec![
            ChatMessage::system("system"),
            ChatMessage::user("hello"),
            ChatMessage::assistant(Some("world".to_string()), Vec::new()),
        ];
        assert!(estimate_tokens(&messages) > 0);
    }

    #[test]
    fn clears_old_tool_results_but_keeps_last_three() {
        let mut messages = vec![ChatMessage::system("system")];
        for index in 0..5 {
            messages.push(ChatMessage::tool(format!("tool-{index}"), "x".repeat(150)));
        }

        microcompact(&mut messages);

        assert_eq!(messages[1].content.as_deref(), Some("[cleared]"));
        assert_eq!(messages[2].content.as_deref(), Some("[cleared]"));
        assert_ne!(messages[3].content.as_deref(), Some("[cleared]"));
        assert_ne!(messages[4].content.as_deref(), Some("[cleared]"));
        assert_ne!(messages[5].content.as_deref(), Some("[cleared]"));
    }
}

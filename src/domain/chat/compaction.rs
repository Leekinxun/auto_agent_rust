use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::bail;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::model::AppConfig;
use crate::infra::llm::client::LlmClient;
use crate::infra::llm::types::{ChatCompletionRequest, ChatMessage};

const AUTO_COMPACT_INPUT_CHARS: usize = 80_000;
const AUTO_COMPACT_MAX_TOKENS: u32 = 2_000;
const RECENT_QA_KEEP_PAIRS_AFTER_THRESHOLD: usize = 3;
const QA_PAIR_GRACE_LIMIT: usize = 5;
const TRANSCRIPTS_DIR: &str = ".transcripts";
pub const COMPRESS_CONTEXT_TOOL: &str = "compress_context";
pub const LEGACY_COMPRESS_TOOL: &str = "compress";
pub const CONTEXT_TRANSCRIPT_GET_TOOL: &str = "context_transcript_get";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionOutcome {
    pub messages: Vec<ChatMessage>,
    pub transcript_id: String,
    pub transcript_path: PathBuf,
    pub summary: String,
    pub compacted_message_count: usize,
    pub kept_message_count: usize,
}

pub fn public_compaction_tool_schemas() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": COMPRESS_CONTEXT_TOOL,
                "description": "Compress older conversation context when the current context is too large. The system records this as a visible built-in tool call.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": CONTEXT_TRANSCRIPT_GET_TOOL,
                "description": "Retrieve the full uncompressed JSONL transcript for a previous context compression by transcript_id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "transcript_id": { "type": "string", "description": "Transcript id shown in the compressed context message." },
                        "limit": { "type": "integer", "description": "Optional max number of JSONL lines to return." }
                    },
                    "required": ["transcript_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": LEGACY_COMPRESS_TOOL,
                "description": "Legacy alias for compress_context.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
    ]
}

pub fn estimate_tokens(messages: &[ChatMessage]) -> usize {
    serde_json::to_string(messages).unwrap_or_default().len() / 4
}

#[cfg(test)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionDecision {
    Skip {
        token_estimate: usize,
        qa_pairs: usize,
    },
    CompactAll {
        token_estimate: usize,
        qa_pairs: usize,
    },
    KeepRecentQa {
        token_estimate: usize,
        qa_pairs: usize,
        keep_pairs: usize,
    },
}

impl CompactionDecision {
    pub fn should_compact(&self) -> bool {
        !matches!(self, Self::Skip { .. })
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::Skip { .. } => "below_token_threshold_or_within_qa_grace",
            Self::CompactAll { .. } => "over_token_threshold_without_recent_qa_window",
            Self::KeepRecentQa { .. } => "over_token_threshold_keep_recent_qa",
        }
    }
}

pub fn build_compaction_payload(decision: &CompactionDecision) -> serde_json::Value {
    match decision {
        CompactionDecision::Skip {
            token_estimate,
            qa_pairs,
        } => json!({
            "decision": "skip",
            "reason": decision.reason(),
            "token_estimate": token_estimate,
            "qa_pairs": qa_pairs,
        }),
        CompactionDecision::CompactAll {
            token_estimate,
            qa_pairs,
        } => json!({
            "decision": "compact_all",
            "reason": decision.reason(),
            "token_estimate": token_estimate,
            "qa_pairs": qa_pairs,
        }),
        CompactionDecision::KeepRecentQa {
            token_estimate,
            qa_pairs,
            keep_pairs,
        } => json!({
            "decision": "keep_recent_qa",
            "reason": decision.reason(),
            "token_estimate": token_estimate,
            "qa_pairs": qa_pairs,
            "keep_pairs": keep_pairs,
        }),
    }
}

pub fn decide_compaction(messages: &[ChatMessage], token_threshold: usize) -> CompactionDecision {
    let token_estimate = estimate_tokens(messages);
    let qa_pairs = count_qa_pairs(messages);
    if token_estimate <= token_threshold || qa_pairs <= QA_PAIR_GRACE_LIMIT {
        return CompactionDecision::Skip {
            token_estimate,
            qa_pairs,
        };
    }

    if qa_pairs > RECENT_QA_KEEP_PAIRS_AFTER_THRESHOLD {
        CompactionDecision::KeepRecentQa {
            token_estimate,
            qa_pairs,
            keep_pairs: RECENT_QA_KEEP_PAIRS_AFTER_THRESHOLD,
        }
    } else {
        CompactionDecision::CompactAll {
            token_estimate,
            qa_pairs,
        }
    }
}

pub async fn auto_compact(
    repo_root: &Path,
    config: &AppConfig,
    llm_client: &LlmClient,
    messages: Vec<ChatMessage>,
    keep_recent_qa_pairs: Option<usize>,
) -> Result<CompactionOutcome> {
    if messages.is_empty() {
        return Ok(CompactionOutcome {
            messages,
            transcript_id: String::new(),
            transcript_path: repo_root.join(TRANSCRIPTS_DIR),
            summary: String::new(),
            compacted_message_count: 0,
            kept_message_count: 0,
        });
    }

    let system = messages[0].clone();
    let tail = messages.into_iter().skip(1).collect::<Vec<_>>();
    let (compactable_tail, kept_tail) = split_recent_qa_tail(tail, keep_recent_qa_pairs);
    let compacted_message_count = compactable_tail.len();
    let kept_message_count = kept_tail.len();
    let transcript = write_transcript(repo_root, &compactable_tail).await?;
    let conv_text = truncate_chars(
        serde_json::to_string(&compactable_tail).context("failed to encode transcript json")?,
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
            stream_options: None,
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

    let mut compacted = vec![
        system,
        ChatMessage::user(render_compacted_context_message(
            &transcript.transcript_id,
            &summary,
        )),
        ChatMessage::assistant(
            Some("Understood. Continuing with summary context.".to_string()),
            Vec::new(),
        ),
    ];
    compacted.extend(kept_tail);
    Ok(CompactionOutcome {
        messages: compacted,
        transcript_id: transcript.transcript_id,
        transcript_path: transcript.path,
        summary,
        compacted_message_count,
        kept_message_count,
    })
}

#[derive(Debug, Clone)]
struct WrittenTranscript {
    transcript_id: String,
    path: PathBuf,
}

async fn write_transcript(repo_root: &Path, messages: &[ChatMessage]) -> Result<WrittenTranscript> {
    let transcript_dir = repo_root.join(TRANSCRIPTS_DIR);
    tokio::fs::create_dir_all(&transcript_dir)
        .await
        .with_context(|| format!("failed to create {}", transcript_dir.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let transcript_id = format!("transcript_{timestamp}");
    let path = transcript_dir.join(format!("{transcript_id}.jsonl"));
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
    Ok(WrittenTranscript {
        transcript_id,
        path,
    })
}

pub async fn read_context_transcript(repo_root: &Path, arguments: &Value) -> Result<String> {
    let transcript_id = arguments
        .get("transcript_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("context_transcript_get requires a non-empty transcript_id")?;
    if !is_safe_transcript_id(transcript_id) {
        bail!("invalid transcript_id");
    }

    let limit = optional_line_limit(arguments)?;
    let path = repo_root
        .join(TRANSCRIPTS_DIR)
        .join(format!("{transcript_id}.jsonl"));
    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read transcript {transcript_id}"))?;
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    let truncated = if let Some(limit) = limit
        && limit < lines.len()
    {
        let remaining = lines.len() - limit;
        lines.truncate(limit);
        lines.push(format!("... ({remaining} more)"));
        true
    } else {
        false
    };

    Ok(serde_json::to_string_pretty(&json!({
        "transcript_id": transcript_id,
        "path": path.display().to_string(),
        "truncated": truncated,
        "content_jsonl": lines.join("\n"),
    }))?)
}

fn is_safe_transcript_id(value: &str) -> bool {
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn optional_line_limit(arguments: &Value) -> Result<Option<usize>> {
    let Some(limit) = arguments.get("limit") else {
        return Ok(None);
    };
    if limit.is_null() {
        return Ok(None);
    }
    let raw = limit
        .as_i64()
        .context("context_transcript_get limit must be an integer")?;
    if raw <= 0 {
        return Ok(None);
    }
    usize::try_from(raw)
        .map(Some)
        .context("context_transcript_get limit is too large")
}

fn truncate_chars(text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    text.chars().take(limit).collect()
}

fn render_compacted_context_message(transcript_id: &str, summary: &str) -> String {
    format!(
        "[Compressed context]\nSummary:\n{summary}\n\nFull uncompressed context transcript_id: {transcript_id}\nIf this summary is not detailed enough, call the built-in {CONTEXT_TRANSCRIPT_GET_TOOL} tool with this transcript_id to inspect the complete JSONL transcript before continuing."
    )
}

fn count_qa_pairs(messages: &[ChatMessage]) -> usize {
    let mut pending_user = false;
    let mut pairs = 0usize;
    for message in messages {
        match message.role.as_str() {
            "user" => pending_user = true,
            "assistant"
                if pending_user && message.tool_calls.as_ref().is_none_or(Vec::is_empty) =>
            {
                pairs = pairs.saturating_add(1);
                pending_user = false;
            }
            _ => {}
        }
    }
    pairs
}

fn split_recent_qa_tail(
    tail: Vec<ChatMessage>,
    keep_recent_qa_pairs: Option<usize>,
) -> (Vec<ChatMessage>, Vec<ChatMessage>) {
    let Some(keep_pairs) = keep_recent_qa_pairs.filter(|value| *value > 0) else {
        return (tail, Vec::new());
    };
    let mut pairs_seen = 0usize;
    let mut start_index = tail.len();
    let mut awaiting_user = false;
    for (index, message) in tail.iter().enumerate().rev() {
        match message.role.as_str() {
            "assistant" if message.tool_calls.as_ref().is_none_or(Vec::is_empty) => {
                awaiting_user = true;
            }
            "user" if awaiting_user => {
                pairs_seen = pairs_seen.saturating_add(1);
                start_index = index;
                awaiting_user = false;
                if pairs_seen >= keep_pairs {
                    break;
                }
            }
            _ => {}
        }
    }
    if pairs_seen == 0 {
        return (tail, Vec::new());
    }
    let compactable = tail[..start_index].to_vec();
    let kept = tail[start_index..].to_vec();
    (compactable, kept)
}

#[cfg(test)]
mod tests {
    use super::{
        CONTEXT_TRANSCRIPT_GET_TOOL, CompactionDecision, decide_compaction, estimate_tokens,
        microcompact, render_compacted_context_message, split_recent_qa_tail,
    };
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

    #[test]
    fn skips_compaction_below_threshold_even_with_many_qa_pairs() {
        let messages = qa_messages(6, "short");
        assert_eq!(
            decide_compaction(&messages, 80_000),
            CompactionDecision::Skip {
                token_estimate: estimate_tokens(&messages),
                qa_pairs: 6
            }
        );
    }

    #[test]
    fn skips_compaction_within_five_qa_pairs_even_when_large() {
        let messages = qa_messages(5, &"x".repeat(80_000));
        assert_eq!(
            decide_compaction(&messages, 1),
            CompactionDecision::Skip {
                token_estimate: estimate_tokens(&messages),
                qa_pairs: 5
            }
        );
    }

    #[test]
    fn keeps_recent_three_qa_pairs_when_threshold_is_exceeded_after_grace() {
        let messages = qa_messages(6, &"x".repeat(1000));
        assert_eq!(
            decide_compaction(&messages, 1),
            CompactionDecision::KeepRecentQa {
                token_estimate: estimate_tokens(&messages),
                qa_pairs: 6,
                keep_pairs: 3
            }
        );
    }

    #[test]
    fn recent_qa_split_ignores_tool_messages_for_pair_counting() {
        let mut tail = Vec::new();
        for index in 0..6 {
            tail.push(ChatMessage::user(format!("u{index}")));
            tail.push(ChatMessage::assistant(
                Some(format!("a{index}")),
                Vec::new(),
            ));
            tail.push(ChatMessage::tool(format!("tool-{index}"), "tool-output"));
        }

        let (_compactable, kept) = split_recent_qa_tail(tail, Some(3));

        assert_eq!(
            kept.iter().filter(|message| message.role == "user").count(),
            3
        );
        assert_eq!(
            kept.iter()
                .filter(|message| message.role == "assistant")
                .count(),
            3
        );
    }

    #[test]
    fn compacted_context_message_points_agent_to_full_transcript() {
        let message = render_compacted_context_message("transcript_1", "short summary");

        assert!(message.contains("transcript_id: transcript_1"));
        assert!(message.contains(CONTEXT_TRANSCRIPT_GET_TOOL));
        assert!(!message.contains("read_file"));
    }

    fn qa_messages(pairs: usize, content: &str) -> Vec<ChatMessage> {
        let mut messages = vec![ChatMessage::system("system")];
        for index in 0..pairs {
            messages.push(ChatMessage::user(format!("{content}-u{index}")));
            messages.push(ChatMessage::assistant(
                Some(format!("{content}-a{index}")),
                Vec::new(),
            ));
        }
        messages
    }
}

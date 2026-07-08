use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HitlDecisionKind {
    ToolCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HitlRiskLevel {
    Low,
    Medium,
    High,
}

impl HitlRiskLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HitlDecisionStatus {
    Pending,
    Approved,
    Rejected,
    Modified,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlDecisionRequest {
    pub approval_id: String,
    pub session_id: String,
    pub generation: u64,
    pub kind: HitlDecisionKind,
    pub title: String,
    pub summary: String,
    pub risk_level: HitlRiskLevel,
    pub tool_name: Option<String>,
    pub display_name: Option<String>,
    pub arguments: Value,
    pub preview: Option<String>,
    pub created_at_ms: u128,
    pub expires_at_ms: Option<u128>,
    pub status: HitlDecisionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlDecisionResolution {
    pub approval_id: String,
    pub status: HitlDecisionStatus,
    pub resolved_at_ms: u128,
    pub resolved_by: String,
    pub note: Option<String>,
    pub modified_arguments: Option<Value>,
}

impl HitlDecisionRequest {
    pub fn new_tool_call(
        session_id: &str,
        generation: u64,
        tool_name: &str,
        arguments: Value,
        risk_level: HitlRiskLevel,
        timeout_seconds: u64,
        display_name: Option<String>,
    ) -> Result<Self> {
        let tool_name = tool_name.trim();
        ensure!(!tool_name.is_empty(), "tool_name cannot be empty");
        let now = now_ms();
        let display_name = display_name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let action_name = display_name.as_deref().unwrap_or(tool_name);
        Ok(Self {
            approval_id: new_hitl_approval_id(),
            session_id: session_id.to_string(),
            generation,
            kind: HitlDecisionKind::ToolCall,
            title: format!("确认执行：{action_name}"),
            summary: format!("模型请求执行“{action_name}”，需要人工确认后继续。"),
            risk_level,
            tool_name: Some(tool_name.to_string()),
            display_name,
            preview: Some(preview_json(&arguments, 2_000)),
            arguments,
            created_at_ms: now,
            expires_at_ms: if timeout_seconds == 0 {
                None
            } else {
                Some(now.saturating_add(u128::from(timeout_seconds) * 1_000))
            },
            status: HitlDecisionStatus::Pending,
        })
    }
}

impl HitlDecisionResolution {
    pub fn approved(approval_id: String, resolved_by: String, note: Option<String>) -> Self {
        Self {
            approval_id,
            status: HitlDecisionStatus::Approved,
            resolved_at_ms: now_ms(),
            resolved_by,
            note,
            modified_arguments: None,
        }
    }

    pub fn rejected(approval_id: String, resolved_by: String, note: Option<String>) -> Self {
        Self {
            approval_id,
            status: HitlDecisionStatus::Rejected,
            resolved_at_ms: now_ms(),
            resolved_by,
            note,
            modified_arguments: None,
        }
    }

    pub fn modified(
        approval_id: String,
        resolved_by: String,
        note: Option<String>,
        arguments: Value,
    ) -> Self {
        Self {
            approval_id,
            status: HitlDecisionStatus::Modified,
            resolved_at_ms: now_ms(),
            resolved_by,
            note,
            modified_arguments: Some(arguments),
        }
    }

    pub fn expired(approval_id: String) -> Self {
        Self {
            approval_id,
            status: HitlDecisionStatus::Expired,
            resolved_at_ms: now_ms(),
            resolved_by: "system-timeout".to_string(),
            note: Some("HITL approval timed out".to_string()),
            modified_arguments: None,
        }
    }
}

pub fn new_hitl_approval_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("hitl-{nanos}")
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn preview_json(value: &Value, limit: usize) -> String {
    let raw = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    if raw.chars().count() <= limit {
        raw
    } else {
        format!(
            "{}...[truncated]",
            raw.chars().take(limit).collect::<String>().trim_end()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{HitlDecisionRequest, HitlDecisionStatus, HitlRiskLevel};
    use serde_json::json;

    #[test]
    fn creates_pending_tool_call_request() {
        let request = HitlDecisionRequest::new_tool_call(
            "session-a",
            7,
            "write_file",
            json!({"path":"a.md"}),
            HitlRiskLevel::High,
            60,
            Some("写入文件".to_string()),
        )
        .unwrap();

        assert!(request.approval_id.starts_with("hitl-"));
        assert_eq!(request.session_id, "session-a");
        assert_eq!(request.generation, 7);
        assert_eq!(request.tool_name.as_deref(), Some("write_file"));
        assert_eq!(request.display_name.as_deref(), Some("写入文件"));
        assert_eq!(request.title, "确认执行：写入文件");
        assert_eq!(request.status, HitlDecisionStatus::Pending);
        assert!(request.expires_at_ms.is_some());
    }
}

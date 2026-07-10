use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use serde_json::{Value, json};

use crate::domain::chat::compaction::public_compaction_tool_schemas;
use crate::domain::session::service::SessionService;
use crate::domain::tasks::service::public_task_tool_schemas;
use crate::domain::worktree::service::public_worktree_tool_schemas;
use crate::infra::fs::tool_ops::public_file_tool_schemas;

#[derive(Debug, Clone, Serialize)]
pub struct BuiltinToolPreview {
    pub name: String,
    pub label: String,
    pub description: String,
}

pub fn builtin_tool_schemas(include_session_tools: bool) -> Vec<Value> {
    let mut tools = public_file_tool_schemas();
    tools.extend(public_compaction_tool_schemas());
    tools.extend(public_task_tool_schemas());
    tools.extend(public_worktree_tool_schemas());
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "task",
            "description": "Spawn a subagent for isolated exploration or work. Returns a summary.",
            "parameters": {
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "agent_type": { "type": "string", "enum": ["Explore", "general-purpose"] }
                },
                "required": ["prompt"]
            }
        }
    }));
    if include_session_tools {
        tools.extend(SessionService::session_tool_schemas());
    }
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "load_skill",
            "description": "Load the full content of a named skill when the task requires it.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The skill name to load." }
                },
                "required": ["name"]
            }
        }
    }));
    tools
}

pub fn builtin_tool_previews() -> Vec<BuiltinToolPreview> {
    let mut by_name = BTreeMap::new();
    for schema in builtin_tool_schemas(true) {
        let Some(function) = schema.get("function") else {
            continue;
        };
        let Some(name) = function
            .get("name")
            .and_then(Value::as_str)
            .map(normalize_builtin_tool_name)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let description = function
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        by_name
            .entry(name.to_string())
            .or_insert(BuiltinToolPreview {
                name: name.to_string(),
                label: name.to_string(),
                description,
            });
    }
    by_name.into_values().collect()
}

pub fn builtin_tool_names() -> Vec<String> {
    builtin_tool_previews()
        .into_iter()
        .map(|tool| tool.name)
        .collect()
}

pub fn is_builtin_tool(name: &str) -> bool {
    let normalized = normalize_builtin_tool_name(name);
    !normalized.is_empty()
        && builtin_tool_previews()
            .iter()
            .any(|tool| tool.name == normalized)
}

pub fn normalize_builtin_tool_list(values: Vec<String>) -> Vec<String> {
    let known = builtin_tool_names().into_iter().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|item| normalize_builtin_tool_name(&item).to_string())
        .filter(|item| !item.is_empty())
        .filter(|item| known.contains(item))
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

pub fn normalize_builtin_tool_name(name: &str) -> &str {
    name.trim()
}

#[cfg(test)]
mod tests {
    use super::{builtin_tool_names, is_builtin_tool};

    #[test]
    fn registry_discovers_builtin_tools_from_schemas() {
        let names = builtin_tool_names();
        assert!(names.iter().any(|name| name == "read_file"));
        assert!(names.iter().any(|name| name == "compress_context"));
        assert!(names.iter().any(|name| name == "context_transcript_get"));
        assert!(is_builtin_tool("compress_context"));
    }
}

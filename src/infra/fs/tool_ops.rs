use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const MAX_READ_OUTPUT_CHARS: usize = 50_000;

pub fn public_file_tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read file contents from the current workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Workspace-relative or in-workspace absolute path." },
                        "limit": { "type": "integer", "description": "Optional max number of lines to return." }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write full file contents inside the current workspace, creating parent folders if needed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Workspace-relative or in-workspace absolute path." },
                        "content": { "type": "string", "description": "Complete file content to write." }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace the first exact text match in a file inside the current workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Workspace-relative or in-workspace absolute path." },
                        "old_text": { "type": "string", "description": "Exact existing text to replace once." },
                        "new_text": { "type": "string", "description": "Replacement text." }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        }),
    ]
}

pub fn dispatch_public_file_tool(
    workspace_root: &Path,
    name: &str,
    arguments: &Value,
) -> Option<String> {
    match name {
        "read_file" => Some(format_tool_result(run_read(
            workspace_root,
            required_path_arg(arguments, "read_file"),
            optional_limit(arguments),
        ))),
        "write_file" => Some(format_tool_result(run_write(
            workspace_root,
            required_path_arg(arguments, "write_file"),
            required_string_arg(arguments, "content", "write_file"),
        ))),
        "edit_file" => Some(format_tool_result(run_edit(
            workspace_root,
            required_path_arg(arguments, "edit_file"),
            required_string_arg(arguments, "old_text", "edit_file"),
            required_string_arg(arguments, "new_text", "edit_file"),
        ))),
        _ => None,
    }
}

pub fn safe_workspace_path(workspace_root: &Path, raw_path: &str) -> Result<PathBuf> {
    let candidate = raw_path.trim();
    if candidate.is_empty() {
        bail!("path cannot be empty");
    }

    let workspace_root = workspace_root.canonicalize().with_context(|| {
        format!(
            "failed to resolve workspace root {}",
            workspace_root.display()
        )
    })?;
    let joined = if Path::new(candidate).is_absolute() {
        PathBuf::from(candidate)
    } else {
        workspace_root.join(candidate)
    };
    let normalized = normalize_path(&joined);
    let resolved = resolve_existing_ancestor(&normalized)?;

    if !resolved.starts_with(&workspace_root) {
        bail!("Path escapes workspace: {candidate}");
    }

    Ok(resolved)
}

fn run_read(
    workspace_root: &Path,
    path: Result<&str>,
    limit: Result<Option<usize>>,
) -> Result<String> {
    let path = path?;
    let limit = limit?;
    let file_path = safe_workspace_path(workspace_root, path)?;
    let content = fs::read_to_string(&file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    if let Some(limit) = limit
        && limit < lines.len()
    {
        let remaining = lines.len() - limit;
        lines.truncate(limit);
        lines.push(format!("... ({remaining} more)"));
    }
    Ok(truncate_chars(&lines.join("\n"), MAX_READ_OUTPUT_CHARS))
}

fn run_write(workspace_root: &Path, path: Result<&str>, content: Result<&str>) -> Result<String> {
    let path = path?;
    let content = content?;
    let file_path = safe_workspace_path(workspace_root, path)?;
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directories for {}", path))?;
    }
    fs::write(&file_path, content)
        .with_context(|| format!("failed to write {}", file_path.display()))?;
    Ok(format!("Wrote {} bytes", content.len()))
}

fn run_edit(
    workspace_root: &Path,
    path: Result<&str>,
    old_text: Result<&str>,
    new_text: Result<&str>,
) -> Result<String> {
    let path = path?;
    let old_text = old_text?;
    let new_text = new_text?;
    let file_path = safe_workspace_path(workspace_root, path)?;
    let current = fs::read_to_string(&file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;
    if !current.contains(old_text) {
        return Ok(format!("Error: Text not found in {path}"));
    }
    let updated = current.replacen(old_text, new_text, 1);
    fs::write(&file_path, updated)
        .with_context(|| format!("failed to write {}", file_path.display()))?;
    Ok(format!("Edited {path}"))
}

fn required_path_arg<'a>(arguments: &'a Value, tool_name: &str) -> Result<&'a str> {
    arguments
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{tool_name} requires a non-empty path"))
}

fn required_string_arg<'a>(arguments: &'a Value, key: &str, tool_name: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{tool_name} requires a string {key}"))
}

fn optional_limit(arguments: &Value) -> Result<Option<usize>> {
    let Some(limit) = arguments.get("limit") else {
        return Ok(None);
    };
    if limit.is_null() {
        return Ok(None);
    }
    let raw = limit
        .as_i64()
        .context("read_file limit must be an integer")?;
    if raw <= 0 {
        return Ok(None);
    }
    usize::try_from(raw)
        .map(Some)
        .context("read_file limit is too large")
}

fn format_tool_result(result: Result<String>) -> String {
    result.unwrap_or_else(|error| format!("Error: {error}"))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

fn resolve_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut missing = Vec::<OsString>::new();
    let mut current = path;

    while !current.exists() {
        let Some(name) = current.file_name() else {
            bail!("path has no existing ancestor: {}", path.display());
        };
        missing.push(name.to_os_string());
        current = current
            .parent()
            .with_context(|| format!("path has no parent: {}", path.display()))?;
    }

    let mut resolved = current
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", current.display()))?;
    for segment in missing.into_iter().rev() {
        resolved.push(segment);
    }
    Ok(resolved)
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::{dispatch_public_file_tool, safe_workspace_path};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestWorkspace {
        path: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-code-rs-tool-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time before unix epoch")
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create temp workspace");
            Self { path }
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn allows_nested_relative_paths_inside_workspace() {
        let workspace = TestWorkspace::new();
        let resolved = safe_workspace_path(&workspace.path, "nested/example.txt").unwrap();
        let expected_root = workspace.path.canonicalize().unwrap();
        assert_eq!(resolved, expected_root.join("nested/example.txt"));
    }

    #[test]
    fn rejects_parent_dir_escape() {
        let workspace = TestWorkspace::new();
        let error = safe_workspace_path(&workspace.path, "../outside.txt")
            .unwrap_err()
            .to_string();
        assert!(error.contains("Path escapes workspace"));
    }

    #[test]
    fn rejects_absolute_escape() {
        let workspace = TestWorkspace::new();
        let error = safe_workspace_path(&workspace.path, "/tmp/outside.txt")
            .unwrap_err()
            .to_string();
        assert!(error.contains("Path escapes workspace"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let workspace = TestWorkspace::new();
        let outside = TestWorkspace::new();
        let link = workspace.path.join("link");
        symlink(&outside.path, &link).expect("create symlink");

        let error = safe_workspace_path(&workspace.path, "link/secret.txt")
            .unwrap_err()
            .to_string();
        assert!(error.contains("Path escapes workspace"));
    }

    #[test]
    fn file_tools_can_write_read_and_edit_in_workspace() {
        let workspace = TestWorkspace::new();

        let write_result = dispatch_public_file_tool(
            &workspace.path,
            "write_file",
            &json!({ "path": "notes/demo.txt", "content": "hello\nworld" }),
        )
        .unwrap();
        assert_eq!(write_result, "Wrote 11 bytes");

        let read_result = dispatch_public_file_tool(
            &workspace.path,
            "read_file",
            &json!({ "path": "notes/demo.txt", "limit": 1 }),
        )
        .unwrap();
        assert_eq!(read_result, "hello\n... (1 more)");

        let edit_result = dispatch_public_file_tool(
            &workspace.path,
            "edit_file",
            &json!({ "path": "notes/demo.txt", "old_text": "world", "new_text": "rust" }),
        )
        .unwrap();
        assert_eq!(edit_result, "Edited notes/demo.txt");

        let full_read = dispatch_public_file_tool(
            &workspace.path,
            "read_file",
            &json!({ "path": "notes/demo.txt" }),
        )
        .unwrap();
        assert_eq!(full_read, "hello\nrust");
    }
}

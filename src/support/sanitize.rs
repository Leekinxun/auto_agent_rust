use anyhow::{Result, bail};
use sha1::{Digest, Sha1};

pub fn safe_user_dir_name(user_id: &str) -> String {
    let trimmed = user_id.trim();
    let base = trimmed
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches(|ch| ch == '.' || ch == '_')
        .to_string();

    if !base.is_empty() && base == trimmed && base.len() <= 80 {
        return base;
    }

    let fallback = if base.is_empty() { "user" } else { &base };
    let digest = format!("{:x}", Sha1::digest(trimmed.as_bytes()));
    format!(
        "{}--{}",
        fallback.chars().take(80).collect::<String>(),
        &digest[..8]
    )
}

pub fn sanitize_skill_folder(folder: &str) -> Result<String> {
    let candidate = folder.trim();
    if candidate.is_empty() {
        bail!("Skill 文件夹名不能为空");
    }
    if candidate.contains('/') || candidate.contains('\\') {
        bail!("Skill 文件夹名不能包含路径分隔符");
    }

    let normalized = candidate
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .trim_matches(|ch: char| ch == ' ' || ch == '.')
        .to_string();

    if normalized.is_empty() || matches!(normalized.as_str(), "." | "..") {
        bail!("Skill 文件夹名无效");
    }

    if normalized
        .chars()
        .any(|ch| matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|') || ch.is_control())
    {
        bail!("Skill 文件夹名包含非法字符");
    }

    Ok(normalized)
}

use std::collections::BTreeMap;

pub fn parse_frontmatter(raw: &str) -> (BTreeMap<String, String>, String) {
    let normalized = raw.replace("\r\n", "\n");
    let Some(rest) = normalized.strip_prefix("---\n") else {
        return (BTreeMap::new(), normalized.trim().to_string());
    };

    let Some((frontmatter, body)) = rest.split_once("\n---\n") else {
        return (BTreeMap::new(), normalized.trim().to_string());
    };

    let mut meta = BTreeMap::new();
    for line in frontmatter.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            if !key.is_empty() && !value.is_empty() {
                meta.insert(key.to_string(), value.to_string());
            }
        }
    }

    (meta, body.trim().to_string())
}

pub fn render_frontmatter(meta: &BTreeMap<String, String>, body: &str) -> String {
    let mut lines = vec!["---".to_string()];
    for (key, value) in meta {
        if !value.trim().is_empty() {
            lines.push(format!("{key}: {}", value.trim()));
        }
    }
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(body.trim().to_string());
    lines.push(String::new());
    lines.join("\n")
}

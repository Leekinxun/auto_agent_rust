use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

#[derive(Clone)]
pub struct MessageBus {
    inbox_dir: PathBuf,
}

impl MessageBus {
    pub fn new(team_dir: PathBuf) -> Result<Self> {
        let inbox_dir = team_dir.join("inbox");
        fs::create_dir_all(&inbox_dir)
            .with_context(|| format!("failed to create {}", inbox_dir.display()))?;
        Ok(Self { inbox_dir })
    }

    pub fn send(
        &self,
        sender: &str,
        to: &str,
        content: &str,
        msg_type: &str,
        extra: Option<Map<String, Value>>,
    ) -> Result<String> {
        let mut payload = json!({
            "type": msg_type,
            "from": sender,
            "content": content,
            "timestamp": now_secs_f64(),
        });
        if let Some(extra) = extra
            && let Some(object) = payload.as_object_mut()
        {
            for (key, value) in extra {
                object.insert(key, value);
            }
        }

        fs::create_dir_all(&self.inbox_dir)
            .with_context(|| format!("failed to create {}", self.inbox_dir.display()))?;
        let path = self.inbox_dir.join(format!("{to}.jsonl"));
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&payload).context("failed to encode inbox message")?
        )
        .with_context(|| format!("failed to append {}", path.display()))?;
        Ok(format!("Sent {msg_type} to {to}"))
    }

    pub fn ensure_inbox(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        let path = self.inbox_dir.join(format!("{name}.jsonl"));
        if !path.exists() {
            fs::write(&path, "").with_context(|| format!("failed to create {}", path.display()))?;
        }
        Ok(())
    }

    pub fn read_inbox(&self, name: &str) -> Result<Vec<Value>> {
        let path = self.inbox_dir.join(format!("{name}.jsonl"));
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        fs::write(&path, "").with_context(|| format!("failed to clear {}", path.display()))?;
        let mut messages = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            messages.push(
                serde_json::from_str::<Value>(line).with_context(|| {
                    format!("failed to parse inbox message in {}", path.display())
                })?,
            );
        }
        Ok(messages)
    }

    pub fn broadcast(&self, sender: &str, content: &str) -> Result<String> {
        let mut count = 0usize;
        for entry in fs::read_dir(&self.inbox_dir)
            .with_context(|| format!("failed to read {}", self.inbox_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if name == sender {
                continue;
            }
            self.send(sender, name, content, "broadcast", None)?;
            count += 1;
        }
        Ok(format!("Broadcast to {count} teammates"))
    }
}

fn now_secs_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::MessageBus;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-bus-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time before unix epoch")
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn sends_and_reads_messages() {
        let dir = TestDir::new();
        let bus = MessageBus::new(dir.path.clone()).unwrap();
        bus.send("lead", "lead", "hello", "message", None).unwrap();
        let items = bus.read_inbox("lead").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["content"], "hello");
        assert!(bus.read_inbox("lead").unwrap().is_empty());
    }

    #[test]
    fn can_create_empty_inbox_file() {
        let dir = TestDir::new();
        let bus = MessageBus::new(dir.path.clone()).unwrap();
        bus.ensure_inbox("alice").unwrap();
        assert!(dir.path.join("inbox/alice.jsonl").exists());
    }
}

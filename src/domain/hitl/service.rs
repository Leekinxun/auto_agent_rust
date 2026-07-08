use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::models::{HitlDecisionRequest, HitlDecisionResolution};

const HITL_APPROVAL_DIR: &str = ".omx/approvals/hitl";

pub async fn write_hitl_request(
    repo_root: &Path,
    request: &HitlDecisionRequest,
) -> Result<PathBuf> {
    let dir = repo_root.join(HITL_APPROVAL_DIR);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{}.request.json", request.approval_id));
    let encoded = serde_json::to_string_pretty(request).context("failed to encode HITL request")?;
    tokio::fs::write(&path, encoded)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub async fn write_hitl_resolution(
    repo_root: &Path,
    resolution: &HitlDecisionResolution,
) -> Result<PathBuf> {
    let dir = repo_root.join(HITL_APPROVAL_DIR);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{}.resolution.json", resolution.approval_id));
    let encoded =
        serde_json::to_string_pretty(resolution).context("failed to encode HITL resolution")?;
    tokio::fs::write(&path, encoded)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{write_hitl_request, write_hitl_resolution};
    use crate::domain::hitl::models::{HitlDecisionRequest, HitlDecisionResolution, HitlRiskLevel};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-hitl-{}-{}",
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

    #[tokio::test]
    async fn writes_request_and_resolution() {
        let repo = TestRepo::new();
        let request = HitlDecisionRequest::new_tool_call(
            "s",
            1,
            "write_file",
            json!({}),
            HitlRiskLevel::High,
            10,
            Some("写入文件".to_string()),
        )
        .unwrap();
        let request_path = write_hitl_request(&repo.root, &request).await.unwrap();
        assert!(request_path.exists());

        let resolution = HitlDecisionResolution::approved(
            request.approval_id.clone(),
            "operator".to_string(),
            None,
        );
        let resolution_path = write_hitl_resolution(&repo.root, &resolution)
            .await
            .unwrap();
        assert!(resolution_path.exists());
    }
}

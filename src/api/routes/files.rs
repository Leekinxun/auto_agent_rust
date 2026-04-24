use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;

use crate::api::errors::{ApiError, ApiResult};
use crate::app_state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new().route("/download/{*file_path}", get(download_file))
}

async fn download_file(
    State(state): State<SharedState>,
    AxumPath(file_path): AxumPath<String>,
) -> ApiResult<Response> {
    let resolved = resolve_download_target(&state.repo_root, &file_path)
        .ok_or_else(|| ApiError::not_found(format!("文件不存在: {file_path}")))?;
    let bytes = tokio::fs::read(&resolved)
        .await
        .map_err(anyhow::Error::from)?;

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    let filename = resolved
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download.bin");
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    Ok(response)
}

fn resolve_download_target(repo_root: &Path, file_path: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(file_path);
    if candidate.is_absolute() && candidate.is_file() {
        return Some(candidate);
    }

    #[cfg(unix)]
    if !candidate.is_absolute() {
        let unix_absolute = PathBuf::from(format!("/{file_path}"));
        if unix_absolute.is_file() {
            return Some(unix_absolute);
        }
    }

    for dir in [
        repo_root.join("uploads"),
        repo_root.join("outputs"),
        repo_root.to_path_buf(),
    ] {
        let target = dir.join(file_path);
        if target.is_file() {
            return Some(target);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::resolve_download_target;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-download-{}-{}",
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
    fn resolves_relative_downloads_from_outputs() {
        let repo = TestRepo::new();
        let outputs = repo.root.join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        let file = outputs.join("sample.md");
        fs::write(&file, "hello").unwrap();

        let resolved = resolve_download_target(&repo.root, "sample.md").unwrap();
        assert_eq!(resolved, file);
    }

    #[cfg(unix)]
    #[test]
    fn resolves_route_safe_absolute_paths_without_leading_slash() {
        let repo = TestRepo::new();
        let outputs = repo.root.join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        let file = outputs.join("sample.md");
        fs::write(&file, "hello").unwrap();

        let route_safe = file
            .display()
            .to_string()
            .trim_start_matches('/')
            .to_string();
        let resolved = resolve_download_target(&repo.root, &route_safe).unwrap();
        assert_eq!(resolved, file);
    }
}

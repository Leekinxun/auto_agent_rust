use std::path::Path;

use axum::extract::{OriginalUri, State};
use axum::response::Html;

use crate::api::errors::{ApiError, ApiResult};
use crate::app_state::SharedState;

pub fn is_frontend_route(path: &str) -> bool {
    !(path.starts_with("agent")
        || path.starts_with("health")
        || path.starts_with("static")
        || path.starts_with("docs")
        || path.starts_with("redoc")
        || path.starts_with("openapi")
        || Path::new(path).extension().is_some())
}

pub async fn index(State(state): State<SharedState>) -> ApiResult<Html<String>> {
    serve_index(state).await
}

pub async fn fallback(
    State(state): State<SharedState>,
    OriginalUri(uri): OriginalUri,
) -> ApiResult<Html<String>> {
    let path = uri.path().trim_start_matches('/');
    if !is_frontend_route(path) {
        return Err(ApiError::not_found(format!("路径不存在: {}", uri.path())));
    }
    serve_index(state).await
}

async fn serve_index(state: SharedState) -> ApiResult<Html<String>> {
    let index_path = state
        .repo_root
        .join("static")
        .join("frontend")
        .join("index.html");
    if !index_path.exists() {
        return Err(ApiError::service_unavailable(
            "前端构建产物不存在，请先执行 frontend 目录下的 npm run build",
        ));
    }
    let html = tokio::fs::read_to_string(&index_path)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Html(html))
}

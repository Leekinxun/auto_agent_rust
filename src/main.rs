mod api;
mod app_state;
mod config;
mod domain;
mod infra;
mod support;

use anyhow::Context;
use tokio::net::TcpListener;

use crate::api::build_router;
use crate::app_state::AppState;
use crate::config::loader::{find_repo_root, load_config, load_repo_dotenv};
use crate::support::logging::init_tracing;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let repo_root = find_repo_root().context("failed to locate repository root")?;
    let dotenv_path = load_repo_dotenv(&repo_root).context("failed to load repository .env")?;
    let config = load_config(&repo_root).context("failed to load config/config.yaml")?;
    init_tracing(&repo_root, &config.logging).context("failed to initialize tracing")?;

    if let Some(path) = dotenv_path.as_ref() {
        tracing::info!(dotenv = %path.display(), "loaded .env for local startup");
    }

    let state = AppState::new(repo_root.clone(), config.clone())?;
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);

    tracing::info!(
        bind_addr,
        repo_root = %repo_root.display(),
        model = %config.agent.model_id,
        "starting rust backend"
    );

    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {bind_addr}"))?;

    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum server failed")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};

        if let Ok(mut stream) = signal(SignalKind::terminate()) {
            let _ = stream.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}

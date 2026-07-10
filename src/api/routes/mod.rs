pub mod chat;
pub mod files;
pub mod frontend;
pub mod harness;
pub mod health;
pub mod memory;
pub mod runs;
pub mod skills;
pub mod tasks;
pub mod worktrees;

use axum::Router;

use crate::app_state::SharedState;

pub fn agent_router() -> Router<SharedState> {
    Router::new()
        .merge(chat::router())
        .merge(files::router())
        .merge(harness::router())
        .merge(memory::router())
        .merge(runs::router())
        .merge(skills::router())
        .merge(tasks::router())
        .merge(worktrees::router())
        .merge(worktrees::events_router())
}

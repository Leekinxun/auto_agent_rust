pub mod chat;
pub mod files;
pub mod frontend;
pub mod health;
pub mod memory;
pub mod skills;
pub mod tasks;
pub mod worktrees;

use axum::Router;

use crate::app_state::SharedState;

pub fn agent_router() -> Router<SharedState> {
    Router::new()
        .merge(chat::router())
        .merge(files::router())
        .merge(memory::router())
        .merge(skills::router())
        .merge(tasks::router())
        .merge(worktrees::router())
        .merge(worktrees::events_router())
}

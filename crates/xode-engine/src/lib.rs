//! Xode engine: the single entry point used by both the desktop app and the CLI.
//!
//! PUBLIC API CONTRACT (frontends depend on these signatures):
//! - `Engine::new() -> Result<Arc<Engine>>`
//! - `subscribe() -> broadcast::Receiver<AgentEvent>`
//! - config: `config() -> Config`, `set_config(Config)`
//! - projects: `projects()`, `add_project(root)`, `update_project(Project)`, `remove_project(id)`
//! - sessions: `sessions(project_id: Option<&str>)`, `new_session(project_id)`, `session(id)`,
//!   `rename_session(id, title)`, `delete_session(id)`, `messages(id)`
//! - run: `send(session_id, text, attachments)` (queues while running), `steer(session_id)`,
//!   `unqueue(session_id, id)`, `queued(session_id)`, `cancel(session_id)`, `is_running(session_id)`,
//!   `command(session_id, line) -> CommandResult`, `commands(project_id) -> Vec<CommandInfo>`
//! - `set_mode(session_id, Mode)`, `set_model(session_id, gateway_id, model)`
//! - `context_view(session_id) -> ContextView`, `stats(session_id) -> LiveStats`
//! - `permission_reply(req_id, PermDecision)`
//! - gateways: `detect_gateway(url, api_key) -> Gateway`, `test_gateway(Gateway) -> GatewayTest`,
//!   `scan_gateways(extra_hosts: Vec<String>) -> Vec<Gateway>`, `refresh_models(gateway_id)`
//! - fs: `list_dir(path) -> Vec<FileEntry>`, `complete_path(project_id, prefix) -> Vec<String>`
//! - `browser_test() -> GatewayTest`, `mcp_test(McpServer) -> Result<Vec<String>>`
//! - knowledge base: `kb_overview(project_id)`, `kb_add_source(project_id, layer, path, name)`,
//!   `kb_remove_source`, `kb_update_source`, `kb_reindex`, `kb_search(project_id, KbSearchReq)`,
//!   `kb_note`, `kb_list`, `kb_save_note`, `kb_create_note`, `kb_delete_note`, `kb_graph`,
//!   `kb_titles`, `kb_embed_retry()`, `set_session_kb(session_id, off)`
pub mod api;
mod commands;
mod engine;
mod import;
mod kb;

pub use api::*;
pub use engine::Engine;
pub use import::{ImportResult, ImportSource};
pub use kb::{KbFolder, KbNoteRow, KbOverview, KbSearchReq};
pub use xode_kb;
pub use xode_core;

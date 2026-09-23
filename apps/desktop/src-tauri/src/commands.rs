//! One Tauri command per `xode_engine::Engine` method. Errors are mapped to strings.
use std::sync::Arc;
use tauri::State;
use xode_engine::xode_core::config::{Gateway, McpServer};
use xode_engine::xode_core::store::{Project, SessionInfo};
use xode_engine::xode_core::{Config, LiveStats, Message, Mode};
use xode_engine::xode_kb::store::Graph;
use xode_engine::xode_kb::{Hit, NoteView, Source};
use xode_engine::{Attachment, CommandInfo, CommandResult, ContextView, Engine, FileEntry, GatewayTest, KbFolder, KbOverview, KbSearchReq, PermDecision};

type Eng<'a> = State<'a, Arc<Engine>>;
type Res<T> = Result<T, String>;

/// Maps an engine error (anyhow) to a string, keeping the context chain.
macro_rules! map {
    ($x:expr) => {
        $x.map_err(|err| format!("{err:#}"))
    };
}

#[tauri::command]
pub fn config(engine: Eng) -> Config {
    engine.config()
}

#[tauri::command]
pub fn set_config(engine: Eng, cfg: Config) -> Res<()> {
    map!(engine.set_config(cfg))
}

#[tauri::command]
pub fn projects(engine: Eng) -> Res<Vec<Project>> {
    map!(engine.projects())
}

#[tauri::command]
pub fn add_project(engine: Eng, root: String) -> Res<Project> {
    map!(engine.add_project(&root))
}

#[tauri::command]
pub fn update_project(engine: Eng, project: Project) -> Res<()> {
    map!(engine.update_project(project))
}

#[tauri::command]
pub fn remove_project(engine: Eng, id: String) -> Res<()> {
    map!(engine.remove_project(&id))
}

#[tauri::command]
pub fn sessions(engine: Eng, project_id: Option<String>) -> Res<Vec<SessionInfo>> {
    map!(engine.sessions(project_id.as_deref()))
}

#[tauri::command]
pub fn new_session(engine: Eng, project_id: String) -> Res<SessionInfo> {
    map!(engine.new_session(&project_id))
}

#[tauri::command]
pub fn session(engine: Eng, id: String) -> Res<SessionInfo> {
    map!(engine.session(&id))
}

#[tauri::command]
pub fn rename_session(engine: Eng, id: String, title: String) -> Res<()> {
    map!(engine.rename_session(&id, &title))
}

#[tauri::command]
pub fn delete_session(engine: Eng, id: String) -> Res<()> {
    map!(engine.delete_session(&id))
}

#[tauri::command]
pub fn messages(engine: Eng, session_id: String) -> Res<Vec<Message>> {
    map!(engine.messages(&session_id))
}

#[tauri::command]
pub async fn send(engine: Eng<'_>, session_id: String, text: String, attachments: Vec<Attachment>) -> Res<()> {
    map!(engine.inner().send(&session_id, text, attachments).await)
}

#[tauri::command]
pub fn cancel(engine: Eng, session_id: String) {
    engine.cancel(&session_id)
}

#[tauri::command]
pub fn steer(engine: Eng, session_id: String) {
    engine.steer(&session_id)
}

#[tauri::command]
pub fn rewind(engine: Eng, session_id: String, message_id: String, restore_files: bool) -> Res<xode_engine::RewindResult> {
    map!(engine.rewind(&session_id, &message_id, restore_files))
}

#[tauri::command]
pub fn unqueue(engine: Eng, session_id: String, id: String) {
    engine.unqueue(&session_id, &id)
}

#[tauri::command]
pub fn queued(engine: Eng, session_id: String) -> Vec<xode_engine::xode_core::event::QueuedMsg> {
    engine.queued(&session_id)
}

#[tauri::command]
pub fn is_running(engine: Eng, session_id: String) -> bool {
    engine.is_running(&session_id)
}

#[tauri::command]
pub async fn command(engine: Eng<'_>, session_id: String, line: String) -> Res<CommandResult> {
    map!(engine.inner().command(&session_id, &line).await)
}

#[tauri::command]
pub fn commands(engine: Eng, project_id: Option<String>) -> Vec<CommandInfo> {
    engine.commands(project_id.as_deref())
}

#[tauri::command]
pub fn set_mode(engine: Eng, session_id: String, mode: Mode) -> Res<()> {
    map!(engine.set_mode(&session_id, mode))
}

#[tauri::command]
pub fn set_model(engine: Eng, session_id: String, gateway_id: String, model: String) -> Res<()> {
    map!(engine.set_model(&session_id, &gateway_id, &model))
}

#[tauri::command]
pub fn context_view(engine: Eng, session_id: String) -> Res<ContextView> {
    map!(engine.context_view(&session_id))
}

#[tauri::command]
pub fn stats(engine: Eng, session_id: String) -> LiveStats {
    engine.stats(&session_id)
}

#[tauri::command]
pub fn permission_reply(engine: Eng, req_id: String, decision: PermDecision) {
    engine.permission_reply(&req_id, decision)
}

#[tauri::command]
pub async fn detect_gateway(engine: Eng<'_>, url: String, api_key: String) -> Res<Gateway> {
    map!(engine.detect_gateway(&url, &api_key).await)
}

#[tauri::command]
pub async fn test_gateway(engine: Eng<'_>, gateway: Gateway) -> Res<GatewayTest> {
    Ok(engine.test_gateway(gateway).await)
}

#[tauri::command]
pub async fn scan_gateways(engine: Eng<'_>, extra_hosts: Vec<String>) -> Res<Vec<Gateway>> {
    Ok(engine.scan_gateways(extra_hosts).await)
}

#[tauri::command]
pub async fn refresh_models(engine: Eng<'_>, gateway_id: String) -> Res<Gateway> {
    map!(engine.refresh_models(&gateway_id).await)
}

#[tauri::command]
pub fn list_dir(engine: Eng, path: String) -> Res<Vec<FileEntry>> {
    map!(engine.list_dir(&path))
}

#[tauri::command]
pub fn complete_path(engine: Eng, project_id: String, prefix: String) -> Vec<String> {
    engine.complete_path(&project_id, &prefix)
}

#[tauri::command]
pub async fn browser_test(engine: Eng<'_>) -> Res<GatewayTest> {
    Ok(engine.browser_test().await)
}

#[tauri::command]
pub async fn mcp_test(engine: Eng<'_>, server: McpServer) -> Res<Vec<String>> {
    map!(engine.mcp_test(server).await)
}

#[tauri::command]
pub fn mcp_status(engine: Eng) -> serde_json::Value {
    engine.mcp_status()
}

#[tauri::command]
pub fn index_stats(engine: Eng, project_id: String) -> serde_json::Value {
    engine.index_stats(&project_id)
}

/// Used by the export flow after the native save dialog.
#[tauri::command]
pub fn write_text_file(path: String, contents: String) -> Res<()> {
    std::fs::write(&path, contents).map_err(|err| err.to_string())
}

// ---------------------------------------------------------------- knowledge base
// Blocking engine calls (DB queries, graph building) run off the main thread.

async fn blocking<T: Send + 'static>(engine: Eng<'_>, f: impl FnOnce(&Engine) -> anyhow::Result<T> + Send + 'static) -> Res<T> {
    let e = engine.inner().clone();
    match tauri::async_runtime::spawn_blocking(move || f(&e)).await {
        Ok(r) => map!(r),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn kb_overview(engine: Eng<'_>, project_id: String) -> Res<KbOverview> {
    blocking(engine, move |e| e.kb_overview(&project_id)).await
}

#[tauri::command]
pub async fn kb_add_source(engine: Eng<'_>, project_id: String, layer: String, path: String, name: String) -> Res<Source> {
    blocking(engine, move |e| e.kb_add_source(&project_id, &layer, &path, &name)).await
}

#[tauri::command]
pub async fn kb_remove_source(engine: Eng<'_>, project_id: String, key: String) -> Res<()> {
    blocking(engine, move |e| e.kb_remove_source(&project_id, &key)).await
}

#[tauri::command]
pub async fn kb_update_source(
    engine: Eng<'_>,
    project_id: String,
    key: String,
    name: Option<String>,
    default_on: Option<bool>,
) -> Res<()> {
    blocking(engine, move |e| e.kb_update_source(&project_id, &key, name, default_on)).await
}

#[tauri::command]
pub async fn kb_reindex(engine: Eng<'_>, project_id: String, key: String) -> Res<()> {
    blocking(engine, move |e| e.kb_reindex(&project_id, &key)).await
}

#[tauri::command]
pub async fn kb_search(engine: Eng<'_>, project_id: String, req: KbSearchReq) -> Res<Vec<Hit>> {
    map!(engine.kb_search(&project_id, req).await)
}

#[tauri::command]
pub async fn kb_note(engine: Eng<'_>, project_id: String, id: String) -> Res<NoteView> {
    blocking(engine, move |e| e.kb_note(&project_id, &id)).await
}

#[tauri::command]
pub async fn kb_list(engine: Eng<'_>, project_id: String, key: String, dir: String, tag: String) -> Res<KbFolder> {
    blocking(engine, move |e| e.kb_list(&project_id, &key, &dir, &tag)).await
}

#[tauri::command]
pub async fn kb_save_note(engine: Eng<'_>, project_id: String, id: String, text: String) -> Res<()> {
    blocking(engine, move |e| e.kb_save_note(&project_id, &id, &text)).await
}

#[tauri::command]
pub async fn kb_create_note(engine: Eng<'_>, project_id: String, global: bool, title: String, body: String) -> Res<String> {
    blocking(engine, move |e| e.kb_create_note(&project_id, global, &title, &body)).await
}

#[tauri::command]
pub async fn kb_delete_note(engine: Eng<'_>, project_id: String, id: String) -> Res<()> {
    blocking(engine, move |e| e.kb_delete_note(&project_id, &id)).await
}

#[tauri::command]
pub async fn kb_graph(engine: Eng<'_>, project_id: String, off: Vec<String>, limit: usize) -> Res<Graph> {
    blocking(engine, move |e| e.kb_graph(&project_id, off, limit)).await
}

#[tauri::command]
pub async fn kb_titles(engine: Eng<'_>, project_id: String, q: String) -> Res<Vec<(String, String)>> {
    blocking(engine, move |e| e.kb_titles(&project_id, &q)).await
}

#[tauri::command]
pub fn kb_embed_retry(engine: Eng) {
    engine.kb_embed_retry()
}

#[tauri::command]
pub fn set_session_kb(engine: Eng, session_id: String, off: Vec<String>) -> Res<()> {
    map!(engine.set_session_kb(&session_id, off))
}

/// `XODE_DEMO=1`: the UI runs on the built-in mock engine and plays a scripted demo (README recording).
#[tauri::command]
pub fn demo_mode() -> bool {
    std::env::var("XODE_DEMO").map(|v| !v.is_empty() && v != "0").unwrap_or(false)
}

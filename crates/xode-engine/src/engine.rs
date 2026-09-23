use crate::api::*;
use crate::commands;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use xode_core::agent::{CtxCache, PermReply, PermissionBroker, Queued, RepoMapFn, Runtime, SteerQueue};
use xode_core::config::{Gateway, McpServer};
use xode_core::store::{Project, SessionInfo, Store};
use xode_core::tool::{Outliner, SessionState, SharedState};
use xode_core::{context, gateway, tokens, AgentEvent, Config, LiveStats, Message, Mode, MsgKind, Part, Role, ToolRegistry};
use xode_index::ProjectIndex;
use xode_net::McpManager;

/// Long-lived per-session runtime bits (survive between runs).
pub(crate) struct SessionRt {
    pub state: SharedState,
    pub stats: Arc<Mutex<LiveStats>>,
    pub cache: Arc<Mutex<CtxCache>>,
    pub always: Arc<Mutex<HashSet<String>>>,
    pub cancel: Mutex<Option<CancellationToken>>,
    pub running: AtomicBool,
    pub user_stopped: Arc<AtomicBool>,
    /// File snapshots per run: (run start ms, [(path, previous bytes)]).
    pub undo: Mutex<Vec<UndoSet>>,
    pub redo: Mutex<Vec<UndoSet>>,
    pub run_started: Mutex<i64>,
    pub queue: Arc<SteerQueue>,
}

pub struct Engine {
    pub(crate) store: Arc<Store>,
    config: RwLock<Arc<Config>>,
    events: broadcast::Sender<AgentEvent>,
    broker: Arc<PermissionBroker>,
    sessions: Mutex<HashMap<String, Arc<SessionRt>>>,
    indexes: Mutex<HashMap<String, Arc<ProjectIndex>>>,
    mcp: tokio::sync::Mutex<Option<(Vec<McpServer>, Arc<McpManager>)>>,
    mcp_status: Mutex<serde_json::Value>,
}

pub type UndoSet = (i64, Vec<(String, Option<Vec<u8>>)>);

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Engine {
    pub fn new() -> Result<Arc<Engine>> {
        let store = Arc::new(Store::open_default()?);
        let (events, _) = broadcast::channel(8192);
        let e = Arc::new(Engine {
            store,
            config: RwLock::new(Arc::new(Config::load())),
            events,
            broker: Arc::new(PermissionBroker::default()),
            sessions: Mutex::new(HashMap::new()),
            indexes: Mutex::new(HashMap::new()),
            mcp: tokio::sync::Mutex::new(None),
            mcp_status: Mutex::new(serde_json::json!([])),
        });
        if e.config().gateways.is_empty() {
            if let Ok(h) = tokio::runtime::Handle::try_current() {
                let e2 = e.clone();
                h.spawn(async move {
                    let _ = e2.bootstrap_gateways().await;
                });
            }
        }
        Ok(e)
    }

    /// First start: find local model servers and select the first model.
    async fn bootstrap_gateways(&self) -> Result<()> {
        if !self.config().gateways.is_empty() {
            return Ok(());
        }
        let found = gateway::scan(&[], gateway::SCAN_PORTS).await;
        if found.is_empty() {
            return Ok(());
        }
        let mut cfg = self.config();
        if !cfg.gateways.is_empty() {
            return Ok(());
        }
        cfg.selected.gateway = found[0].id.clone();
        cfg.selected.model = found[0].models.first().map(|m| m.id.clone()).unwrap_or_default();
        cfg.gateways = found;
        self.set_config(cfg)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
    fn emit(&self, e: AgentEvent) {
        let _ = self.events.send(e);
    }

    pub fn config(&self) -> Config {
        (**self.config.read()).clone()
    }
    fn config_arc(&self) -> Arc<Config> {
        self.config.read().clone()
    }
    pub fn set_config(&self, cfg: Config) -> Result<()> {
        cfg.save()?;
        *self.config.write() = Arc::new(cfg);
        Ok(())
    }

    // ------------------------------------------------------------ projects
    pub fn projects(&self) -> Result<Vec<Project>> {
        self.store.projects()
    }
    pub fn add_project(&self, root: &str) -> Result<Project> {
        let p = PathBuf::from(root.trim());
        let p = dunce_canon(&p).with_context(|| format!("no such folder: {root}"))?;
        if !p.is_dir() {
            bail!("not a folder: {root}");
        }
        self.store.add_project(&p.to_string_lossy(), None)
    }
    pub fn update_project(&self, p: Project) -> Result<()> {
        self.store.update_project(&p)
    }
    pub fn remove_project(&self, id: &str) -> Result<()> {
        self.indexes.lock().remove(id);
        self.store.remove_project(id)
    }
    fn project(&self, id: &str) -> Result<Project> {
        self.store.project(id)?.ok_or_else(|| anyhow!("project not found"))
    }

    // ------------------------------------------------------------ sessions
    pub fn sessions(&self, project_id: Option<&str>) -> Result<Vec<SessionInfo>> {
        self.store.sessions(project_id)
    }
    pub fn new_session(&self, project_id: &str) -> Result<SessionInfo> {
        let cfg = self.config();
        let mut s = self.store.create_session(project_id)?;
        s.mode = cfg.selected.mode;
        if let Some((g, m)) = cfg.active() {
            s.gateway = g.id;
            s.model = m;
        }
        self.store.save_session(&s)?;
        let _ = self.store.touch_project(project_id);
        Ok(s)
    }
    pub fn session(&self, id: &str) -> Result<SessionInfo> {
        self.store.session(id)?.ok_or_else(|| anyhow!("session not found"))
    }
    pub fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        let mut s = self.session(id)?;
        s.title = title.trim().to_string();
        self.store.save_session(&s)
    }
    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.cancel(id);
        self.sessions.lock().remove(id);
        self.store.delete_session(id)
    }
    pub fn messages(&self, session_id: &str) -> Result<Vec<Message>> {
        self.store.messages(session_id)
    }

    pub(crate) fn srt(&self, session_id: &str) -> Arc<SessionRt> {
        let mut map = self.sessions.lock();
        map.entry(session_id.to_string())
            .or_insert_with(|| {
                let mut st = SessionState::default();
                if let Ok(Some(s)) = self.store.session(session_id) {
                    st.segment = s.segment;
                    st.cwd = s.cwd.as_ref().map(PathBuf::from).filter(|p| p.is_dir());
                    if let Some(t) = self.store_touched(session_id) {
                        st.touched = t;
                    }
                }
                Arc::new(SessionRt {
                    state: Arc::new(Mutex::new(st)),
                    stats: Arc::new(Mutex::new(LiveStats::default())),
                    cache: Arc::new(Mutex::new(CtxCache::default())),
                    always: Arc::new(Mutex::new(HashSet::new())),
                    cancel: Mutex::new(None),
                    running: AtomicBool::new(false),
                    user_stopped: Arc::new(AtomicBool::new(false)),
                    undo: Mutex::new(vec![]),
                    redo: Mutex::new(vec![]),
                    run_started: Mutex::new(0),
                    queue: Arc::new(SteerQueue::default()),
                })
            })
            .clone()
    }

    fn touched_file(&self, session_id: &str) -> PathBuf {
        let d = xode_core::config::data_dir().join("sessions");
        let _ = std::fs::create_dir_all(&d);
        d.join(format!("{session_id}.touched.json"))
    }
    fn store_touched(&self, session_id: &str) -> Option<std::collections::BTreeMap<String, xode_core::tool::FileTouch>> {
        let s = std::fs::read_to_string(self.touched_file(session_id)).ok()?;
        serde_json::from_str(&s).ok()
    }
    fn save_touched(&self, session_id: &str, rt: &SessionRt) {
        let t = rt.state.lock().touched.clone();
        if let Ok(s) = serde_json::to_string(&t) {
            let _ = std::fs::write(self.touched_file(session_id), s);
        }
    }

    // ------------------------------------------------------------ index / tools
    pub(crate) fn index(&self, project: &Project) -> Option<Arc<ProjectIndex>> {
        let cfg = self.config_arc();
        if !cfg.tools.index_enabled {
            return None;
        }
        let mut m = self.indexes.lock();
        if let Some(i) = m.get(&project.id) {
            return Some(i.clone());
        }
        match ProjectIndex::open(Path::new(&project.root), &cfg.tools) {
            Ok(i) => {
                m.insert(project.id.clone(), i.clone());
                Some(i)
            }
            Err(e) => {
                tracing::warn!("index open failed: {e}");
                None
            }
        }
    }

    async fn mcp_manager(&self, cfg: &Config) -> Option<Arc<McpManager>> {
        let servers: Vec<McpServer> = cfg.mcp.iter().filter(|s| s.enabled).cloned().collect();
        let mut g = self.mcp.lock().await;
        if let Some((cur, mgr)) = g.as_ref() {
            if *cur == servers {
                return Some(mgr.clone());
            }
            mgr.shutdown().await;
        }
        if servers.is_empty() {
            *g = None;
            *self.mcp_status.lock() = serde_json::json!([]);
            return None;
        }
        let mgr = McpManager::start(&servers).await;
        *self.mcp_status.lock() = serde_json::to_value(mgr.status()).unwrap_or_default();
        *g = Some((servers, mgr.clone()));
        Some(mgr)
    }

    async fn registry(&self, cfg: &Config, index: Option<&Arc<ProjectIndex>>) -> ToolRegistry {
        let mut r = ToolRegistry::default();
        for t in xode_tools::fs_tools() {
            r.add(t);
        }
        r.add(xode_tools::shell_tool());
        if let Some(i) = index {
            r.add(xode_index::code_tool(i.clone()));
        }
        r.add(xode_net::web_search_tool());
        r.add(xode_net::web_fetch_tool());
        r.add(xode_net::browser_tool());
        if let Some(m) = self.mcp_manager(cfg).await {
            for t in m.tools() {
                r.add(t);
            }
        }
        r.tools.retain(|t| cfg.tools.is_enabled(t.name()));
        r
    }

    fn repo_map_fn(project: &Project, index: Option<Arc<ProjectIndex>>) -> RepoMapFn {
        let root = PathBuf::from(&project.root);
        Arc::new(move |budget: u64, focus: &[String]| {
            let mut s = String::new();
            if let Ok(b) = std::fs::read_to_string(root.join(".xode").join("BRIEF.md")) {
                s.push_str(b.trim());
                s.push_str("\n\n");
            }
            if let Some(i) = &index {
                s.push_str(&i.repo_map(budget, focus));
            }
            s
        })
    }

    pub(crate) async fn runtime(&self, session_id: &str) -> Result<Runtime> {
        let cfg = self.config_arc();
        let sess = self.session(session_id)?;
        let project = self.project(&sess.project_id)?;
        let (gw, model) = self.resolve_model(&cfg, &sess)?;
        let index = self.index(&project);
        let tools = self.registry(&cfg, index.as_ref()).await;
        let rt = self.srt(session_id);
        let cancel = rt.cancel.lock().clone().unwrap_or_default();
        Ok(Runtime {
            session_id: session_id.to_string(),
            project: project.clone(),
            store: self.store.clone(),
            config: cfg.clone(),
            gateway: gw,
            model,
            tools,
            state: rt.state.clone(),
            outliner: index.clone().map(|i| i as Arc<dyn Outliner>),
            repo_map: Some(Self::repo_map_fn(&project, index)),
            events: self.events.clone(),
            cancel,
            broker: self.broker.clone(),
            always: rt.always.clone(),
            stats: rt.stats.clone(),
            cache: rt.cache.clone(),
            user_stopped: rt.user_stopped.clone(),
            queue: rt.queue.clone(),
        })
    }

    fn resolve_model(&self, cfg: &Config, sess: &SessionInfo) -> Result<(Gateway, String)> {
        if let Some(g) = cfg.gateway(&sess.gateway).filter(|g| g.enabled) {
            let m = if sess.model.is_empty() { g.models.first().map(|m| m.id.clone()).unwrap_or_default() } else { sess.model.clone() };
            if !m.is_empty() {
                return Ok((g.clone(), m));
            }
        }
        cfg.active().filter(|(_, m)| !m.is_empty()).ok_or_else(|| anyhow!("no model configured: add a gateway in Settings → Gateway"))
    }

    // ------------------------------------------------------------ running
    pub async fn send(self: &Arc<Self>, session_id: &str, text: String, attachments: Vec<Attachment>) -> Result<()> {
        if self.config().gateways.is_empty() {
            let _ = self.bootstrap_gateways().await;
        }
        let mut sess = self.session(session_id)?;
        let project = self.project(&sess.project_id)?;
        if sess.title.is_empty() {
            sess.title = context::clip(text.lines().next().unwrap_or(""), 60).trim_end_matches('…').to_string();
            if sess.title.is_empty() {
                sess.title = "New chat".into();
            }
            sess.updated_at = now();
            self.store.save_session(&sess)?;
        }
        let msg = self.build_user_message(&project, &sess, &text, &attachments)?;
        let rt = self.srt(session_id);
        if rt.running.load(Ordering::SeqCst) {
            // Steer: delivered after the next tool call (or immediately via `steer`).
            rt.queue.push(msg);
            self.emit_queue(session_id);
            return Ok(());
        }
        if !rt.queue.is_empty() {
            // Leftovers from a stopped run go first.
            rt.queue.push(msg);
            self.flush_queue(session_id).await;
            return Ok(());
        }
        self.start_run(session_id, Some(msg))
    }

    /// Deliver queued messages now: interrupts the current generation.
    pub fn steer(&self, session_id: &str) {
        let rt = self.srt(session_id);
        if rt.running.load(Ordering::SeqCst) {
            rt.queue.force();
        }
    }

    /// Remove a queued message before it is delivered.
    pub fn unqueue(&self, session_id: &str, id: &str) {
        let rt = self.srt(session_id);
        if rt.queue.remove(id) {
            self.emit_queue(session_id);
        }
    }

    /// Currently queued messages.
    pub fn queued(&self, session_id: &str) -> Vec<xode_core::event::QueuedMsg> {
        self.srt(session_id).queue.snapshot()
    }

    pub(crate) fn start_run(self: &Arc<Self>, session_id: &str, msg: Option<Message>) -> Result<()> {
        let rt = self.srt(session_id);
        if rt.running.swap(true, Ordering::SeqCst) {
            bail!("already running");
        }
        let cancel = CancellationToken::new();
        *rt.cancel.lock() = Some(cancel.clone());
        *rt.run_started.lock() = now();
        let me = self.clone();
        let sid = session_id.to_string();
        self.emit(AgentEvent::State { session: sid.clone(), running: true });
        tokio::spawn(async move {
            let res = match me.runtime(&sid).await {
                Ok(r) => r.run(msg).await.map(|_| ()),
                Err(e) => {
                    if let Some(m) = msg {
                        let _ = me.store.append_message(&sid, &m);
                        me.emit(AgentEvent::Message { session: sid.clone(), message: m });
                    }
                    Err(e)
                }
            };
            let error = res.is_err();
            if let Err(e) = res {
                me.emit(AgentEvent::Error { session: sid.clone(), text: e.to_string() });
            }
            me.after_run(&sid, error).await;
        });
        Ok(())
    }

    /// Ends a run: continues with queued items unless the user stopped, then reports completion.
    pub(crate) async fn after_run(self: &Arc<Self>, sid: &str, error: bool) {
        let stopped = self.srt(sid).user_stopped.load(Ordering::SeqCst);
        self.finish_run(sid);
        if !stopped {
            self.flush_queue(sid).await;
        }
        if !self.is_running(sid) {
            self.emit(AgentEvent::Finished { session: sid.into(), stopped, error });
        }
    }

    pub(crate) fn finish_run(&self, sid: &str) {
        let rt = self.srt(sid);
        let snaps = std::mem::take(&mut rt.state.lock().undo);
        if !snaps.is_empty() {
            let t = *rt.run_started.lock();
            rt.undo.lock().push((t, snaps));
            rt.redo.lock().clear();
        }
        self.save_touched(sid, &rt);
        rt.running.store(false, Ordering::SeqCst);
        *rt.cancel.lock() = None;
        self.emit(AgentEvent::State { session: sid.to_string(), running: false });
    }

    pub(crate) fn emit_queue(&self, sid: &str) {
        let items = self.srt(sid).queue.snapshot();
        self.emit(AgentEvent::Queue { session: sid.into(), items });
    }

    /// Queue a slash command to run when the agent reaches it (`/compact` runs at the next tool call).
    pub(crate) fn enqueue(&self, sid: &str, item: Queued) {
        self.srt(sid).queue.push_item(item);
        self.emit_queue(sid);
    }

    /// Process what is left in the queue after a run ended: commands run in order, and the
    /// next batch of messages starts a new run (which flushes again when it ends).
    /// Boxed: commands can send messages, which flush the queue again.
    fn flush_queue<'a>(self: &'a Arc<Self>, sid: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
        let rt = self.srt(sid);
        loop {
            if self.is_running(sid) {
                return;
            }
            let Some(item) = rt.queue.pop() else { return };
            match item {
                Queued::Msg(first) => {
                    let mut msgs = vec![first];
                    while let Some(Queued::Msg(_)) = rt.queue.items.lock().first() {
                        if let Some(Queued::Msg(m)) = rt.queue.pop() {
                            msgs.push(m);
                        }
                    }
                    self.emit_queue(sid);
                    let last = msgs.pop();
                    for m in msgs {
                        let _ = self.store.append_message(sid, &m);
                        self.emit(AgentEvent::Message { session: sid.into(), message: m });
                    }
                    let _ = self.start_run(sid, last);
                    return;
                }
                Queued::Compact { .. } => {
                    self.emit_queue(sid);
                    if let Err(e) = crate::commands::compact_now(self, sid) {
                        self.emit_notice(sid, format!("compaction failed: {e}"));
                    }
                }
                Queued::Cmd { line, .. } => {
                    self.emit_queue(sid);
                    match crate::commands::run(self, sid, &line).await {
                        Ok(r) => self.emit(AgentEvent::CommandDone {
                            session: sid.into(),
                            result: serde_json::to_value(&r).unwrap_or_default(),
                        }),
                        Err(e) => self.emit_notice(sid, format!("{line}: {e}")),
                    }
                }
            }
        }
        })
    }

    pub fn cancel(&self, session_id: &str) {
        if let Some(rt) = self.sessions.lock().get(session_id) {
            rt.user_stopped.store(true, Ordering::SeqCst);
            if let Some(c) = rt.cancel.lock().as_ref() {
                c.cancel();
            }
        }
    }
    pub fn is_running(&self, session_id: &str) -> bool {
        self.sessions.lock().get(session_id).map(|r| r.running.load(Ordering::SeqCst)).unwrap_or(false)
    }

    fn build_user_message(&self, project: &Project, sess: &SessionInfo, text: &str, atts: &[Attachment]) -> Result<Message> {
        let cfg = self.config();
        let vision = self
            .resolve_model(&cfg, sess)
            .ok()
            .and_then(|(g, m)| g.models.iter().find(|x| x.id == m).map(|x| x.vision))
            .unwrap_or(false)
            || matches!(cfg.gateway(&sess.gateway).map(|g| g.kind), Some(xode_core::config::ApiKind::Anthropic));
        let root = PathBuf::from(&project.root);
        let mut parts = vec![Part::text(text)];
        let mut notes = String::new();
        let inline_max = cfg.token_saving.attach_inline_max_tokens;

        let mut files: Vec<Attachment> = atts.to_vec();
        // @mentions → attachments (files only, deduped).
        for w in text.split_whitespace() {
            if let Some(p) = w.strip_prefix('@') {
                let p = p.trim_end_matches([',', '.', ')', ';', ':']);
                let abs = if Path::new(p).is_absolute() { PathBuf::from(p) } else { root.join(p) };
                if abs.is_file() && !files.iter().any(|a| Path::new(&a.path) == abs) {
                    files.push(Attachment { path: abs.to_string_lossy().into(), name: p.into(), mime: String::new(), data: String::new() });
                }
            }
        }
        for a in &files {
            let mime = if a.mime.is_empty() { guess_mime(&a.path) } else { a.mime.clone() };
            let name = if a.name.is_empty() { a.path.clone() } else { a.name.clone() };
            if mime.starts_with("image/") {
                let data = if !a.data.is_empty() {
                    a.data.clone()
                } else {
                    base64::engine::general_purpose::STANDARD.encode(std::fs::read(&a.path)?)
                };
                if vision {
                    parts.push(Part::Image { mime, data });
                } else {
                    // Save pasted images so the model can at least reference them.
                    let saved = if a.path.is_empty() {
                        let d = root.join(".xode").join("attachments");
                        let _ = std::fs::create_dir_all(&d);
                        let f = d.join(format!("{}.png", &uuid::Uuid::new_v4().simple().to_string()[..8]));
                        if let Ok(b) = base64::engine::general_purpose::STANDARD.decode(&data) {
                            let _ = std::fs::write(&f, b);
                        }
                        f.to_string_lossy().to_string()
                    } else {
                        a.path.clone()
                    };
                    notes.push_str(&format!("\n[image attached: {saved} — current model has no vision]"));
                }
                continue;
            }
            let bytes = if !a.data.is_empty() {
                base64::engine::general_purpose::STANDARD.decode(&a.data).unwrap_or_default()
            } else {
                std::fs::read(&a.path).unwrap_or_default()
            };
            if bytes.iter().take(8000).any(|b| *b == 0) {
                notes.push_str(&format!("\n[binary file attached: {}]", a.path));
                continue;
            }
            let content = String::from_utf8_lossy(&bytes);
            let t = tokens::count(&content);
            let disp = display_path(&root, &a.path, &name);
            if t <= inline_max {
                notes.push_str(&format!("\n\n<file path=\"{disp}\">\n{}\n</file>", content.trim_end()));
                // Mark as read so the model doesn't re-read it.
                if !a.path.is_empty() {
                    let rt = self.srt(&sess.id);
                    let ctx_touch = |st: &mut SessionState| {
                        let lines = content.lines().count();
                        st.touched.insert(
                            disp.clone(),
                            xode_core::tool::FileTouch {
                                path: disp.clone(),
                                how: "read".into(),
                                hash: xode_core::tool::hash_bytes(&bytes),
                                step: st.step,
                                segment: st.segment,
                                lines,
                                ranges: vec![format!("1-{lines}")],
                            },
                        );
                    };
                    ctx_touch(&mut rt.state.lock());
                }
            } else {
                let outline = self.index(project).and_then(|i| i.outline(Path::new(&a.path))).unwrap_or_default();
                notes.push_str(&format!(
                    "\n\n[attached file {disp}: {} lines, ~{t} tokens — too large to inline; read ranges as needed]\n{}",
                    content.lines().count(),
                    outline
                ));
            }
        }
        if !notes.is_empty() {
            // Kept as a separate part so frontends can show the typed text alone.
            parts.push(Part::text(notes.trim_start().to_string()));
        }
        let mut m = Message::new(Role::User, parts);
        m.segment = sess.segment;
        Ok(m)
    }

    // ------------------------------------------------------------ commands
    pub async fn command(self: &Arc<Self>, session_id: &str, line: &str) -> Result<CommandResult> {
        commands::run(self, session_id, line).await
    }
    pub fn commands(&self, project_id: Option<&str>) -> Vec<CommandInfo> {
        let root = project_id.and_then(|p| self.project(p).ok()).map(|p| PathBuf::from(p.root));
        commands::list(root.as_deref())
    }

    pub fn set_mode(&self, session_id: &str, mode: Mode) -> Result<()> {
        let mut s = self.session(session_id)?;
        s.mode = mode;
        self.store.save_session(&s)?;
        let mut cfg = self.config();
        if cfg.selected.mode != mode {
            cfg.selected.mode = mode;
            let _ = self.set_config(cfg);
        }
        Ok(())
    }
    pub fn set_model(&self, session_id: &str, gateway_id: &str, model: &str) -> Result<()> {
        let mut cfg = self.config();
        cfg.selected.gateway = gateway_id.into();
        cfg.selected.model = model.into();
        self.set_config(cfg)?;
        if !session_id.is_empty() {
            if let Ok(mut s) = self.session(session_id) {
                s.gateway = gateway_id.into();
                s.model = model.into();
                self.store.save_session(&s)?;
                self.srt(session_id).cache.lock().key.clear();
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ context
    pub fn context_view(&self, session_id: &str) -> Result<ContextView> {
        let cfg = self.config_arc();
        let sess = self.session(session_id)?;
        let project = self.project(&sess.project_id)?;
        let rt = self.srt(session_id);
        let (gw, model) = self.resolve_model(&cfg, &sess).unwrap_or_default();
        let limit = if gw.url.is_empty() { cfg.compaction.fallback_context } else { cfg.context_limit_for(&gw, &model) };
        let index = self.index(&project);
        // Tool specs without starting MCP (use the running manager if any).
        let mut reg = ToolRegistry::default();
        for t in xode_tools::fs_tools() {
            reg.add(t);
        }
        reg.add(xode_tools::shell_tool());
        if let Some(i) = &index {
            reg.add(xode_index::code_tool(i.clone()));
        }
        reg.add(xode_net::web_search_tool());
        reg.add(xode_net::web_fetch_tool());
        reg.add(xode_net::browser_tool());
        if let Ok(g) = self.mcp.try_lock() {
            if let Some((_, m)) = g.as_ref() {
                for t in m.tools() {
                    reg.add(t);
                }
            }
        }
        reg.tools.retain(|t| cfg.tools.is_enabled(t.name()));
        let specs = reg.specs();

        let r = Runtime {
            session_id: session_id.into(),
            project: project.clone(),
            store: self.store.clone(),
            config: cfg.clone(),
            gateway: gw,
            model,
            tools: reg,
            state: rt.state.clone(),
            outliner: index.clone().map(|i| i as Arc<dyn Outliner>),
            repo_map: Some(Self::repo_map_fn(&project, index)),
            events: self.events.clone(),
            cancel: CancellationToken::new(),
            broker: self.broker.clone(),
            always: rt.always.clone(),
            stats: rt.stats.clone(),
            cache: rt.cache.clone(),
            user_stopped: rt.user_stopped.clone(),
            queue: rt.queue.clone(),
        };
        let (system, brief, path_md) = r.system(&sess);
        let all = self.store.messages(session_id)?;
        let active = context::active_messages(&all, sess.segment);
        let shaped = context::shape_messages(&active, &cfg);
        let sys_only = tokens::count(&system).saturating_sub(tokens::count(&brief) + tokens::count(&path_md));
        let b = context::breakdown(sys_only, &brief, &path_md, &specs, &shaped);
        let calib = { rt.cache.lock().calib };
        let calib = if calib > 0.0 { calib.clamp(0.6, 1.8) } else { 1.0 };
        let sc = |n: u64| (n as f64 * calib) as u64;

        let mut msg_txt = String::new();
        let mut tr_txt = String::new();
        let mut per_tool_txt: std::collections::HashMap<String, String> = Default::default();
        let mut th_txt = String::new();
        let mut seed_txt = String::new();
        for m in &shaped {
            if m.kind == MsgKind::Seed {
                seed_txt.push_str(&m.text());
                continue;
            }
            for p in &m.parts {
                match p {
                    Part::Text { text } => msg_txt.push_str(&format!("[{}] {}\n\n", role_name(m.role), text)),
                    Part::ToolCall { name, args, .. } => msg_txt.push_str(&format!("[call] {name} {args}\n\n")),
                    Part::ToolResult { name, content, .. } => {
                        tr_txt.push_str(&format!("[{name}]\n{content}\n\n"));
                        per_tool_txt.entry(name.clone()).or_default().push_str(&format!("{content}\n\n"));
                    }
                    Part::Thinking { text } => th_txt.push_str(&format!("{text}\n\n")),
                    Part::Image { mime, .. } => msg_txt.push_str(&format!("[image {mime}]\n\n")),
                }
            }
        }
        let tools_txt = serde_json::to_string_pretty(&specs).unwrap_or_default();
        let sys_txt = system.replace(&brief, "").replace(&path_md, "");
        let tool_children: Vec<ContextSection> = b
            .per_tool
            .iter()
            .map(|(name, t, n)| ContextSection {
                key: format!("tool_results:{name}"),
                label: format!("{name} ×{n}"),
                tokens: sc(*t),
                content: per_tool_txt.remove(name).unwrap_or_default(),
                children: vec![],
            })
            .collect();
        let sections = vec![
            ContextSection { key: "system".into(), label: "System".into(), tokens: sc(b.system), content: sys_txt, children: vec![] },
            ContextSection { key: "tools".into(), label: "Tools".into(), tokens: sc(b.tools), content: tools_txt, children: vec![] },
            ContextSection { key: "brief".into(), label: "Repo map".into(), tokens: sc(b.brief), content: brief, children: vec![] },
            ContextSection { key: "path".into(), label: "Path".into(), tokens: sc(b.path), content: path_md, children: vec![] },
            ContextSection { key: "state".into(), label: "Handoff".into(), tokens: sc(b.seed), content: seed_txt, children: vec![] },
            ContextSection { key: "messages".into(), label: "Messages".into(), tokens: sc(b.messages), content: msg_txt, children: vec![] },
            ContextSection { key: "tool_results".into(), label: "Tool results".into(), tokens: sc(b.tool_results), content: tr_txt, children: tool_children },
            ContextSection { key: "thinking".into(), label: "Thinking".into(), tokens: sc(b.thinking), content: th_txt, children: vec![] },
            ContextSection { key: "attachments".into(), label: "Images".into(), tokens: sc(b.attachments), content: String::new(), children: vec![] },
        ];
        let used = sections.iter().map(|s| s.tokens).sum();
        Ok(ContextView {
            used,
            limit,
            threshold: cfg.threshold(limit),
            sections,
            segments: self.store.segments(session_id)?,
            path_md: xode_core::compaction::read_path_md(Path::new(&project.root), &cfg.compaction),
        })
    }

    pub fn stats(&self, session_id: &str) -> LiveStats {
        let rt = self.srt(session_id);
        let mut s = rt.stats.lock().clone();
        if let Ok(sess) = self.session(session_id) {
            let cfg = self.config();
            if s.context_limit == 0 {
                if let Ok((g, m)) = self.resolve_model(&cfg, &sess) {
                    s.context_limit = cfg.context_limit_for(&g, &m);
                    s.model = m;
                    s.gateway = g.name;
                }
            }
            if s.tokens_in == 0 {
                s.tokens_in = sess.tokens_in;
                s.tokens_out = sess.tokens_out;
            }
            s.total_work_ms = s.total_work_ms.max(sess.total_work_ms);
            s.compactions = sess.compactions;
            if s.tool_calls == 0 {
                s.tool_calls = sess.tool_calls;
            }
            if s.context_used == 0 {
                if let Ok(v) = self.context_view(session_id) {
                    s.context_used = v.used;
                }
            }
        }
        s
    }

    pub fn permission_reply(&self, req_id: &str, decision: PermDecision) {
        self.broker.reply(
            req_id,
            match decision {
                PermDecision::Once => PermReply::Once,
                PermDecision::Always => PermReply::Always,
                PermDecision::Deny => PermReply::Deny,
            },
        );
    }

    // ------------------------------------------------------------ gateways
    pub async fn detect_gateway(&self, url: &str, api_key: &str) -> Result<Gateway> {
        gateway::detect(url, api_key).await
    }
    pub async fn test_gateway(&self, g: Gateway) -> GatewayTest {
        let r = gateway::test(&g).await;
        GatewayTest { ok: r.ok, latency_ms: r.latency_ms, models: r.models, message: r.message }
    }
    pub async fn scan_gateways(&self, extra_hosts: Vec<String>) -> Vec<Gateway> {
        gateway::scan(&extra_hosts, gateway::SCAN_PORTS).await
    }
    pub async fn refresh_models(&self, gateway_id: &str) -> Result<Gateway> {
        let mut cfg = self.config();
        let g = cfg.gateways.iter_mut().find(|g| g.id == gateway_id).ok_or_else(|| anyhow!("gateway not found"))?;
        let fresh = gateway::detect(&g.url, &g.api_key).await?;
        // Keep user overrides of context size.
        let mut models = fresh.models;
        for m in &mut models {
            if let Some(old) = g.models.iter().find(|o| o.id == m.id) {
                if m.context == 0 {
                    m.context = old.context;
                }
            }
        }
        g.models = models;
        g.kind = fresh.kind;
        g.flavor = fresh.flavor;
        let out = g.clone();
        self.set_config(cfg)?;
        Ok(out)
    }

    // ------------------------------------------------------------ fs
    pub fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>> {
        let mut out = vec![];
        for e in std::fs::read_dir(path)? {
            let Ok(e) = e else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git" || name == ".DS_Store" {
                continue;
            }
            let md = e.metadata().ok();
            out.push(FileEntry {
                name,
                path: e.path().to_string_lossy().to_string(),
                is_dir: md.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                size: md.map(|m| m.len()).unwrap_or(0),
            });
        }
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }

    pub fn complete_path(&self, project_id: &str, prefix: &str) -> Vec<String> {
        let Ok(p) = self.project(project_id) else { return vec![] };
        if let Some(i) = self.index(&p) {
            let r = i.file_paths(prefix, 30);
            if !r.is_empty() {
                return r;
            }
        }
        let needle = prefix.to_lowercase().replace('\\', "/");
        let mut out = vec![];
        for e in ignore::WalkBuilder::new(&p.root).hidden(true).build().flatten() {
            let rel = e.path().strip_prefix(&p.root).unwrap_or(e.path()).to_string_lossy().replace('\\', "/");
            if rel.is_empty() {
                continue;
            }
            if rel.to_lowercase().contains(&needle) {
                out.push(rel);
                if out.len() >= 30 {
                    break;
                }
            }
        }
        out
    }

    pub async fn browser_test(&self) -> GatewayTest {
        let t = std::time::Instant::now();
        match xode_net::browser_test(&self.config().browser).await {
            Ok(v) => GatewayTest { ok: true, latency_ms: t.elapsed().as_millis() as u64, models: 0, message: v },
            Err(e) => GatewayTest { ok: false, latency_ms: t.elapsed().as_millis() as u64, models: 0, message: e.to_string() },
        }
    }
    pub async fn mcp_test(&self, server: McpServer) -> Result<Vec<String>> {
        xode_net::mcp::test_server(&server).await
    }
    pub fn mcp_status(&self) -> serde_json::Value {
        self.mcp_status.lock().clone()
    }
    pub fn index_stats(&self, project_id: &str) -> serde_json::Value {
        let Ok(p) = self.project(project_id) else { return serde_json::json!({}) };
        match self.index(&p) {
            Some(i) => serde_json::to_value(i.stats()).unwrap_or_default(),
            None => serde_json::json!({"files": 0, "symbols": 0, "ready": false}),
        }
    }

    // ------------------------------------------------------------ helpers for commands
    pub(crate) fn emit_state(&self, sid: &str, running: bool) {
        self.emit(AgentEvent::State { session: sid.into(), running });
    }
    pub(crate) fn emit_notice(&self, sid: &str, text: impl Into<String>) {
        self.emit(AgentEvent::Notice { session: sid.into(), text: text.into() });
    }
    pub(crate) fn project_of(&self, sid: &str) -> Result<Project> {
        self.project(&self.session(sid)?.project_id)
    }
    /// Rewinds the chat: a user message is removed with everything after it (its text is
    /// returned for the composer); any other message is kept and what follows is removed.
    /// With `restore_files`, file changes of runs started after that point are reverted.
    pub fn rewind(&self, sid: &str, message_id: &str, restore_files: bool) -> Result<RewindResult> {
        if self.is_running(sid) {
            bail!("stop the current run first");
        }
        let msgs = self.messages(sid)?;
        let i = msgs.iter().position(|m| m.id == message_id).ok_or_else(|| anyhow!("message not found"))?;
        let target = &msgs[i];
        let inclusive = target.role == Role::User && target.kind == MsgKind::Normal;
        let mut keep = if inclusive { i } else { i + 1 };
        // Keep the results of the kept tool calls so the conversation stays well-formed.
        while !inclusive && msgs.get(keep).map(|m| m.role == Role::Tool).unwrap_or(false) {
            keep += 1;
        }
        let Some(first_removed) = msgs.get(keep) else {
            return Ok(RewindResult { text: String::new(), removed: 0, files: 0 });
        };
        let cutoff = if inclusive { target.created_at } else { first_removed.created_at };
        let text = if inclusive {
            target.parts.iter().find_map(|p| if let Part::Text { text } = p { Some(text.clone()) } else { None }).unwrap_or_default()
        } else {
            String::new()
        };

        let mut files = 0;
        if restore_files {
            let rt = self.srt(sid);
            let mut undo = rt.undo.lock();
            while undo.last().map(|(t, _)| *t >= cutoff).unwrap_or(false) {
                let (t, set) = undo.pop().unwrap();
                let (inverse, n) = commands::apply_snapshot(&set)?;
                rt.redo.lock().push((t, inverse));
                files += n;
            }
        }

        let removed = if inclusive {
            self.store.truncate_messages(sid, message_id, true)?
        } else {
            self.store.truncate_messages(sid, &msgs[keep - 1].id, false)?
        };
        let segment = msgs[..keep].last().map(|m| m.segment).unwrap_or(0);
        self.store.truncate_segments(sid, segment)?;
        let mut s = self.session(sid)?;
        let segs = self.store.segments(sid)?;
        s.segment = segment;
        s.compactions = segs.len() as u32;
        s.state = segs.iter().rev().find(|g| g.index <= segment).map(|g| g.state.clone()).unwrap_or_default();
        s.updated_at = now();
        self.store.save_session(&s)?;
        // Drop cached runtime state (prompt cache, read dedup) tied to the removed tail.
        {
            let rt = self.srt(sid);
            rt.cache.lock().key.clear();
            rt.queue.items.lock().clear();
            // Read dedup must not claim the model saw file contents that were rewound away.
            rt.state.lock().touched.clear();
            self.save_touched(sid, &rt);
        }
        self.emit_queue(sid);
        Ok(RewindResult { text, removed, files })
    }

    pub(crate) fn clear_session(&self, sid: &str) -> Result<()> {
        self.cancel(sid);
        self.store.clear_messages(sid)?;
        let mut s = self.session(sid)?;
        s.segment = 0;
        s.state.clear();
        s.compactions = 0;
        s.goal = None;
        self.store.save_session(&s)?;
        self.sessions.lock().remove(sid);
        let _ = std::fs::remove_file(self.touched_file(sid));
        Ok(())
    }
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::System => "system",
    }
}

fn display_path(root: &Path, path: &str, name: &str) -> String {
    if path.is_empty() {
        return name.to_string();
    }
    Path::new(path).strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| path.replace('\\', "/"))
}

pub(crate) fn guess_mime(path: &str) -> String {
    let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "text/plain",
    }
    .to_string()
}

/// Canonicalize without the `\\?\` prefix on Windows.
fn dunce_canon(p: &Path) -> Result<PathBuf> {
    let c = std::fs::canonicalize(p)?;
    let s = c.to_string_lossy().to_string();
    Ok(PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s)))
}

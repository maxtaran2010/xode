//! The agent loop: model → tools → model … with silent self-compaction and the goal hook.
use crate::compaction::{self, Handoff};
use crate::config::{Config, Gateway};
use crate::context::{self, Env};
use crate::event::{AgentEvent, EventTx, LiveStats};
use crate::permissions::{self, CallInfo, Decision};
use crate::provider::{self, openai::ContextOverflow, ChatRequest, StreamEvent};
use crate::store::{Project, Segment, SessionInfo, Store};
use crate::tool::{Outliner, SharedState, ToolCtx, ToolRegistry};
use crate::types::{Message, Mode, MsgKind, Part, Role};
use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermReply {
    Once,
    Always,
    Deny,
}

#[derive(Default)]
pub struct PermissionBroker {
    pending: Mutex<HashMap<String, oneshot::Sender<PermReply>>>,
}

impl PermissionBroker {
    pub fn register(&self) -> (String, oneshot::Receiver<PermReply>) {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.clone(), tx);
        (id, rx)
    }
    pub fn reply(&self, id: &str, r: PermReply) {
        if let Some(tx) = self.pending.lock().remove(id) {
            let _ = tx.send(r);
        }
    }
}

/// Something waiting for the running agent: a user message, an inline compaction, or a
/// command the engine runs once the run ends.
#[derive(Debug, Clone)]
pub enum Queued {
    Msg(Message),
    Compact { id: String },
    Cmd { id: String, line: String },
}

impl Queued {
    pub fn id(&self) -> &str {
        match self {
            Queued::Msg(m) => &m.id,
            Queued::Compact { id } | Queued::Cmd { id, .. } => id,
        }
    }
    /// Handled inside the run (at the next tool boundary).
    pub fn inline(&self) -> bool {
        !matches!(self, Queued::Cmd { .. })
    }
    fn view(&self) -> crate::event::QueuedMsg {
        match self {
            Queued::Msg(m) => crate::event::QueuedMsg { id: m.id.clone(), text: m.text(), command: false },
            Queued::Compact { id } => crate::event::QueuedMsg { id: id.clone(), text: "/compact".into(), command: true },
            Queued::Cmd { id, line } => crate::event::QueuedMsg { id: id.clone(), text: line.clone(), command: true },
        }
    }
}

/// Items sent while a run is in progress ("steer" queue).
#[derive(Default)]
pub struct SteerQueue {
    pub items: Mutex<Vec<Queued>>,
    /// Cancels only the current model stream (not the run) to deliver the queue now.
    pub turn: Mutex<Option<CancellationToken>>,
    pub forced: std::sync::atomic::AtomicBool,
}

impl SteerQueue {
    pub fn snapshot(&self) -> Vec<crate::event::QueuedMsg> {
        self.items.lock().iter().map(Queued::view).collect()
    }
    pub fn push(&self, m: Message) {
        self.items.lock().push(Queued::Msg(m));
    }
    pub fn push_item(&self, q: Queued) {
        self.items.lock().push(q);
    }
    pub fn remove(&self, id: &str) -> bool {
        let mut q = self.items.lock();
        let n = q.len();
        q.retain(|m| m.id() != id);
        q.len() != n
    }
    /// Pops the front item if the run itself can handle it.
    pub fn pop_inline(&self) -> Option<Queued> {
        let mut q = self.items.lock();
        if q.first().map(Queued::inline).unwrap_or(false) {
            Some(q.remove(0))
        } else {
            None
        }
    }
    pub fn pop(&self) -> Option<Queued> {
        let mut q = self.items.lock();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    }
    pub fn is_empty(&self) -> bool {
        self.items.lock().is_empty()
    }
    /// Interrupt the current generation so queued messages are delivered immediately.
    pub fn force(&self) {
        self.forced.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = self.turn.lock().as_ref() {
            t.cancel();
        }
    }
}

pub type RepoMapFn = Arc<dyn Fn(u64, &[String]) -> String + Send + Sync>;

/// Per-session prompt cache: system prompt is rebuilt only when the segment or mode changes.
#[derive(Debug, Clone, Default)]
pub struct CtxCache {
    pub key: String,
    pub system: String,
    pub brief: String,
    pub path_in_system: String,
    /// Ratio actual/estimated prompt tokens (server tokenizer vs cl100k).
    pub calib: f64,
    pub last_used: u64,
}

pub struct Runtime {
    pub session_id: String,
    pub project: Project,
    pub store: Arc<Store>,
    pub config: Arc<Config>,
    pub gateway: Gateway,
    pub model: String,
    pub tools: ToolRegistry,
    pub state: SharedState,
    pub outliner: Option<Arc<dyn Outliner>>,
    pub repo_map: Option<RepoMapFn>,
    pub events: EventTx,
    pub cancel: CancellationToken,
    pub broker: Arc<PermissionBroker>,
    pub always: Arc<Mutex<HashSet<String>>>,
    pub stats: Arc<Mutex<LiveStats>>,
    pub cache: Arc<Mutex<CtxCache>>,
    /// Set when the user stopped the run; the goal hook must not auto-continue.
    pub user_stopped: Arc<std::sync::atomic::AtomicBool>,
    pub queue: Arc<SteerQueue>,
}

pub struct RunOutcome {
    pub stopped: bool,
}

impl Runtime {
    fn emit(&self, e: AgentEvent) {
        let _ = self.events.send(e);
    }
    fn sid(&self) -> String {
        self.session_id.clone()
    }
    fn session(&self) -> Result<SessionInfo> {
        self.store.session(&self.session_id)?.ok_or_else(|| anyhow!("session not found"))
    }
    fn save_session(&self, s: &mut SessionInfo) {
        s.updated_at = chrono::Utc::now().timestamp_millis();
        let _ = self.store.save_session(s);
    }
    pub fn context_limit(&self) -> u64 {
        self.config.context_limit_for(&self.gateway, &self.model)
    }
    fn root(&self) -> PathBuf {
        PathBuf::from(&self.project.root)
    }

    fn tool_ctx(&self) -> ToolCtx {
        ToolCtx {
            session_id: self.sid(),
            project_root: self.root(),
            extra_roots: self.project.extra_roots.iter().map(PathBuf::from).collect(),
            config: self.config.clone(),
            state: self.state.clone(),
            cancel: self.cancel.clone(),
            outliner: self.outliner.clone(),
        }
    }

    fn shell_name(&self) -> String {
        let s = &self.config.tools.shell;
        if s != "auto" {
            return s.clone();
        }
        if cfg!(windows) {
            "powershell".into()
        } else {
            "bash".into()
        }
    }

    /// Build (or reuse) the system prompt for the current segment.
    pub fn system(&self, sess: &SessionInfo) -> (String, String, String) {
        let key = format!("{}:{:?}:{}", sess.segment, sess.mode, self.model);
        {
            let c = self.cache.lock();
            if c.key == key && !c.system.is_empty() {
                return (c.system.clone(), c.brief.clone(), c.path_in_system.clone());
            }
        }
        let cfg = &self.config;
        let brief = if cfg.compaction.include_repo_map {
            let focus: Vec<String> = self.state.lock().touched.keys().cloned().collect();
            self.repo_map.as_ref().map(|f| f(cfg.token_saving.repo_map_tokens, &focus)).unwrap_or_default()
        } else {
            String::new()
        };
        let path_md = if sess.segment == 0 { compaction::read_path_md(&self.root(), &cfg.compaction) } else { String::new() };
        let cwd = self.state.lock().cwd.clone().unwrap_or_else(|| self.root());
        let shell = self.shell_name();
        let env = Env {
            project_root: &self.project.root,
            extra_roots: &self.project.extra_roots,
            cwd: &cwd.to_string_lossy(),
            shell: &shell,
        };
        let sys = context::system_prompt(cfg, sess.mode, &env, &brief, Some(&path_md));
        let mut c = self.cache.lock();
        c.key = key;
        c.system = sys.clone();
        c.brief = brief.clone();
        c.path_in_system = path_md.clone();
        if c.calib <= 0.0 {
            c.calib = 1.0;
        }
        (sys, brief, path_md)
    }

    fn estimate(&self, system: &str, msgs: &[Message]) -> u64 {
        let specs = self.tools.specs();
        let raw = crate::tokens::count(system)
            + context::tools_tokens(&specs)
            + msgs.iter().map(context::message_tokens).sum::<u64>();
        let calib = self.cache.lock().calib.clamp(0.6, 1.8);
        (raw as f64 * if calib > 0.0 { calib } else { 1.0 }) as u64
    }

    fn update_stats(&self, f: impl FnOnce(&mut LiveStats)) {
        let snapshot = {
            let mut s = self.stats.lock();
            f(&mut s);
            s.clone()
        };
        self.emit(AgentEvent::Stats { session: self.sid(), stats: snapshot });
    }

    fn persist(&self, all: &mut Vec<Message>, m: Message) -> Message {
        let _ = self.store.append_message(&self.session_id, &m);
        self.emit(AgentEvent::Message { session: self.sid(), message: m.clone() });
        all.push(m.clone());
        m
    }

    /// Handle queued items at a tool boundary: user messages are appended, `/compact` runs now.
    /// Commands for the engine stay queued. Returns true if a user message was delivered.
    async fn drain_queue(&self, sess: &mut SessionInfo, all: &mut Vec<Message>) -> bool {
        self.queue.forced.store(false, std::sync::atomic::Ordering::SeqCst);
        let mut delivered = false;
        while let Some(item) = self.queue.pop_inline() {
            self.emit(AgentEvent::Queue { session: self.sid(), items: self.queue.snapshot() });
            match item {
                Queued::Msg(mut m) => {
                    m.segment = sess.segment;
                    m.created_at = chrono::Utc::now().timestamp_millis();
                    self.persist(all, m);
                    delivered = true;
                }
                Queued::Compact { .. } => {
                    let (system, _, _) = self.system(sess);
                    let active = context::active_messages(all, sess.segment);
                    if active.len() <= 1 {
                        continue;
                    }
                    let used = self.estimate(&system, &active);
                    if let Err(e) = self.compact(sess, all, used).await {
                        if !self.cancel.is_cancelled() {
                            self.emit(AgentEvent::Notice { session: self.sid(), text: format!("compaction failed: {e}") });
                        }
                    }
                }
                Queued::Cmd { .. } => {}
            }
        }
        delivered
    }

    /// Main entry: optionally append a user message, then loop until done.
    pub async fn run(&self, input: Option<Message>) -> Result<RunOutcome> {
        let run_start = Instant::now();
        self.user_stopped.store(false, std::sync::atomic::Ordering::SeqCst);
        let mut sess = self.session()?;
        let mut all = self.store.messages(&self.session_id)?;
        self.state.lock().segment = sess.segment;
        {
            let mut st = self.state.lock();
            st.turn += 1;
            st.undo.clear();
        }
        let limit = self.context_limit();
        self.update_stats(|s| {
            s.context_limit = limit;
            s.model = self.model.clone();
            s.gateway = self.gateway.name.clone();
            s.steps = 0;
            s.elapsed_ms = 0;
            s.total_work_ms = sess.total_work_ms;
            s.compactions = sess.compactions;
        });

        if let Some(mut m) = input {
            m.segment = sess.segment;
            self.persist(&mut all, m);
        }

        let mut empty_nudges = 0;
        let mut errors = 0;
        let mut overflow_retries = 0;
        let mut stopped = false;

        loop {
            if self.cancel.is_cancelled() {
                stopped = true;
                break;
            }
            // ---- silent self-compaction
            let (system, _, _) = self.system(&sess);
            let active = context::active_messages(&all, sess.segment);
            let used = self.estimate(&system, &active);
            self.update_stats(|s| s.context_used = used);
            if self.config.compaction.enabled && used >= self.config.threshold(limit) && active.len() > 2 {
                if let Err(e) = self.compact(&mut sess, &mut all, used).await {
                    if self.cancel.is_cancelled() {
                        stopped = true;
                        break;
                    }
                    self.emit(AgentEvent::Error { session: self.sid(), text: format!("compaction failed: {e}") });
                    break;
                }
                continue;
            }

            // ---- model call
            let msgs = context::shape_messages(&active, &self.config);
            let req = ChatRequest {
                model: self.model.clone(),
                system: system.clone(),
                messages: msgs,
                tools: self.tools.specs(),
                generation: self.config.generation.clone(),
                no_tools: false,
                max_tokens: None,
            };
            let turn_id = uuid::Uuid::new_v4().to_string();
            self.emit(AgentEvent::TurnStart { session: self.sid(), id: turn_id.clone() });
            let turn_token = self.cancel.child_token();
            *self.queue.turn.lock() = Some(turn_token.clone());
            let partial = Arc::new(Mutex::new((String::new(), String::new())));
            let res = self.stream(&req, &turn_id, &turn_token, partial.clone()).await;
            *self.queue.turn.lock() = None;
            let comp = match res {
                Ok(c) => c,
                Err(e) => {
                    if self.cancel.is_cancelled() {
                        stopped = true;
                        break;
                    }
                    if turn_token.is_cancelled() {
                        // Steer: keep what was generated so far, then deliver the queue.
                        let (text, thinking) = partial.lock().clone();
                        let mut parts = vec![];
                        if !thinking.trim().is_empty() {
                            parts.push(Part::Thinking { text: thinking.trim().to_string() });
                        }
                        if !text.trim().is_empty() {
                            parts.push(Part::Text { text: format!("{} [interrupted]", text.trim()) });
                        }
                        let mut msg = Message::new(Role::Assistant, parts);
                        msg.id = turn_id.clone();
                        msg.segment = sess.segment;
                        msg.meta = Some(Default::default());
                        if !msg.parts.is_empty() {
                            let _ = self.store.append_message(&self.session_id, &msg);
                            all.push(msg.clone());
                        }
                        self.emit(AgentEvent::TurnEnd { session: self.sid(), message: msg, meta: Default::default() });
                        self.drain_queue(&mut sess, &mut all).await;
                        continue;
                    }
                    if e.downcast_ref::<ContextOverflow>().is_some() && overflow_retries < 2 {
                        overflow_retries += 1;
                        let used = self.estimate(&system, &active).max(limit);
                        if self.compact(&mut sess, &mut all, used).await.is_ok() {
                            continue;
                        }
                    }
                    errors += 1;
                    if errors <= 3 {
                        self.emit(AgentEvent::Notice { session: self.sid(), text: format!("retrying after error: {e}") });
                        tokio::select! {
                            _ = self.cancel.cancelled() => { stopped = true; break; }
                            _ = tokio::time::sleep(Duration::from_secs(2u64.pow(errors))) => {}
                        }
                        continue;
                    }
                    self.emit(AgentEvent::Error { session: self.sid(), text: e.to_string() });
                    break;
                }
            };
            errors = 0;
            overflow_retries = 0;

            // Calibrate the estimator with the server's real prompt count.
            if comp.meta.prompt_tokens > 0 {
                let est = self.estimate(&system, &active) as f64 / self.cache.lock().calib.clamp(0.6, 1.8).max(0.01);
                if est > 500.0 {
                    let mut c = self.cache.lock();
                    c.calib = (comp.meta.prompt_tokens as f64 / est).clamp(0.6, 1.8);
                    c.last_used = comp.meta.prompt_tokens + comp.meta.completion_tokens;
                }
            }

            let mut parts = comp.parts.clone();
            // Text tool-call fallback for models without native function calling.
            if !parts.iter().any(|p| matches!(p, Part::ToolCall { .. })) {
                let names: Vec<String> = self.tools.tools.iter().map(|t| t.name().to_string()).collect();
                if let Some(i) = parts.iter().position(|p| matches!(p, Part::Text { .. })) {
                    if let Part::Text { text } = &parts[i] {
                        if text.contains("<tool_call>") || text.contains("<function=") {
                            let (rest, calls) = provider::textcalls::extract(text, &names);
                            if !calls.is_empty() {
                                if rest.is_empty() {
                                    parts.remove(i);
                                } else {
                                    parts[i] = Part::Text { text: rest };
                                }
                                parts.extend(calls);
                            }
                        }
                    }
                }
            }
            let mut msg = Message::new(Role::Assistant, parts);
            msg.id = turn_id.clone();
            msg.segment = sess.segment;
            let mut meta = comp.meta.clone();
            meta.thinking_signature = comp.thinking_signature.clone();
            msg.meta = Some(meta.clone());
            let _ = self.store.append_message(&self.session_id, &msg);
            all.push(msg.clone());
            self.emit(AgentEvent::TurnEnd { session: self.sid(), message: msg.clone(), meta: meta.clone() });

            sess.tokens_in += meta.prompt_tokens;
            sess.tokens_out += meta.completion_tokens;
            let used_now = meta.prompt_tokens + meta.completion_tokens;
            self.update_stats(|s| {
                s.steps += 1;
                // 0 = not measurable this turn (burst delivery): keep the last reading.
                if meta.decode_tps > 0.0 {
                    s.tps = meta.decode_tps;
                }
                s.ttft_ms = meta.ttft_ms;
                if meta.prefill_tps > 0.0 {
                    s.prefill_tps = meta.prefill_tps;
                }
                s.tokens_in = sess.tokens_in;
                s.tokens_out = sess.tokens_out;
                if used_now > 0 {
                    s.context_used = used_now;
                }
                s.elapsed_ms = run_start.elapsed().as_millis() as u64;
            });
            self.state.lock().step += 1;

            let calls = msg.tool_calls();
            if calls.is_empty() && self.drain_queue(&mut sess, &mut all).await {
                continue;
            }
            if calls.is_empty() {
                let has_text = !msg.text().trim().is_empty();
                if !has_text && empty_nudges < 2 {
                    empty_nudges += 1;
                    let nudge = if comp.finish_reason == "length" {
                        "Output limit hit. Continue; be more concise."
                    } else {
                        "Continue: call a tool or give the final answer."
                    };
                    let mut m = Message::user(nudge).with_kind(MsgKind::Note);
                    m.segment = sess.segment;
                    self.persist(&mut all, m);
                    continue;
                }
                // ---- goal hook
                if let Some(goal) = sess.goal.clone().filter(|g| !g.trim().is_empty()) {
                    if self.config.tools.goal_judge && !self.user_stopped.load(std::sync::atomic::Ordering::SeqCst) {
                        match self.judge_goal(&goal, &all, &sess).await {
                            Ok((true, reason)) => {
                                self.emit(AgentEvent::GoalCheck { session: self.sid(), done: true, reason });
                            }
                            Ok((false, reason)) => {
                                self.emit(AgentEvent::GoalCheck { session: self.sid(), done: false, reason: reason.clone() });
                                let mut m = Message::user(format!(
                                    "Goal not complete: {reason}\nKeep working toward the goal: {goal}"
                                ))
                                .with_kind(MsgKind::Goal);
                                m.segment = sess.segment;
                                self.persist(&mut all, m);
                                continue;
                            }
                            Err(e) => {
                                if self.cancel.is_cancelled() {
                                    stopped = true;
                                }
                                self.emit(AgentEvent::Notice { session: self.sid(), text: format!("goal check failed: {e}") });
                            }
                        }
                    }
                }
                break;
            }
            empty_nudges = 0;

            // ---- tools
            let results = self.run_tools(&calls, sess.mode, &mut sess).await;
            let mut tm = Message::new(Role::Tool, results);
            tm.segment = sess.segment;
            self.persist(&mut all, tm);
            self.drain_queue(&mut sess, &mut all).await;
            if self.cancel.is_cancelled() {
                stopped = true;
                break;
            }
        }

        if stopped {
            self.user_stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let mut sess2 = self.session().unwrap_or(sess.clone());
        sess2.segment = sess.segment;
        sess2.compactions = sess.compactions;
        sess2.state = sess.state.clone();
        sess2.tokens_in = sess.tokens_in;
        sess2.tokens_out = sess.tokens_out;
        sess2.tool_calls = sess.tool_calls;
        sess2.total_work_ms += run_start.elapsed().as_millis() as u64;
        sess2.cwd = self.state.lock().cwd.as_ref().map(|p| p.to_string_lossy().to_string());
        let total = sess2.total_work_ms;
        self.save_session(&mut sess2);
        self.update_stats(|s| {
            s.elapsed_ms = run_start.elapsed().as_millis() as u64;
            s.total_work_ms = total;
            s.rtk_saved = self.state.lock().rtk_saved_tokens;
            s.tps = 0.0;
        });
        Ok(RunOutcome { stopped })
    }

    /// Stream one completion, forwarding deltas as events with live tok/s.
    async fn stream(
        &self,
        req: &ChatRequest,
        turn_id: &str,
        token: &CancellationToken,
        partial: Arc<Mutex<(String, String)>>,
    ) -> Result<provider::Completion> {
        let p = provider::for_gateway(&self.gateway);
        let sid = self.sid();
        let events = self.events.clone();
        let stats = self.stats.clone();
        let start = Instant::now();
        let mut first: Option<Instant> = None;
        let mut n: u64 = 0;
        let mut last_emit = Instant::now();
        let mut call_ids: HashMap<usize, String> = HashMap::new();
        let mut call_args: HashMap<usize, String> = HashMap::new();
        let tid = turn_id.to_string();
        let mut on = |ev: StreamEvent| {
            n += 1;
            if first.is_none() {
                first = Some(Instant::now());
                let ttft = start.elapsed().as_millis() as u64;
                stats.lock().ttft_ms = ttft;
            }
            match ev {
                StreamEvent::Text(t) => {
                    partial.lock().0.push_str(&t);
                    let _ = events.send(AgentEvent::TextDelta { session: sid.clone(), id: tid.clone(), text: t });
                }
                StreamEvent::Thinking(t) => {
                    partial.lock().1.push_str(&t);
                    let _ = events.send(AgentEvent::ThinkingDelta { session: sid.clone(), id: tid.clone(), text: t });
                }
                StreamEvent::ToolCallStart { index, id, name } => {
                    let cid = if id.is_empty() { format!("{tid}-{index}") } else { id };
                    call_ids.insert(index, cid.clone());
                    let _ = events.send(AgentEvent::ToolCallStart { session: sid.clone(), id: tid.clone(), call_id: cid, name });
                }
                StreamEvent::ToolCallDelta { index, delta } => {
                    call_args.entry(index).or_default().push_str(&delta);
                }
            }
            if last_emit.elapsed() >= Duration::from_millis(250) {
                last_emit = Instant::now();
                if let Some(f) = first {
                    let secs = f.elapsed().as_secs_f64();
                    if secs > 0.2 {
                        let snap = {
                            let mut s = stats.lock();
                            s.tps = n as f64 / secs;
                            s.clone()
                        };
                        let _ = events.send(AgentEvent::Stats { session: sid.clone(), stats: snap });
                    }
                }
            }
        };
        let comp = p.complete(req, &mut on, token).await?;
        Ok(comp)
    }

    async fn run_tools(&self, calls: &[(String, String, Value)], mode: Mode, sess: &mut SessionInfo) -> Vec<Part> {
        let ctx = self.tool_ctx();
        let mut results = vec![];
        for (id, name, args) in calls {
            if self.cancel.is_cancelled() {
                results.push(Part::ToolResult { id: id.clone(), name: name.clone(), content: "cancelled by user".into(), is_error: true });
                continue;
            }
            self.emit(AgentEvent::ToolCallArgs { session: self.sid(), id: String::new(), call_id: id.clone(), args: args.clone() });
            let t0 = Instant::now();
            let out = self.run_one(&ctx, name, args, mode).await;
            let ms = t0.elapsed().as_millis() as u64;
            sess.tool_calls += 1;
            let content = safety_cap(&out.content, self.config.token_saving.max_tool_output_tokens * 2);
            self.emit(AgentEvent::ToolResult {
                session: self.sid(),
                call_id: id.clone(),
                name: name.clone(),
                content: content.clone(),
                is_error: out.is_error,
                ms,
            });
            let tc = sess.tool_calls;
            let saved = self.state.lock().rtk_saved_tokens;
            self.update_stats(|s| {
                s.tool_calls = tc;
                s.rtk_saved = saved;
            });
            results.push(Part::ToolResult { id: id.clone(), name: name.clone(), content, is_error: out.is_error });
        }
        results
    }

    async fn run_one(&self, ctx: &ToolCtx, name: &str, args: &Value, mode: Mode) -> crate::tool::ToolOutput {
        use crate::tool::ToolOutput;
        let Some(tool) = self.tools.get(name) else {
            let names: Vec<&str> = self.tools.tools.iter().map(|t| t.name()).collect();
            return ToolOutput::err(format!("unknown tool `{name}`. available: {}", names.join(", ")));
        };
        if let Some(raw) = args.get("_raw") {
            return ToolOutput::err(format!("invalid JSON arguments: {}", context::clip(&raw.to_string(), 200)));
        }
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|p| ctx.resolve(p));
        let ro = tool.read_only(args);
        let info = CallInfo { tool: name, args, read_only: ro, project_root: &ctx.project_root, path: path.as_deref() };
        match permissions::decide(&self.config.permissions, mode, &info) {
            Decision::Allow => {}
            Decision::Deny(why) => return ToolOutput::err(format!("denied: {why}")),
            Decision::Ask(_) => {
                let key = approval_key(name, args);
                if !self.always.lock().contains(&key) {
                    let (req_id, rx) = self.broker.register();
                    self.emit(AgentEvent::PermissionAsk {
                        session: self.sid(),
                        req_id,
                        tool: name.to_string(),
                        summary: tool.summary(args),
                    });
                    let r = tokio::select! {
                        _ = self.cancel.cancelled() => return ToolOutput::err("cancelled by user"),
                        r = rx => r.unwrap_or(PermReply::Deny),
                    };
                    match r {
                        PermReply::Once => {}
                        PermReply::Always => {
                            self.always.lock().insert(key);
                        }
                        PermReply::Deny => return ToolOutput::err("user denied this action; choose another approach or ask"),
                    }
                }
            }
        }
        tokio::select! {
            _ = self.cancel.cancelled() => ToolOutput::err("cancelled by user"),
            o = tool.run(args.clone(), ctx) => o,
        }
    }

    /// Silent self-compaction: ask for a handoff, write PATH.md, restart with a clean seeded context.
    pub async fn compact(&self, sess: &mut SessionInfo, all: &mut Vec<Message>, used: u64) -> Result<()> {
        let cfg = &self.config.compaction;
        let limit = self.context_limit();
        self.emit(AgentEvent::CompactionStart { session: self.sid(), used, limit });
        let (system, _, _) = self.system(sess);
        let active = context::active_messages(all, sess.segment);
        let prompt = cfg
            .prompt
            .replace("{path_lines}", &cfg.path_entry_max_lines.to_string())
            .replace("{state_words}", &cfg.state_max_words.to_string());
        let mut ask = Message::user(prompt).with_kind(MsgKind::Compaction);
        ask.segment = sess.segment;
        let expected = (cfg.path_entry_max_lines as u64 * 22 + cfg.state_max_words as u64 * 2).max(200);
        self.compaction_progress("prefill", 0, expected);

        let handoff = self.request_handoff(&system, &active, &ask, limit).await;
        let handoff = match handoff {
            Ok(h) => h,
            Err(e) => {
                if self.cancel.is_cancelled() {
                    return Err(e);
                }
                self.emit(AgentEvent::Notice { session: self.sid(), text: format!("handoff fallback: {e}") });
                self.fallback_handoff(&active)
            }
        };
        let _ = self.store.append_message(&self.session_id, &ask);
        let mut reply = Message::new(
            Role::Assistant,
            vec![Part::text(format!(
                "<path>\n{}\n</path>\n<state>\n{}\n</state>",
                handoff.path.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"),
                handoff.state
            ))],
        )
        .with_kind(MsgKind::Compaction);
        reply.segment = sess.segment;
        let _ = self.store.append_message(&self.session_id, &reply);

        // PATH.md
        self.compaction_progress("path", expected, expected);
        let root = self.root();
        let existing = compaction::read_path_md(&root, cfg);
        let title = if sess.title.is_empty() { "chat".to_string() } else { context::clip(&sess.title, 40) };
        let header = format!("{} {} #{}", chrono::Local::now().format("%Y-%m-%d %H:%M"), title, sess.segment + 1);
        let new_md = compaction::append_path(&existing, &header, &handoff.path, cfg);
        if let Err(e) = compaction::write_path_md(&root, cfg, &new_md) {
            self.emit(AgentEvent::Notice { session: self.sid(), text: format!("cannot write PATH.md: {e}") });
        }

        // New segment + seed.
        self.compaction_progress("seed", expected, expected);
        let old_segment = sess.segment;
        sess.segment += 1;
        sess.compactions += 1;
        sess.state = handoff.state.clone();
        self.state.lock().segment = sess.segment;

        // What the user wants now, as summarized in the handoff (`user:` line). Verbatim old
        // prompts are not replayed: after a topic switch they would drag the model back.
        let (intent, state_rest) = split_intent(&handoff.state);
        let request = if !cfg.keep_original_request {
            String::new()
        } else if !intent.is_empty() {
            format!("## User wants\n{intent}")
        } else {
            all.iter()
                .rev()
                .find(|m| m.role == Role::User && m.kind == MsgKind::Normal && m.segment == old_segment && m.text().trim().len() > 12)
                .map(|m| format!("## User wants\n{}", context::clip(m.text().trim(), 300)))
                .unwrap_or_default()
        };
        let ws = if cfg.include_working_set {
            let touched: Vec<_> = self.state.lock().touched.values().cloned().collect();
            compaction::working_set(&touched, &root, self.outliner.as_deref(), cfg.working_set_max_files, 1800)
        } else {
            String::new()
        };
        let recent: String = active
            .iter()
            .rev()
            .filter(|m| m.kind == MsgKind::Normal)
            .take(cfg.keep_recent_messages)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|m| context::render_brief(m, 600))
            .collect();
        let seed_text = compaction::render_seed(
            &cfg.seed_template,
            &request,
            &new_md,
            &state_rest,
            sess.goal.as_deref(),
            &ws,
            &recent,
        );
        let mut seed = Message::user(seed_text).with_kind(MsgKind::Seed);
        seed.segment = sess.segment;
        self.persist(all, seed.clone());

        self.cache.lock().key.clear();
        let (system, _, _) = self.system(sess);
        let after = self.estimate(&system, &[seed]);
        let _ = self.store.add_segment(&Segment {
            session_id: self.sid(),
            index: sess.segment,
            path: handoff.path.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"),
            state: handoff.state.clone(),
            tokens_before: used,
            tokens_after: after,
            created_at: chrono::Utc::now().timestamp_millis(),
        });
        self.save_session(sess);
        self.emit(AgentEvent::CompactionDone {
            session: self.sid(),
            segment: sess.segment,
            path: handoff.path.join("\n"),
            state: handoff.state,
            before: used,
            after,
        });
        let c = sess.compactions;
        self.update_stats(|s| {
            s.compactions = c;
            s.context_used = after;
        });
        Ok(())
    }

    fn compaction_progress(&self, stage: &str, tokens: u64, expected: u64) {
        self.emit(AgentEvent::CompactionProgress { session: self.sid(), stage: stage.into(), tokens, expected });
    }

    async fn request_handoff(&self, system: &str, active: &[Message], ask: &Message, limit: u64) -> Result<Handoff> {
        let cfg = &self.config.compaction;
        let mut msgs = context::shape_messages(active, &self.config);
        // If the context is already too full for the request, trim tool results hard.
        let budget = limit.saturating_sub(cfg.reserve_tokens + 600);
        for pass in 0..3 {
            let est = self.estimate(system, &msgs) + context::message_tokens(ask);
            if est <= budget {
                break;
            }
            let keep = [1200usize, 400, 120][pass];
            for m in msgs.iter_mut() {
                for p in &mut m.parts {
                    if let Part::ToolResult { content, .. } = p {
                        if content.len() > keep {
                            *content = format!("{}… [trimmed]", content.chars().take(keep).collect::<String>());
                        }
                    }
                    if let Part::Thinking { .. } = p {
                        *p = Part::Text { text: String::new() };
                    }
                }
            }
            if pass == 2 {
                // Drop the oldest half as a last resort.
                let n = msgs.len() / 2;
                msgs.drain(1..n.max(1));
                while msgs.get(1).map(|m| m.role == Role::Tool).unwrap_or(false) {
                    msgs.remove(1);
                }
            }
        }
        msgs.push(ask.clone());
        let mut generation = self.config.generation.clone();
        generation.extra_body = no_think_extra(&self.gateway.flavor, &generation.extra_body);
        if !generation.effort().is_empty() {
            generation.reasoning_effort = "off".into();
        }
        let req = ChatRequest {
            model: self.model.clone(),
            system: system.to_string(),
            messages: msgs,
            tools: self.tools.specs(),
            generation,
            no_tools: true,
            max_tokens: Some((cfg.reserve_tokens as u32).max(1024) + 2048),
        };
        let p = provider::for_gateway(&self.gateway);
        let mut last_err = anyhow!("empty handoff");
        let expected = (cfg.path_entry_max_lines as u64 * 22 + cfg.state_max_words as u64 * 2).max(200);
        for _ in 0..2 {
            let mut n: u64 = 0;
            let mut last = Instant::now();
            let mut on = |_e: StreamEvent| {
                n += 1;
                if n == 1 || last.elapsed() >= Duration::from_millis(150) {
                    last = Instant::now();
                    self.compaction_progress("handoff", n, expected);
                }
            };
            match p.complete(&req, &mut on, &self.cancel).await {
                Ok(c) => {
                    let text: String = c
                        .parts
                        .iter()
                        .filter_map(|p| if let Part::Text { text } = p { Some(text.as_str()) } else { None })
                        .collect();
                    if let Some(h) = compaction::parse_reply(&text, cfg) {
                        return Ok(h);
                    }
                    last_err = anyhow!("unparseable handoff");
                }
                Err(e) => {
                    if self.cancel.is_cancelled() {
                        return Err(e);
                    }
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    /// Deterministic handoff built from the tool log when the model can't produce one.
    fn fallback_handoff(&self, active: &[Message]) -> Handoff {
        let mut path = vec![];
        let touched: Vec<_> = self.state.lock().touched.values().filter(|t| t.how != "read").cloned().collect();
        if !touched.is_empty() {
            path.push(format!(
                "changed: {}",
                touched.iter().map(|t| t.path.clone()).collect::<Vec<_>>().join(", ")
            ));
        }
        let last_text = active
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant && !m.text().trim().is_empty())
            .map(|m| context::clip(&m.text(), 300))
            .unwrap_or_default();
        if !last_text.is_empty() {
            path.push(format!("last note: {}", context::clip(&last_text, 150)));
        }
        let calls: Vec<String> = active
            .iter()
            .flat_map(|m| m.tool_calls())
            .rev()
            .take(8)
            .map(|(_, n, a)| format!("{n} {}", context::clip(&a.to_string(), 80)))
            .collect();
        let state = format!("now: {last_text}\nrecent calls: {}", calls.join("; "));
        Handoff { path, state }
    }

    async fn judge_goal(&self, goal: &str, all: &[Message], sess: &SessionInfo) -> Result<(bool, String)> {
        let active = context::active_messages(all, sess.segment);
        let recent: String = active
            .iter()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|m| context::render_brief(m, 500))
            .collect();
        let path = compaction::read_path_md(&self.root(), &self.config.compaction);
        let prompt = format!(
            "GOAL:\n{goal}\n\nPROGRESS NOTES:\n{}\n{}\n\nRECENT ACTIVITY:\n{}\n\nIs the goal fully achieved and verified? Answer ONLY with JSON: {{\"done\": true|false, \"reason\": \"<one short sentence: what is missing, or why it is done>\"}}",
            context::clip(&path, 3000),
            context::clip(&sess.state, 1500),
            recent
        );
        let mut generation = self.config.generation.clone();
        generation.extra_body = no_think_extra(&self.gateway.flavor, &generation.extra_body);
        if !generation.effort().is_empty() {
            generation.reasoning_effort = "off".into();
        }
        let req = ChatRequest {
            model: self.model.clone(),
            system: "You are a strict reviewer judging whether an autonomous agent has completely achieved a goal. Unverified claims are not done.".into(),
            messages: vec![Message::user(prompt)],
            tools: vec![],
            generation,
            no_tools: true,
            max_tokens: Some(2048),
        };
        let p = provider::for_gateway(&self.gateway);
        let mut on = |_e: StreamEvent| {};
        let c = p.complete(&req, &mut on, &self.cancel).await?;
        let text: String = c
            .parts
            .iter()
            .filter_map(|p| if let Part::Text { text } = p { Some(text.as_str()) } else { None })
            .collect();
        let v = provider::parse_args(&text);
        let done = v["done"].as_bool().unwrap_or_else(|| text.to_lowercase().contains("\"done\": true"));
        let reason = v["reason"].as_str().unwrap_or("").to_string();
        Ok((done, if reason.is_empty() { context::clip(&text, 200) } else { reason }))
    }
}

/// Disable thinking for utility calls on servers that support chat_template_kwargs.
fn no_think_extra(flavor: &str, extra: &str) -> String {
    let mut v: Value = serde_json::from_str(extra).unwrap_or(json!({}));
    if !v.is_object() {
        v = json!({});
    }
    if matches!(flavor, "llamacpp" | "vllm" | "lmstudio" | "openai-compatible") {
        v["chat_template_kwargs"] = json!({"enable_thinking": false});
    }
    v.to_string()
}

fn approval_key(name: &str, args: &Value) -> String {
    if name == "shell" {
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let first = cmd.split_whitespace().next().unwrap_or("");
        format!("shell:{first}")
    } else {
        name.to_string()
    }
}

fn safety_cap(s: &str, max_tokens: u64) -> String {
    let t = crate::tokens::quick(s);
    if t <= max_tokens.max(500) {
        return s.to_string();
    }
    let keep = (max_tokens.max(500) * 3) as usize / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = {
        let v: Vec<char> = s.chars().collect();
        v[v.len().saturating_sub(keep)..].iter().collect()
    };
    format!("{head}\n…[{} tokens omitted]…\n{tail}", t.saturating_sub(max_tokens))
}

/// Split the handoff's `user:` line (current intent) from the rest of the state.
fn split_intent(state: &str) -> (String, String) {
    let mut intent = String::new();
    let mut rest = vec![];
    for l in state.lines() {
        match l.trim().strip_prefix("user:") {
            Some(v) if intent.is_empty() => intent = v.trim().to_string(),
            _ => rest.push(l),
        }
    }
    (intent, rest.join("\n"))
}

#[cfg(test)]
mod intent_tests {
    #[test]
    fn splits_user_line() {
        let (i, rest) = super::split_intent("user: build the shop site\ngoal: site\nnext: header");
        assert_eq!(i, "build the shop site");
        assert_eq!(rest, "goal: site\nnext: header");
    }
}

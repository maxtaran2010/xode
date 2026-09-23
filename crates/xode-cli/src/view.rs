//! View model: reduces stored messages and live `AgentEvent`s into renderable chat items.
//! Pure (no engine access) so it can be unit tested.
use serde_json::Value;
use std::collections::VecDeque;
use std::time::Instant;
use xode_engine::xode_core::{AgentEvent, LiveStats, Message, MsgKind, Part, Role, TurnMeta};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Ok,
    Err,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User { text: String, pending: bool },
    Text { turn: String, text: String },
    Thinking { turn: String, text: String, started: Option<Instant>, dur_ms: Option<u64>, expanded: bool },
    Tool { call_id: String, name: String, args: Value, state: ToolState, ms: u64, result: String, expanded: bool, started: Option<Instant> },
    /// Compaction divider. `done == false` while compaction is running.
    Compaction { before: Option<u64>, after: Option<u64>, done: bool },
    Goal { text: String },
    Meta(TurnMeta),
    Notice { text: String, level: Level },
}

impl Item {
    pub fn collapsible(&self) -> bool {
        matches!(self, Item::Thinking { .. } | Item::Tool { .. })
    }
    fn turn(&self) -> Option<&str> {
        match self {
            Item::Text { turn, .. } | Item::Thinking { turn, .. } => Some(turn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub item: Item,
    /// Bumped on every change; used by the renderer's line cache.
    pub rev: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Waiting,
    Thinking,
    Writing,
    Tool(String),
    Compacting,
}

impl Activity {
    pub fn label(&self) -> String {
        match self {
            Activity::Idle => String::new(),
            Activity::Waiting => "Waiting…".into(),
            Activity::Thinking => "Thinking…".into(),
            Activity::Writing => "Writing…".into(),
            Activity::Tool(n) => format!("Running {n}…"),
            Activity::Compacting => "Compacting…".into(),
        }
    }
}

pub const SPARK_LEN: usize = 60;

#[derive(Debug)]
pub struct ChatView {
    pub session: String,
    pub items: Vec<Entry>,
    rev: u64,
    pub running: bool,
    pub activity: Activity,
    pub run_start: Option<Instant>,
    pub turns_this_run: u32,
    pub stats: LiveStats,
    pub tps_samples: VecDeque<u64>,
    /// Messages sent while running (steer queue).
    pub queue: Vec<xode_engine::xode_core::event::QueuedMsg>,
    /// Compaction progress: (stage, tokens, expected).
    pub compact: Option<(String, u64, u64)>,
}

impl ChatView {
    pub fn new(session: impl Into<String>) -> Self {
        Self {
            session: session.into(),
            items: Vec::new(),
            rev: 0,
            running: false,
            activity: Activity::Idle,
            run_start: None,
            turns_this_run: 0,
            stats: LiveStats::default(),
            tps_samples: VecDeque::new(),
            queue: Vec::new(),
            compact: None,
        }
    }

    fn next_rev(&mut self) -> u64 {
        self.rev += 1;
        self.rev
    }

    pub fn push(&mut self, item: Item) {
        let rev = self.next_rev();
        self.items.push(Entry { item, rev });
    }

    fn touch(&mut self, idx: usize) {
        let rev = self.next_rev();
        self.items[idx].rev = rev;
    }

    pub fn notice(&mut self, text: impl Into<String>, level: Level) {
        self.push(Item::Notice { text: text.into(), level });
    }

    /// Optimistically show a user message before the engine echoes it.
    pub fn push_user_pending(&mut self, text: &str) {
        self.push(Item::User { text: text.to_string(), pending: true });
    }

    pub fn toggle(&mut self, idx: usize) {
        if let Some(e) = self.items.get_mut(idx) {
            match &mut e.item {
                Item::Thinking { expanded, .. } | Item::Tool { expanded, .. } => *expanded = !*expanded,
                _ => return,
            }
            self.touch(idx);
        }
    }

    pub fn collapsibles(&self) -> Vec<usize> {
        self.items.iter().enumerate().filter(|(_, e)| e.item.collapsible()).map(|(i, _)| i).collect()
    }

    // ---------------------------------------------------------------- history

    /// Rebuild items from a stored session transcript.
    pub fn load(&mut self, messages: &[Message]) {
        self.items.clear();
        for m in messages {
            self.add_message(m, true);
        }
    }

    fn add_message(&mut self, m: &Message, history: bool) {
        match (m.role, m.kind) {
            (_, MsgKind::Compaction) | (_, MsgKind::Note) | (Role::System, _) => {}
            (_, MsgKind::Seed) => {
                if !matches!(self.items.last().map(|e| &e.item), Some(Item::Compaction { .. })) {
                    self.push(Item::Compaction { before: None, after: None, done: true });
                }
            }
            (_, MsgKind::Goal) => self.push(Item::Goal { text: m.text() }),
            (Role::User, _) => {
                // First text part is what the user typed; later parts are attachment payloads.
                let mut text = m
                    .parts
                    .iter()
                    .find_map(|p| if let Part::Text { text } = p { Some(text.clone()) } else { None })
                    .unwrap_or_default();
                let typed = text.clone();
                let extra = m.parts.iter().filter(|p| matches!(p, Part::Text { .. })).count().saturating_sub(1);
                if extra > 0 {
                    text.push_str("\n[attachments]");
                }
                let images = m.parts.iter().filter(|p| matches!(p, Part::Image { .. })).count();
                if images > 0 {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("[{images} image{}]", if images > 1 { "s" } else { "" }));
                }
                if !history {
                    let pending = self.items.iter().rposition(|e| matches!(&e.item, Item::User { pending: true, .. }));
                    if let Some(i) = pending {
                        if let Item::User { text: t, pending } = &mut self.items[i].item {
                            if t.trim() == typed.trim() {
                                *pending = false;
                                self.touch(i);
                                return;
                            }
                        }
                    }
                }
                self.push(Item::User { text, pending: false });
            }
            (Role::Assistant, _) => {
                if !history && self.items.iter().any(|e| e.item.turn() == Some(m.id.as_str())) {
                    return;
                }
                self.add_assistant(m);
                if let Some(meta) = &m.meta {
                    if !m.text().trim().is_empty() {
                        self.push(Item::Meta(meta.clone()));
                    }
                }
            }
            (Role::Tool, _) => {
                for p in &m.parts {
                    if let Part::ToolResult { id, name, content, is_error } = p {
                        self.set_tool_result(id, name, content, *is_error, 0, !history);
                    }
                }
            }
        }
    }

    fn add_assistant(&mut self, m: &Message) {
        for p in &m.parts {
            match p {
                Part::Thinking { text } if !text.trim().is_empty() => self.push(Item::Thinking {
                    turn: m.id.clone(),
                    text: text.clone(),
                    started: None,
                    dur_ms: Some(0),
                    expanded: false,
                }),
                Part::Text { text } if !text.trim().is_empty() => self.push(Item::Text { turn: m.id.clone(), text: text.clone() }),
                Part::ToolCall { id, name, args } => self.push(Item::Tool {
                    call_id: id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                    state: ToolState::Running,
                    ms: 0,
                    result: String::new(),
                    expanded: false,
                    started: None,
                }),
                _ => {}
            }
        }
    }

    fn find_tool(&self, call_id: &str) -> Option<usize> {
        self.items.iter().rposition(|e| matches!(&e.item, Item::Tool { call_id: c, .. } if c == call_id))
    }

    /// `only_if_running`: live Role::Tool messages must not overwrite results already set by events.
    fn set_tool_result(&mut self, call_id: &str, name: &str, content: &str, is_error: bool, ms: u64, only_if_running: bool) {
        let idx = match self.find_tool(call_id) {
            Some(i) => i,
            None => {
                self.push(Item::Tool {
                    call_id: call_id.into(),
                    name: name.into(),
                    args: Value::Null,
                    state: ToolState::Running,
                    ms: 0,
                    result: String::new(),
                    expanded: false,
                    started: None,
                });
                self.items.len() - 1
            }
        };
        if let Item::Tool { state, ms: m, result, .. } = &mut self.items[idx].item {
            if only_if_running && *state != ToolState::Running {
                return;
            }
            *state = if is_error { ToolState::Err } else { ToolState::Ok };
            if ms > 0 || *m == 0 {
                *m = ms;
            }
            *result = content.to_string();
        }
        self.touch(idx);
    }

    /// Close an open (still streaming) thinking block, recording its duration.
    fn close_thinking(&mut self, now: Instant) {
        let mut touched = None;
        for (i, e) in self.items.iter_mut().enumerate().rev().take(8) {
            if let Item::Thinking { started: Some(s), dur_ms: dur @ None, .. } = &mut e.item {
                *dur = Some(now.duration_since(*s).as_millis() as u64);
                touched = Some(i);
            }
        }
        if let Some(i) = touched {
            self.touch(i);
        }
    }

    // ---------------------------------------------------------------- live events

    /// Apply one engine event (already filtered to this session by the caller, but checked again).
    pub fn apply(&mut self, ev: &AgentEvent, now: Instant) {
        if ev.session() != self.session {
            return;
        }
        match ev {
            AgentEvent::Message { message, .. } => {
                self.queue.retain(|q| q.id != message.id);
                if message.role != Role::Assistant {
                    self.close_thinking(now);
                }
                self.add_message(message, false);
            }
            AgentEvent::TurnStart { .. } => {
                self.turns_this_run += 1;
                self.activity = Activity::Waiting;
            }
            AgentEvent::ThinkingDelta { id, text, .. } => {
                self.activity = Activity::Thinking;
                if let Some(e) = self.items.last_mut() {
                    if let Item::Thinking { turn, text: t, dur_ms: None, .. } = &mut e.item {
                        if turn == id {
                            t.push_str(text);
                            let i = self.items.len() - 1;
                            self.touch(i);
                            return;
                        }
                    }
                }
                self.push(Item::Thinking { turn: id.clone(), text: text.clone(), started: Some(now), dur_ms: None, expanded: false });
            }
            AgentEvent::TextDelta { id, text, .. } => {
                self.close_thinking(now);
                self.activity = Activity::Writing;
                if let Some(e) = self.items.last_mut() {
                    if let Item::Text { turn, text: t } = &mut e.item {
                        if turn == id {
                            t.push_str(text);
                            let i = self.items.len() - 1;
                            self.touch(i);
                            return;
                        }
                    }
                }
                if text.trim().is_empty() {
                    return;
                }
                self.push(Item::Text { turn: id.clone(), text: text.trim_start_matches('\n').to_string() });
            }
            AgentEvent::ToolCallStart { call_id, name, .. } => {
                self.close_thinking(now);
                self.activity = Activity::Tool(name.clone());
                if self.find_tool(call_id).is_none() {
                    self.push(Item::Tool {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        args: Value::Null,
                        state: ToolState::Running,
                        ms: 0,
                        result: String::new(),
                        expanded: false,
                        started: Some(now),
                    });
                }
            }
            AgentEvent::ToolCallArgs { call_id, args, .. } => {
                if let Some(i) = self.find_tool(call_id) {
                    if let Item::Tool { args: a, .. } = &mut self.items[i].item {
                        *a = args.clone();
                    }
                    self.touch(i);
                }
            }
            AgentEvent::ToolResult { call_id, name, content, is_error, ms, .. } => {
                self.set_tool_result(call_id, name, content, *is_error, *ms, false);
                self.activity = Activity::Waiting;
            }
            AgentEvent::TurnEnd { message, meta, .. } => {
                self.close_thinking(now);
                let known = self.items.iter().any(|e| e.item.turn() == Some(message.id.as_str()));
                if !known {
                    self.add_assistant(message);
                } else {
                    // Tool calls that were not streamed as ToolCallStart.
                    for (id, name, args) in message.tool_calls() {
                        if self.find_tool(&id).is_none() {
                            self.push(Item::Tool {
                                call_id: id,
                                name,
                                args,
                                state: ToolState::Running,
                                ms: 0,
                                result: String::new(),
                                expanded: false,
                                started: Some(now),
                            });
                        }
                    }
                }
                if !message.text().trim().is_empty() {
                    self.push(Item::Meta(meta.clone()));
                }
                self.activity = Activity::Waiting;
            }
            AgentEvent::Stats { stats, .. } => {
                if stats.tps > 0.0 {
                    self.tps_samples.push_back(stats.tps.round() as u64);
                    while self.tps_samples.len() > SPARK_LEN {
                        self.tps_samples.pop_front();
                    }
                }
                self.stats = stats.clone();
            }
            AgentEvent::CompactionStart { .. } => {
                self.close_thinking(now);
                self.activity = Activity::Compacting;
                self.compact = Some(("prefill".into(), 0, 0));
                self.push(Item::Compaction { before: None, after: None, done: false });
            }
            AgentEvent::CompactionProgress { stage, tokens, expected, .. } => {
                self.activity = Activity::Compacting;
                self.compact = Some((stage.clone(), *tokens, *expected));
            }
            AgentEvent::CompactionDone { before, after, .. } => {
                self.compact = None;
                let open = self.items.iter().rposition(|e| matches!(e.item, Item::Compaction { done: false, .. }));
                match open {
                    Some(i) => {
                        self.items[i].item = Item::Compaction { before: Some(*before), after: Some(*after), done: true };
                        self.touch(i);
                    }
                    None => self.push(Item::Compaction { before: Some(*before), after: Some(*after), done: true }),
                }
                self.activity = Activity::Waiting;
            }
            AgentEvent::GoalCheck { done, reason, .. } => {
                if *done {
                    self.notice(format!("goal met: {reason}"), Level::Info);
                } else {
                    self.notice(format!("goal not met: {reason}"), Level::Warn);
                }
            }
            AgentEvent::PermissionAsk { .. } => {}
            AgentEvent::Queue { items, .. } => self.queue = items.clone(),
            AgentEvent::CommandDone { .. } | AgentEvent::Finished { .. } | AgentEvent::KbProgress { .. } => {}
            AgentEvent::State { running, .. } => {
                if *running && !self.running {
                    self.run_start = Some(now);
                    self.turns_this_run = 0;
                    self.activity = Activity::Waiting;
                }
                self.running = *running;
                if !*running {
                    self.close_thinking(now);
                    self.activity = Activity::Idle;
                    self.run_start = None;
                    for i in 0..self.items.len() {
                        if let Item::Tool { state: s @ ToolState::Running, result, .. } = &mut self.items[i].item {
                            if result.is_empty() {
                                *s = ToolState::Err;
                                *result = "interrupted".into();
                                self.touch(i);
                            }
                        }
                    }
                    for i in 0..self.items.len() {
                        if let Item::Compaction { done: d @ false, .. } = &mut self.items[i].item {
                            *d = true;
                            self.touch(i);
                        }
                    }
                }
            }
            AgentEvent::Notice { text, .. } => self.notice(text.clone(), Level::Info),
            AgentEvent::Error { text, .. } => self.notice(text.clone(), Level::Error),
        }
    }
}

/// One-line summary of a tool call: `read src/main.rs`, `shell cargo test`, `grep foo`.
pub fn tool_summary(name: &str, args: &Value) -> String {
    const KEYS: &[&str] = &[
        "path", "file_path", "file", "command", "cmd", "pattern", "query", "url", "symbol", "name", "action", "paths",
    ];
    let mut detail = String::new();
    if let Value::Object(map) = args {
        for k in KEYS {
            if let Some(v) = map.get(*k) {
                detail = match v {
                    Value::String(s) => s.clone(),
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
                    Value::Null => continue,
                    other => other.to_string(),
                };
                if !detail.is_empty() {
                    break;
                }
            }
        }
        if detail.is_empty() {
            if let Some((_, v)) = map.iter().find(|(_, v)| v.is_string()) {
                detail = v.as_str().unwrap_or_default().to_string();
            }
        }
    }
    let detail = detail.lines().next().unwrap_or("").trim().to_string();
    if detail.is_empty() {
        name.to_string()
    } else {
        format!("{name} {detail}")
    }
}

/// Rough token estimate for display.
pub fn est_tokens(s: &str) -> u64 {
    (s.len() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const S: &str = "s1";

    fn ev_text(id: &str, t: &str) -> AgentEvent {
        AgentEvent::TextDelta { session: S.into(), id: id.into(), text: t.into() }
    }
    fn ev_think(id: &str, t: &str) -> AgentEvent {
        AgentEvent::ThinkingDelta { session: S.into(), id: id.into(), text: t.into() }
    }
    fn assistant(id: &str, parts: Vec<Part>) -> Message {
        let mut m = Message::new(Role::Assistant, parts);
        m.id = id.into();
        m
    }
    fn feed(v: &mut ChatView, evs: Vec<AgentEvent>) {
        let now = Instant::now();
        for e in evs {
            v.apply(&e, now);
        }
    }

    #[test]
    fn streaming_turn_reduces_to_items() {
        let mut v = ChatView::new(S);
        v.push_user_pending("fix it");
        let user = Message::user("fix it");
        let final_msg = assistant(
            "t1",
            vec![
                Part::Thinking { text: "hmm ok".into() },
                Part::text("Let me look."),
                Part::ToolCall { id: "c1".into(), name: "read".into(), args: json!({"path":"src/main.rs"}) },
            ],
        );
        feed(
            &mut v,
            vec![
                AgentEvent::State { session: S.into(), running: true },
                AgentEvent::Message { session: S.into(), message: user },
                AgentEvent::TurnStart { session: S.into(), id: "t1".into() },
                ev_think("t1", "hmm"),
                ev_think("t1", " ok"),
                ev_text("t1", "Let me"),
                ev_text("t1", " look."),
                AgentEvent::ToolCallStart { session: S.into(), id: "t1".into(), call_id: "c1".into(), name: "read".into() },
                AgentEvent::ToolCallArgs { session: S.into(), id: "t1".into(), call_id: "c1".into(), args: json!({"path":"src/main.rs"}) },
                AgentEvent::TurnEnd { session: S.into(), message: final_msg, meta: TurnMeta { duration_ms: 1000, ..Default::default() } },
                AgentEvent::ToolResult { session: S.into(), call_id: "c1".into(), name: "read".into(), content: "fn main(){}".into(), is_error: false, ms: 12 },
                AgentEvent::State { session: S.into(), running: false },
            ],
        );
        let items: Vec<&Item> = v.items.iter().map(|e| &e.item).collect();
        assert_eq!(items.len(), 5, "{items:#?}");
        assert!(matches!(items[0], Item::User { text, pending: false } if text == "fix it"));
        assert!(matches!(items[1], Item::Thinking { text, dur_ms: Some(_), expanded: false, .. } if text == "hmm ok"));
        assert!(matches!(items[2], Item::Text { text, .. } if text == "Let me look."));
        match items[3] {
            Item::Tool { name, args, state, ms, result, .. } => {
                assert_eq!(tool_summary(name, args), "read src/main.rs");
                assert_eq!(*state, ToolState::Ok);
                assert_eq!(*ms, 12);
                assert_eq!(result, "fn main(){}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(items[4], Item::Meta(m) if m.duration_ms == 1000));
        assert!(!v.running);
        assert_eq!(v.activity, Activity::Idle);
    }

    #[test]
    fn activity_tracks_latest_event() {
        let mut v = ChatView::new(S);
        feed(&mut v, vec![AgentEvent::State { session: S.into(), running: true }, ev_think("t", "a")]);
        assert_eq!(v.activity, Activity::Thinking);
        assert!(v.run_start.is_some());
        feed(&mut v, vec![AgentEvent::ToolCallStart { session: S.into(), id: "t".into(), call_id: "c".into(), name: "shell".into() }]);
        assert_eq!(v.activity, Activity::Tool("shell".into()));
        feed(&mut v, vec![AgentEvent::CompactionStart { session: S.into(), used: 80000, limit: 76800 }]);
        assert_eq!(v.activity, Activity::Compacting);
    }

    #[test]
    fn compaction_and_other_sessions() {
        let mut v = ChatView::new(S);
        feed(
            &mut v,
            vec![
                ev_text("x", "ignored"),
                AgentEvent::TextDelta { session: "other".into(), id: "y".into(), text: "nope".into() },
                AgentEvent::CompactionStart { session: S.into(), used: 81200, limit: 76800 },
                AgentEvent::CompactionDone { session: S.into(), segment: 1, path: String::new(), state: String::new(), before: 81200, after: 6400 },
                AgentEvent::Message { session: S.into(), message: Message::user("seed").with_kind(MsgKind::Seed) },
                AgentEvent::Message { session: S.into(), message: Message::user("compact pls").with_kind(MsgKind::Compaction) },
            ],
        );
        let items: Vec<&Item> = v.items.iter().map(|e| &e.item).collect();
        assert_eq!(items.len(), 2);
        assert!(matches!(items[1], Item::Compaction { before: Some(81200), after: Some(6400), done: true }));
    }

    #[test]
    fn cancel_marks_running_tools_interrupted() {
        let mut v = ChatView::new(S);
        feed(
            &mut v,
            vec![
                AgentEvent::State { session: S.into(), running: true },
                AgentEvent::ToolCallStart { session: S.into(), id: "t".into(), call_id: "c".into(), name: "shell".into() },
                AgentEvent::State { session: S.into(), running: false },
            ],
        );
        assert!(matches!(&v.items[0].item, Item::Tool { state: ToolState::Err, result, .. } if result == "interrupted"));
    }

    #[test]
    fn history_pairs_tool_results_and_hides_internal() {
        let mut m_tool = Message::new(
            Role::Tool,
            vec![Part::ToolResult { id: "c1".into(), name: "grep".into(), content: "3 hits".into(), is_error: true }],
        );
        m_tool.id = "r".into();
        let mut a = assistant("a1", vec![Part::text("done"), Part::ToolCall { id: "c1".into(), name: "grep".into(), args: json!({"pattern":"foo"}) }]);
        a.meta = Some(TurnMeta { completion_tokens: 480, ..Default::default() });
        let msgs = vec![
            Message::user("hi"),
            a,
            m_tool,
            Message::user("summarize").with_kind(MsgKind::Compaction),
            Message::user("seed").with_kind(MsgKind::Seed),
            Message::user("keep going").with_kind(MsgKind::Goal),
        ];
        let mut v = ChatView::new(S);
        v.load(&msgs);
        let items: Vec<&Item> = v.items.iter().map(|e| &e.item).collect();
        assert_eq!(items.len(), 6, "{items:#?}");
        assert!(matches!(items[2], Item::Tool { state: ToolState::Err, result, .. } if result == "3 hits"));
        assert!(matches!(items[3], Item::Meta(m) if m.completion_tokens == 480));
        assert!(matches!(items[4], Item::Compaction { before: None, .. }));
        assert!(matches!(items[5], Item::Goal { text } if text == "keep going"));
        assert_eq!(v.collapsibles(), vec![2]);
    }

    #[test]
    fn toggle_bumps_rev() {
        let mut v = ChatView::new(S);
        feed(&mut v, vec![ev_think("t", "a")]);
        let r = v.items[0].rev;
        v.toggle(0);
        assert!(matches!(v.items[0].item, Item::Thinking { expanded: true, .. }));
        assert!(v.items[0].rev > r);
    }

    #[test]
    fn stats_samples_are_bounded() {
        let mut v = ChatView::new(S);
        for i in 0..100 {
            feed(&mut v, vec![AgentEvent::Stats { session: S.into(), stats: LiveStats { tps: i as f64 + 1.0, ..Default::default() } }]);
        }
        assert_eq!(v.tps_samples.len(), SPARK_LEN);
        assert_eq!(v.stats.tps, 100.0);
    }

    #[test]
    fn summaries() {
        assert_eq!(tool_summary("shell", &json!({"command":"cargo test\n--all"})), "shell cargo test");
        assert_eq!(tool_summary("todo", &json!({})), "todo");
    }
}

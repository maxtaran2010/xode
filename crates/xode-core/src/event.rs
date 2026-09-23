use crate::types::{Message, TurnMeta};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Live stats for the speedometer / stats sidebar.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LiveStats {
    pub tps: f64,
    pub ttft_ms: u64,
    pub prefill_tps: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub context_used: u64,
    pub context_limit: u64,
    pub compactions: u32,
    pub steps: u32,
    pub tool_calls: u32,
    pub elapsed_ms: u64,
    pub total_work_ms: u64,
    pub rtk_saved: u64,
    pub model: String,
    pub gateway: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueuedMsg {
    pub id: String,
    pub text: String,
    /// A slash command waiting for its turn (e.g. `/compact`).
    #[serde(default)]
    pub command: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A message was appended (user, seed, tool results ...).
    Message { session: String, message: Message },
    /// Assistant turn started streaming.
    TurnStart { session: String, id: String },
    TextDelta { session: String, id: String, text: String },
    ThinkingDelta { session: String, id: String, text: String },
    ToolCallStart { session: String, id: String, call_id: String, name: String },
    ToolCallArgs { session: String, id: String, call_id: String, args: Value },
    ToolResult { session: String, call_id: String, name: String, content: String, is_error: bool, ms: u64 },
    /// Assistant turn finished; final message committed.
    TurnEnd { session: String, message: Message, meta: TurnMeta },
    Stats { session: String, stats: LiveStats },
    CompactionStart { session: String, used: u64, limit: u64 },
    /// Handoff generation progress: `stage` is prefill | handoff | path | seed.
    CompactionProgress { session: String, stage: String, tokens: u64, expected: u64 },
    CompactionDone { session: String, segment: u32, path: String, state: String, before: u64, after: u64 },
    GoalCheck { session: String, done: bool, reason: String },
    PermissionAsk { session: String, req_id: String, tool: String, summary: String },
    State { session: String, running: bool },
    /// Messages waiting to be delivered to the running agent (sent after the next tool call).
    Queue { session: String, items: Vec<QueuedMsg> },
    /// A queued slash command ran after the run ended; `result` is the engine's CommandResult.
    CommandDone { session: String, result: Value },
    /// Work finished (queue drained). `stopped`: the user stopped it.
    Finished { session: String, stopped: bool, error: bool },
    Notice { session: String, text: String },
    Error { session: String, text: String },
    /// Knowledge-base indexing progress (not tied to a session: `session` is empty).
    /// `source` is a source key (`g:1`) or a store prefix (`g`/`p`) for embedding;
    /// `stage`: scan | index | embed | idle.
    KbProgress { session: String, source: String, stage: String, done: u64, total: u64 },
}

impl AgentEvent {
    pub fn session(&self) -> &str {
        use AgentEvent::*;
        match self {
            Message { session, .. }
            | TurnStart { session, .. }
            | TextDelta { session, .. }
            | ThinkingDelta { session, .. }
            | ToolCallStart { session, .. }
            | ToolCallArgs { session, .. }
            | ToolResult { session, .. }
            | TurnEnd { session, .. }
            | Stats { session, .. }
            | CompactionStart { session, .. }
            | CompactionProgress { session, .. }
            | CompactionDone { session, .. }
            | GoalCheck { session, .. }
            | PermissionAsk { session, .. }
            | State { session, .. }
            | Queue { session, .. }
            | CommandDone { session, .. }
            | Finished { session, .. }
            | Notice { session, .. }
            | Error { session, .. }
            | KbProgress { session, .. } => session,
        }
    }
}

pub type EventTx = tokio::sync::broadcast::Sender<AgentEvent>;

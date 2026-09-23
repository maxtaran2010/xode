use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    Thinking { text: String },
    Image { mime: String, data: String },
    ToolCall { id: String, name: String, args: Value },
    ToolResult { id: String, name: String, content: String, is_error: bool },
}

impl Part {
    pub fn text(s: impl Into<String>) -> Self {
        Part::Text { text: s.into() }
    }
}

/// What a message is for. Hidden messages are sent to the model but not rendered as normal chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MsgKind {
    #[default]
    Normal,
    /// Silent compaction request / reply.
    Compaction,
    /// Rebuilt context seed after compaction.
    Seed,
    /// Auto-continue injected by the goal hook.
    Goal,
    /// Attached file content / system notes.
    Note,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub parts: Vec<Part>,
    #[serde(default)]
    pub kind: MsgKind,
    /// Compaction segment this message belongs to.
    #[serde(default)]
    pub segment: u32,
    #[serde(default)]
    pub meta: Option<TurnMeta>,
    pub created_at: i64,
}

impl Message {
    pub fn new(role: Role, parts: Vec<Part>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role,
            parts,
            kind: MsgKind::Normal,
            segment: 0,
            meta: None,
            created_at: chrono::Utc::now().timestamp_millis(),
        }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![Part::text(text)])
    }
    pub fn with_kind(mut self, k: MsgKind) -> Self {
        self.kind = k;
        self
    }
    pub fn text(&self) -> String {
        let mut s = String::new();
        for p in &self.parts {
            if let Part::Text { text } = p {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(text);
            }
        }
        s
    }
    pub fn tool_calls(&self) -> Vec<(String, String, Value)> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall { id, name, args } => Some((id.clone(), name.clone(), args.clone())),
                _ => None,
            })
            .collect()
    }
}

/// Per-assistant-turn metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TurnMeta {
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub ttft_ms: u64,
    pub duration_ms: u64,
    pub decode_tps: f64,
    pub prefill_tps: f64,
    /// Anthropic thinking signature, required to replay thinking during tool loops.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thinking_signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Normal,
    Plan,
}

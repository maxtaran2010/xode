pub mod anthropic;
pub mod openai;
pub mod sse;
pub mod textcalls;
pub mod think;

use crate::config::{ApiKind, Gateway, Generation};
use crate::types::{Message, Part, ToolSpec, TurnMeta};
use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub generation: Generation,
    /// Force a plain-text reply (compaction, goal judge).
    pub no_tools: bool,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// First byte of any model output (thinking or text or tool call).
    Text(String),
    Thinking(String),
    ToolCallStart { index: usize, id: String, name: String },
    ToolCallDelta { index: usize, delta: String },
}

#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub parts: Vec<Part>,
    pub meta: TurnMeta,
    pub finish_reason: String,
    /// Anthropic thinking signature (needed to send thinking back during tool loops).
    pub thinking_signature: String,
}

pub type OnEvent<'a> = &'a mut (dyn FnMut(StreamEvent) + Send);

#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, req: &ChatRequest, on: OnEvent<'_>, cancel: &CancellationToken) -> Result<Completion>;
}

pub fn for_gateway(g: &Gateway) -> Box<dyn Provider> {
    match g.kind {
        ApiKind::Openai => Box::new(openai::OpenAi::new(g)),
        ApiKind::Anthropic => Box::new(anthropic::Anthropic::new(g)),
    }
}

pub fn http_client(timeout_s: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(timeout_s.max(30)))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap_or_default()
}

/// Normalizes a base url: strips trailing `/` and a trailing `/v1`.
/// Tokens per second, or 0 when the window is too short to mean anything (cloud APIs often
/// deliver a whole turn in one burst, which would otherwise read as millions of tok/s).
pub fn rate(tokens: u64, secs: f64) -> f64 {
    if secs >= 0.25 {
        tokens as f64 / secs
    } else {
        0.0
    }
}

pub fn base_url(url: &str) -> String {
    let mut u = url.trim().trim_end_matches('/').to_string();
    if !u.starts_with("http://") && !u.starts_with("https://") {
        u = format!("http://{u}");
    }
    if u.ends_with("/v1") {
        u.truncate(u.len() - 3);
    }
    u
}

/// Merge `extra_body` JSON (if any) into the request body.
pub fn merge_extra(body: &mut serde_json::Value, extra: &str) {
    if extra.trim().is_empty() {
        return;
    }
    if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(extra) {
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in m {
                obj.insert(k, v);
            }
        }
    }
}

/// Assemble streamed tool call fragments.
#[derive(Default)]
pub struct CallAcc {
    pub calls: Vec<(String, String, String)>,
}

impl CallAcc {
    pub fn start(&mut self, index: usize, id: String, name: String) {
        while self.calls.len() <= index {
            self.calls.push(Default::default());
        }
        let c = &mut self.calls[index];
        if !id.is_empty() {
            c.0 = id;
        }
        if !name.is_empty() {
            c.1 = name;
        }
    }
    pub fn delta(&mut self, index: usize, d: &str) {
        while self.calls.len() <= index {
            self.calls.push(Default::default());
        }
        self.calls[index].2.push_str(d);
    }
    pub fn into_parts(self) -> Vec<Part> {
        self.calls
            .into_iter()
            .filter(|c| !c.1.is_empty())
            .map(|(id, name, args)| {
                let id = if id.is_empty() { format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]) } else { id };
                Part::ToolCall { id, name, args: parse_args(&args) }
            })
            .collect()
    }
}

/// Lenient JSON args parsing (local models sometimes emit trailing junk or single quotes).
pub fn parse_args(s: &str) -> serde_json::Value {
    let t = s.trim();
    if t.is_empty() {
        return serde_json::json!({});
    }
    if let Ok(v) = serde_json::from_str(t) {
        return v;
    }
    // Trim to last closing brace.
    if let (Some(a), Some(b)) = (t.find('{'), t.rfind('}')) {
        if let Ok(v) = serde_json::from_str(&t[a..=b]) {
            return v;
        }
    }
    serde_json::json!({ "_raw": t })
}

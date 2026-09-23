use super::openai::{is_overflow_msg, ContextOverflow};
use super::sse::read_sse;
use super::*;
use crate::config::Gateway;
use crate::types::{Message, Part, Role};
use anyhow::{anyhow, bail};
use serde_json::{json, Value};
use std::time::Instant;

pub struct Anthropic {
    base: String,
    key: String,
}

impl Anthropic {
    pub fn new(g: &Gateway) -> Self {
        Self { base: base_url(&g.url), key: g.api_key.clone() }
    }
}

fn to_messages(msgs: &[Message], thinking_on: bool) -> (Vec<Value>, String) {
    let mut out: Vec<Value> = vec![];
    let mut extra_system = String::new();
    let mut push = |role: &str, blocks: Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        // Anthropic requires alternating roles: merge consecutive same-role messages.
        if let Some(last) = out.last_mut() {
            if last["role"] == role {
                if let Some(arr) = last["content"].as_array_mut() {
                    arr.extend(blocks);
                    return;
                }
            }
        }
        out.push(json!({"role": role, "content": blocks}));
    };
    for m in msgs {
        match m.role {
            Role::System => {
                extra_system.push_str(&m.text());
                extra_system.push('\n');
            }
            Role::User => {
                let mut b = vec![];
                for p in &m.parts {
                    match p {
                        Part::Text { text } if !text.is_empty() => b.push(json!({"type": "text", "text": text})),
                        Part::Image { mime, data } => b.push(json!({
                            "type": "image", "source": {"type": "base64", "media_type": mime, "data": data}
                        })),
                        _ => {}
                    }
                }
                push("user", b);
            }
            Role::Assistant => {
                let mut b = vec![];
                let sig = m.meta.as_ref().map(|x| x.thinking_signature.clone()).unwrap_or_default();
                for p in &m.parts {
                    match p {
                        Part::Thinking { text } if thinking_on && !sig.is_empty() => {
                            b.push(json!({"type": "thinking", "thinking": text, "signature": sig}))
                        }
                        Part::Text { text } if !text.is_empty() => b.push(json!({"type": "text", "text": text})),
                        Part::ToolCall { id, name, args } => {
                            let input = if args.is_object() { args.clone() } else { json!({}) };
                            b.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
                        }
                        _ => {}
                    }
                }
                push("assistant", b);
            }
            Role::Tool => {
                let b: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::ToolResult { id, content, is_error, .. } => Some(json!({
                            "type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error
                        })),
                        _ => None,
                    })
                    .collect();
                push("user", b);
            }
        }
    }
    (out, extra_system)
}

#[async_trait]
impl Provider for Anthropic {
    async fn complete(&self, req: &ChatRequest, on: OnEvent<'_>, cancel: &CancellationToken) -> Result<Completion> {
        let g = &req.generation;
        let think_budget = g.thinking_budget().map(|b| b.max(1024));
        let (messages, extra_sys) = to_messages(&req.messages, think_budget.is_some());
        let mut system = req.system.clone();
        if !extra_sys.is_empty() {
            system.push_str("\n\n");
            system.push_str(extra_sys.trim());
        }
        let mut max_tokens = req.max_tokens.or(g.max_tokens).unwrap_or(8192);
        let mut body = json!({
            "model": req.model,
            "system": system,
            "messages": messages,
            "stream": true,
        });
        if let Some(b) = think_budget {
            max_tokens = max_tokens.max(b + 4096);
            body["thinking"] = json!({"type": "enabled", "budget_tokens": b});
        } else {
            if let Some(t) = g.temperature {
                body["temperature"] = json!(t);
            }
            if let Some(t) = g.top_p {
                body["top_p"] = json!(t);
            }
            if let Some(t) = g.top_k {
                body["top_k"] = json!(t);
            }
        }
        body["max_tokens"] = json!(max_tokens);
        if !g.stop.is_empty() {
            body["stop_sequences"] = json!(g.stop);
        }
        if !req.tools.is_empty() {
            body["tools"] = json!(req
                .tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
                .collect::<Vec<_>>());
            if req.no_tools {
                body["tool_choice"] = json!({"type": "none"});
            }
        }
        merge_extra(&mut body, &g.extra_body);

        let client = http_client(g.request_timeout_s);
        let mut rb = client
            .post(format!("{}/v1/messages", self.base))
            .header("anthropic-version", "2023-06-01")
            .json(&body);
        if !self.key.is_empty() {
            rb = rb.header("x-api-key", &self.key).bearer_auth(&self.key);
        }
        let start = Instant::now();
        let resp = tokio::select! {
            _ = cancel.cancelled() => bail!("cancelled"),
            r = rb.send() => r?,
        };
        if !resp.status().is_success() {
            let st = resp.status();
            let t = resp.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&t)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
                .unwrap_or(t);
            if is_overflow_msg(&msg) {
                return Err(ContextOverflow(msg).into());
            }
            bail!("HTTP {st}: {}", msg.chars().take(600).collect::<String>());
        }

        let mut first: Option<Instant> = None;
        let mut text = String::new();
        let mut thinking = String::new();
        let mut signature = String::new();
        let mut acc = CallAcc::default();
        // block index -> tool index
        let mut tool_idx: std::collections::HashMap<u64, usize> = Default::default();
        let mut finish = String::new();
        let (mut in_tok, mut out_tok, mut cached) = (0u64, 0u64, 0u64);
        let mut model_name = req.model.clone();

        read_sse(resp, cancel, |_ev, data| {
            let v: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => return Ok(true),
            };
            match v["type"].as_str().unwrap_or("") {
                "message_start" => {
                    let u = &v["message"]["usage"];
                    in_tok = u["input_tokens"].as_u64().unwrap_or(0)
                        + u["cache_read_input_tokens"].as_u64().unwrap_or(0)
                        + u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                    cached = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                    if let Some(m) = v["message"]["model"].as_str() {
                        model_name = m.to_string();
                    }
                }
                "content_block_start" => {
                    let cb = &v["content_block"];
                    if cb["type"] == "tool_use" {
                        let i = tool_idx.len();
                        tool_idx.insert(v["index"].as_u64().unwrap_or(0), i);
                        let id = cb["id"].as_str().unwrap_or("").to_string();
                        let name = cb["name"].as_str().unwrap_or("").to_string();
                        acc.start(i, id.clone(), name.clone());
                        on(StreamEvent::ToolCallStart { index: i, id, name });
                        first.get_or_insert_with(Instant::now);
                    }
                }
                "content_block_delta" => {
                    let d = &v["delta"];
                    first.get_or_insert_with(Instant::now);
                    match d["type"].as_str().unwrap_or("") {
                        "text_delta" => {
                            let s = d["text"].as_str().unwrap_or("");
                            text.push_str(s);
                            on(StreamEvent::Text(s.to_string()));
                        }
                        "thinking_delta" => {
                            let s = d["thinking"].as_str().unwrap_or("");
                            thinking.push_str(s);
                            on(StreamEvent::Thinking(s.to_string()));
                        }
                        "signature_delta" => signature.push_str(d["signature"].as_str().unwrap_or("")),
                        "input_json_delta" => {
                            let bi = v["index"].as_u64().unwrap_or(0);
                            if let Some(&i) = tool_idx.get(&bi) {
                                let s = d["partial_json"].as_str().unwrap_or("");
                                acc.delta(i, s);
                                on(StreamEvent::ToolCallDelta { index: i, delta: s.to_string() });
                            }
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    if let Some(s) = v["delta"]["stop_reason"].as_str() {
                        finish = s.to_string();
                    }
                    if let Some(o) = v["usage"]["output_tokens"].as_u64() {
                        out_tok = o;
                    }
                }
                "message_stop" => return Ok(false),
                "error" => {
                    let m = v["error"]["message"].as_str().unwrap_or("stream error").to_string();
                    if is_overflow_msg(&m) {
                        return Err(ContextOverflow(m).into());
                    }
                    return Err(anyhow!(m));
                }
                _ => {}
            }
            Ok(true)
        })
        .await?;

        let end = Instant::now();
        let mut parts = vec![];
        if !thinking.trim().is_empty() {
            parts.push(Part::Thinking { text: thinking });
        }
        if !text.trim().is_empty() {
            parts.push(Part::Text { text: text.trim().to_string() });
        }
        parts.extend(acc.into_parts());
        let ttft = first.map(|f| f - start).unwrap_or(end - start);
        let decode_s = first.map(|f| (end - f).as_secs_f64()).unwrap_or(0.0);
        let meta = TurnMeta {
            model: model_name,
            prompt_tokens: in_tok,
            completion_tokens: out_tok,
            cached_tokens: cached,
            ttft_ms: ttft.as_millis() as u64,
            duration_ms: (end - start).as_millis() as u64,
            decode_tps: if decode_s > 0.0 { out_tok as f64 / decode_s } else { 0.0 },
            prefill_tps: if ttft.as_secs_f64() > 0.0 { in_tok.saturating_sub(cached) as f64 / ttft.as_secs_f64() } else { 0.0 },
            thinking_signature: signature.clone(),
        };
        Ok(Completion { parts, meta, finish_reason: finish, thinking_signature: signature })
    }
}

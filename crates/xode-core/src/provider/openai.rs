use super::sse::read_sse;
use super::think::{Chunk, ThinkSplitter};
use super::*;
use crate::config::Gateway;
use crate::types::{Message, Part, Role};
use anyhow::{anyhow, bail};
use serde_json::{json, Value};
use std::time::Instant;

pub struct OpenAi {
    base: String,
    key: String,
    flavor: String,
}

impl OpenAi {
    pub fn new(g: &Gateway) -> Self {
        Self { base: base_url(&g.url), key: g.api_key.clone(), flavor: g.flavor.clone() }
    }
}

#[derive(Debug)]
pub struct ContextOverflow(pub String);
impl std::fmt::Display for ContextOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "context overflow: {}", self.0)
    }
}
impl std::error::Error for ContextOverflow {}

pub fn is_overflow_msg(s: &str) -> bool {
    let l = s.to_lowercase();
    (l.contains("context") && (l.contains("exceed") || l.contains("too long") || l.contains("overflow")))
        || l.contains("maximum context length")
        || l.contains("prompt is too long")
        || l.contains("n_ctx")
}

pub fn to_openai_messages(system: &str, msgs: &[Message], keep_thinking: bool) -> Vec<Value> {
    let mut out = vec![];
    if !system.is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }
    for m in msgs {
        match m.role {
            Role::System => out.push(json!({"role": "system", "content": m.text()})),
            Role::User => {
                let has_img = m.parts.iter().any(|p| matches!(p, Part::Image { .. }));
                if has_img {
                    let mut arr = vec![];
                    for p in &m.parts {
                        match p {
                            Part::Text { text } => arr.push(json!({"type": "text", "text": text})),
                            Part::Image { mime, data } => arr.push(json!({
                                "type": "image_url",
                                "image_url": {"url": format!("data:{mime};base64,{data}")}
                            })),
                            _ => {}
                        }
                    }
                    out.push(json!({"role": "user", "content": arr}));
                } else {
                    out.push(json!({"role": "user", "content": m.text()}));
                }
            }
            Role::Assistant => {
                let mut o = json!({"role": "assistant", "content": m.text()});
                if keep_thinking {
                    let th: String = m
                        .parts
                        .iter()
                        .filter_map(|p| if let Part::Thinking { text } = p { Some(text.as_str()) } else { None })
                        .collect();
                    if !th.is_empty() {
                        o["reasoning_content"] = json!(th);
                    }
                }
                let calls: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::ToolCall { id, name, args } => Some(json!({
                            "id": id, "type": "function",
                            "function": {"name": name, "arguments": args.to_string()}
                        })),
                        _ => None,
                    })
                    .collect();
                if !calls.is_empty() {
                    o["tool_calls"] = json!(calls);
                }
                out.push(o);
            }
            Role::Tool => {
                for p in &m.parts {
                    if let Part::ToolResult { id, content, .. } = p {
                        out.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
                    }
                }
            }
        }
    }
    out
}

pub fn gen_params(body: &mut Value, g: &crate::config::Generation, max_tokens: Option<u32>) {
    let set = |b: &mut Value, k: &str, v: Value| {
        b[k] = v;
    };
    if let Some(v) = g.temperature {
        set(body, "temperature", json!(v));
    }
    if let Some(v) = g.top_p {
        set(body, "top_p", json!(v));
    }
    if let Some(v) = g.top_k {
        set(body, "top_k", json!(v));
    }
    if let Some(v) = g.min_p {
        set(body, "min_p", json!(v));
    }
    if let Some(v) = g.repeat_penalty {
        set(body, "repeat_penalty", json!(v));
    }
    if let Some(v) = g.presence_penalty {
        set(body, "presence_penalty", json!(v));
    }
    if let Some(v) = g.frequency_penalty {
        set(body, "frequency_penalty", json!(v));
    }
    if let Some(v) = max_tokens.or(g.max_tokens) {
        set(body, "max_tokens", json!(v));
    }
    if let Some(v) = g.seed {
        set(body, "seed", json!(v));
    }
    if !g.stop.is_empty() {
        set(body, "stop", json!(g.stop));
    }
}

/// Maps the effort setting onto what each server family understands.
/// Local servers (llama.cpp, vLLM, LM Studio, Ollama) toggle thinking through the chat template;
/// OpenAI-style servers take `reasoning_effort`.
pub fn effort_params(body: &mut Value, g: &crate::config::Generation, flavor: &str) {
    let effort = g.effort();
    if effort.is_empty() {
        return;
    }
    let local = matches!(flavor, "llamacpp" | "vllm" | "lmstudio" | "ollama" | "openai-compatible" | "");
    if local {
        let kw = &mut body["chat_template_kwargs"];
        if !kw.is_object() {
            *kw = json!({});
        }
        if effort == "off" {
            kw["enable_thinking"] = json!(false);
            kw["thinking"] = json!(false);
        } else {
            kw["enable_thinking"] = json!(true);
            kw["thinking"] = json!(true);
            kw["reasoning_effort"] = json!(effort);
            body["reasoning_effort"] = json!(effort);
            if let Some(b) = g.thinking_budget() {
                body["thinking_budget_tokens"] = json!(b);
            }
        }
        if flavor == "ollama" {
            body["think"] = json!(effort != "off");
        }
    } else {
        body["reasoning_effort"] = json!(if effort == "off" { "minimal" } else { effort });
    }
}

#[async_trait]
impl Provider for OpenAi {
    async fn complete(&self, req: &ChatRequest, on: OnEvent<'_>, cancel: &CancellationToken) -> Result<Completion> {
        let mut body = json!({
            "model": req.model,
            "messages": to_openai_messages(&req.system, &req.messages, req.generation.keep_thinking),
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if !req.tools.is_empty() {
            body["tools"] = json!(req
                .tools
                .iter()
                .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
                .collect::<Vec<_>>());
            if req.no_tools {
                body["tool_choice"] = json!("none");
            } else if req.generation.parallel_tool_calls {
                body["parallel_tool_calls"] = json!(true);
            }
        }
        gen_params(&mut body, &req.generation, req.max_tokens);
        effort_params(&mut body, &req.generation, &self.flavor);
        merge_extra(&mut body, &req.generation.extra_body);

        let client = http_client(req.generation.request_timeout_s);
        let mut rb = client.post(format!("{}/v1/chat/completions", self.base)).json(&body);
        if !self.key.is_empty() {
            rb = rb.bearer_auth(&self.key).header("x-api-key", &self.key);
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
        let mut split = ThinkSplitter::default();
        let mut acc = CallAcc::default();
        let mut finish = String::new();
        let mut usage: Option<Value> = None;
        let mut timings: Option<Value> = None;
        let mut deltas: u64 = 0;
        let mut model_name = req.model.clone();

        read_sse(resp, cancel, |_ev, data| {
            if data.trim() == "[DONE]" {
                return Ok(false);
            }
            let v: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => return Ok(true),
            };
            if let Some(e) = v.get("error") {
                let m = e["message"].as_str().unwrap_or("stream error").to_string();
                if is_overflow_msg(&m) {
                    return Err(ContextOverflow(m).into());
                }
                return Err(anyhow!(m));
            }
            if let Some(m) = v["model"].as_str() {
                model_name = m.to_string();
            }
            if v.get("usage").map(|u| !u.is_null()).unwrap_or(false) {
                usage = Some(v["usage"].clone());
            }
            if v.get("timings").is_some() {
                timings = Some(v["timings"].clone());
            }
            let Some(ch) = v["choices"].get(0) else { return Ok(true) };
            if let Some(f) = ch["finish_reason"].as_str() {
                finish = f.to_string();
            }
            let d = &ch["delta"];
            let mut got = false;
            for key in ["reasoning_content", "reasoning"] {
                if let Some(r) = d[key].as_str() {
                    if !r.is_empty() {
                        got = true;
                        thinking.push_str(r);
                        on(StreamEvent::Thinking(r.to_string()));
                    }
                }
            }
            if let Some(c) = d["content"].as_str() {
                if !c.is_empty() {
                    got = true;
                    for chunk in split.push(c) {
                        match chunk {
                            Chunk::Text(s) => {
                                text.push_str(&s);
                                on(StreamEvent::Text(s));
                            }
                            Chunk::Think(s) => {
                                thinking.push_str(&s);
                                on(StreamEvent::Thinking(s));
                            }
                        }
                    }
                }
            }
            if let Some(calls) = d["tool_calls"].as_array() {
                for c in calls {
                    got = true;
                    let idx = c["index"].as_u64().unwrap_or(0) as usize;
                    let id = c["id"].as_str().unwrap_or("").to_string();
                    let name = c["function"]["name"].as_str().unwrap_or("").to_string();
                    if !id.is_empty() || !name.is_empty() {
                        acc.start(idx, id.clone(), name.clone());
                        if !name.is_empty() {
                            on(StreamEvent::ToolCallStart { index: idx, id, name });
                        }
                    }
                    if let Some(a) = c["function"]["arguments"].as_str() {
                        if !a.is_empty() {
                            acc.delta(idx, a);
                            on(StreamEvent::ToolCallDelta { index: idx, delta: a.to_string() });
                        }
                    }
                }
            }
            if got {
                deltas += 1;
                if first.is_none() {
                    first = Some(Instant::now());
                }
            }
            Ok(true)
        })
        .await?;

        for chunk in split.finish() {
            match chunk {
                Chunk::Text(s) => {
                    text.push_str(&s);
                    on(StreamEvent::Text(s));
                }
                Chunk::Think(s) => {
                    thinking.push_str(&s);
                    on(StreamEvent::Thinking(s));
                }
            }
        }

        let end = Instant::now();
        let mut parts = vec![];
        if !thinking.trim().is_empty() {
            parts.push(Part::Thinking { text: thinking.trim().to_string() });
        }
        if !text.trim().is_empty() {
            parts.push(Part::Text { text: text.trim().to_string() });
        }
        parts.extend(acc.into_parts());

        let mut meta = TurnMeta { model: model_name, ..Default::default() };
        let ttft = first.map(|f| f - start).unwrap_or(end - start);
        meta.ttft_ms = ttft.as_millis() as u64;
        meta.duration_ms = (end - start).as_millis() as u64;
        if let Some(u) = &usage {
            meta.prompt_tokens = u["prompt_tokens"].as_u64().unwrap_or(0);
            meta.completion_tokens = u["completion_tokens"].as_u64().unwrap_or(0);
            meta.cached_tokens = u["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
        }
        if meta.completion_tokens == 0 {
            meta.completion_tokens = deltas;
        }
        let decode_s = first.map(|f| (end - f).as_secs_f64()).unwrap_or(0.0);
        meta.decode_tps = match &timings {
            Some(t) if t["predicted_per_second"].is_number() => t["predicted_per_second"].as_f64().unwrap_or(0.0),
            _ => super::rate(meta.completion_tokens, decode_s),
        };
        meta.prefill_tps = match &timings {
            Some(t) if t["prompt_per_second"].is_number() => t["prompt_per_second"].as_f64().unwrap_or(0.0),
            _ => super::rate(meta.prompt_tokens.saturating_sub(meta.cached_tokens), ttft.as_secs_f64()),
        };
        Ok(Completion { parts, meta, finish_reason: finish, thinking_signature: String::new() })
    }
}

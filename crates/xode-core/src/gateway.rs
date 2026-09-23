//! Gateway detection, liveness test and local port scan.
use crate::config::{ApiKind, Gateway, ModelInfo};
use crate::provider::base_url;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const SCAN_PORTS: &[u16] = &[11434, 1234, 8080, 8000, 8001, 5000, 5001, 4000, 3000, 8888, 8081, 7860, 5555, 9000, 30000];

fn client(timeout_ms: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .connect_timeout(Duration::from_millis(timeout_ms.min(3000)))
        .build()
        .unwrap_or_default()
}

async fn get_json(c: &reqwest::Client, url: &str, key: &str) -> Option<Value> {
    let mut rb = c.get(url).header("anthropic-version", "2023-06-01");
    if !key.is_empty() {
        rb = rb.bearer_auth(key).header("x-api-key", key);
    }
    let r = rb.send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json::<Value>().await.ok()
}

fn host_label(base: &str) -> String {
    base.trim_start_matches("http://").trim_start_matches("https://").to_string()
}

/// Detect API kind, flavor and models of a server.
pub async fn detect(url: &str, api_key: &str) -> Result<Gateway> {
    let base = base_url(url);
    let c = client(6000);
    let (u1, u2, u3, u4) =
        (format!("{base}/v1/models"), format!("{base}/props"), format!("{base}/api/tags"), format!("{base}/api/v0/models"));
    let (models, props, tags, lms) =
        tokio::join!(get_json(&c, &u1, api_key), get_json(&c, &u2, api_key), get_json(&c, &u3, api_key), get_json(&c, &u4, api_key));

    let mut g = Gateway { url: base.clone(), api_key: api_key.to_string(), ..Default::default() };
    let is_anthropic_host = base.contains("anthropic.com");

    if let Some(m) = &models {
        let data = m["data"].as_array().cloned().unwrap_or_default();
        let anthropic_shape = m.get("first_id").is_some()
            || m.get("has_more").is_some() && m.get("object").is_none()
            || data.iter().any(|d| d["type"] == "model" && d.get("display_name").is_some());
        g.kind = if is_anthropic_host || anthropic_shape { ApiKind::Anthropic } else { ApiKind::Openai };
        for d in &data {
            let id = d["id"].as_str().unwrap_or("").to_string();
            if id.is_empty() {
                continue;
            }
            let ctx = d["meta"]["n_ctx"]
                .as_u64()
                .or_else(|| d["max_model_len"].as_u64())
                .or_else(|| d["context_length"].as_u64())
                .or_else(|| d["max_context_length"].as_u64())
                .unwrap_or(0);
            g.models.push(ModelInfo { id, context: ctx, vision: false });
        }
        g.flavor = if g.kind == ApiKind::Anthropic {
            "anthropic".into()
        } else if data.iter().any(|d| d["owned_by"] == "llamacpp") || props.is_some() {
            "llamacpp".into()
        } else if data.iter().any(|d| d.get("max_model_len").is_some()) {
            "vllm".into()
        } else if tags.is_some() {
            "ollama".into()
        } else if lms.is_some() {
            "lmstudio".into()
        } else if base.contains("openai.com") {
            "openai".into()
        } else {
            "openai-compatible".into()
        };
    } else if is_anthropic_host {
        g.kind = ApiKind::Anthropic;
        g.flavor = "anthropic".into();
    } else if tags.is_none() && props.is_none() {
        bail!("no compatible API at {base}");
    }

    // llama.cpp: real n_ctx + vision.
    if let Some(p) = &props {
        let n_ctx = p["default_generation_settings"]["n_ctx"].as_u64().or_else(|| p["n_ctx"].as_u64()).unwrap_or(0);
        let vision = p["modalities"]["vision"].as_bool().unwrap_or(false);
        for m in &mut g.models {
            if m.context == 0 {
                m.context = n_ctx;
            }
            m.vision = vision;
        }
        if g.models.is_empty() {
            let alias = p["model_alias"].as_str().or(p["model_path"].as_str()).unwrap_or("default").to_string();
            g.models.push(ModelInfo { id: alias, context: n_ctx, vision });
        }
        g.flavor = "llamacpp".into();
    }

    // Ollama: names + context via /api/show.
    if let Some(t) = &tags {
        if g.flavor.is_empty() || g.flavor == "openai-compatible" {
            g.flavor = "ollama".into();
        }
        let names: Vec<String> =
            t["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(String::from)).collect();
        for n in names {
            let mut ctx = 0;
            let mut vision = false;
            if let Ok(r) = c.post(format!("{base}/api/show")).json(&json!({"model": n})).send().await {
                if let Ok(v) = r.json::<Value>().await {
                    if let Some(mi) = v["model_info"].as_object() {
                        for (k, val) in mi {
                            if k.ends_with(".context_length") {
                                ctx = val.as_u64().unwrap_or(0);
                            }
                        }
                    }
                    vision = v["capabilities"].as_array().map(|a| a.iter().any(|x| x == "vision")).unwrap_or(false);
                }
            }
            match g.models.iter_mut().find(|m| m.id == n) {
                Some(m) => {
                    if m.context == 0 {
                        m.context = ctx;
                    }
                    m.vision |= vision;
                }
                None => g.models.push(ModelInfo { id: n, context: ctx, vision }),
            }
        }
    }

    // LM Studio: loaded context lengths.
    if let Some(l) = &lms {
        for d in l["data"].as_array().into_iter().flatten() {
            let id = d["id"].as_str().unwrap_or("");
            let ctx = d["loaded_context_length"].as_u64().or_else(|| d["max_context_length"].as_u64()).unwrap_or(0);
            let vision = d["type"] == "vlm";
            if let Some(m) = g.models.iter_mut().find(|m| m.id == id) {
                if m.context == 0 {
                    m.context = ctx;
                }
                m.vision |= vision;
            }
        }
        if g.flavor == "openai-compatible" {
            g.flavor = "lmstudio".into();
        }
    }

    if g.kind == ApiKind::Anthropic && g.models.is_empty() {
        for id in ["claude-opus-5-5", "claude-sonnet-5", "claude-haiku-4-5-20251001"] {
            g.models.push(ModelInfo { id: id.into(), context: 200_000, vision: true });
        }
    }
    g.name = format!("{} · {}", if g.flavor.is_empty() { "gateway" } else { &g.flavor }, host_label(&base));
    Ok(g)
}

pub struct TestResult {
    pub ok: bool,
    pub latency_ms: u64,
    pub models: usize,
    pub message: String,
}

/// Aliveness: list models then a 1-token completion.
pub async fn test(g: &Gateway) -> TestResult {
    let t0 = Instant::now();
    let fresh = match detect(&g.url, &g.api_key).await {
        Ok(f) => f,
        Err(e) => return TestResult { ok: false, latency_ms: t0.elapsed().as_millis() as u64, models: 0, message: e.to_string() },
    };
    let list_ms = t0.elapsed().as_millis() as u64;
    let model = g.models.first().or(fresh.models.first()).map(|m| m.id.clone()).unwrap_or_default();
    if model.is_empty() {
        return TestResult { ok: true, latency_ms: list_ms, models: 0, message: "reachable, no models".into() };
    }
    let c = client(30_000);
    let base = base_url(&g.url);
    let t1 = Instant::now();
    let res = match g.kind {
        ApiKind::Openai => {
            let mut rb = c.post(format!("{base}/v1/chat/completions")).json(&json!({
                "model": model, "max_tokens": 1, "messages": [{"role": "user", "content": "hi"}]
            }));
            if !g.api_key.is_empty() {
                rb = rb.bearer_auth(&g.api_key);
            }
            rb.send().await
        }
        ApiKind::Anthropic => {
            c.post(format!("{base}/v1/messages"))
                .header("x-api-key", &g.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&json!({"model": model, "max_tokens": 1, "messages": [{"role": "user", "content": "hi"}]}))
                .send()
                .await
        }
    };
    let gen_ms = t1.elapsed().as_millis() as u64;
    match res {
        Ok(r) if r.status().is_success() => TestResult {
            ok: true,
            latency_ms: gen_ms,
            models: fresh.models.len(),
            message: format!("ok · list {list_ms}ms · gen {gen_ms}ms"),
        },
        Ok(r) => {
            let st = r.status();
            let t = r.text().await.unwrap_or_default();
            TestResult { ok: false, latency_ms: gen_ms, models: fresh.models.len(), message: format!("{st}: {}", t.chars().take(200).collect::<String>()) }
        }
        Err(e) => TestResult { ok: false, latency_ms: gen_ms, models: fresh.models.len(), message: e.to_string() },
    }
}

/// Probe hosts × ports for local model servers.
pub async fn scan(hosts: &[String], ports: &[u16]) -> Vec<Gateway> {
    let mut hs: Vec<String> = vec!["127.0.0.1".into()];
    for h in hosts {
        let h = h.trim();
        if !h.is_empty() && !hs.iter().any(|x| x == h) {
            hs.push(h.to_string());
        }
    }
    let mut futs = vec![];
    for h in &hs {
        for p in ports {
            let addr = format!("{h}:{p}");
            futs.push(async move {
                let ok = tokio::time::timeout(Duration::from_millis(350), tokio::net::TcpStream::connect(&addr))
                    .await
                    .map(|r| r.is_ok())
                    .unwrap_or(false);
                if !ok {
                    return None;
                }
                detect(&format!("http://{addr}"), "").await.ok().filter(|g| !g.models.is_empty())
            });
        }
    }
    futures::future::join_all(futs).await.into_iter().flatten().collect()
}

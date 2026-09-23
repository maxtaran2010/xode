//! Minimal MCP client: JSON-RPC 2.0 over stdio (newline-delimited) and
//! streamable HTTP (JSON or SSE responses). Supports initialize, tools/list,
//! tools/call.

use crate::util::{cap_tokens, clip};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use xode_core::config::McpServer;
use xode_core::tool::{Tool, ToolCtx, ToolOutput, ToolRef};
use xode_core::types::ToolSpec;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const INIT_TIMEOUT: Duration = Duration::from_secs(90);
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
const DESC_MAX: usize = 200;

type Reply = std::result::Result<Value, String>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

// ---------------------------------------------------------------- stdio

pub struct StdioConn {
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    alive: Arc<AtomicBool>,
    child: Mutex<Option<tokio::process::Child>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl StdioConn {
    /// Wire a connection over any reader/writer pair (child pipes or test duplex).
    pub fn from_io<R, W>(reader: R, writer: W) -> StdioConn
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let mut w = writer;
        tokio::spawn(async move {
            while let Some(mut m) = rx.recv().await {
                m.push('\n');
                if w.write_all(m.as_bytes()).await.is_err() || w.flush().await.is_err() {
                    break;
                }
            }
            let _ = w.shutdown().await;
        });
        let (p, a, reply_tx) = (pending.clone(), alive.clone(), tx.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(line) {
                    Ok(Value::Array(batch)) => batch.into_iter().for_each(|m| dispatch(&p, &reply_tx, m)),
                    Ok(m) => dispatch(&p, &reply_tx, m),
                    Err(_) => tracing::debug!("mcp non-json line: {}", clip(line, 200)),
                }
            }
            a.store(false, Ordering::SeqCst);
            for (_, s) in p.lock().drain() {
                let _ = s.send(Err("server closed connection".into()));
            }
        });
        StdioConn { tx, pending, alive, child: Mutex::new(None), stderr: Arc::new(Mutex::new(VecDeque::new())) }
    }

    async fn spawn(s: &McpServer) -> Result<StdioConn> {
        let mut cmd = build_command(s);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().with_context(|| format!("spawn '{}'", s.command))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let conn = StdioConn::from_io(stdout, stdin);
        if let Some(err) = child.stderr.take() {
            let tail = conn.stderr.clone();
            let name = s.name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    tracing::debug!("mcp[{name}] {l}");
                    let mut t = tail.lock();
                    if t.len() >= 20 {
                        t.pop_front();
                    }
                    t.push_back(l);
                }
            });
        }
        *conn.child.lock() = Some(child);
        Ok(conn)
    }

    fn stderr_tail(&self) -> String {
        let t = self.stderr.lock();
        let v: Vec<&str> = t.iter().rev().take(3).map(|s| s.as_str()).collect();
        v.into_iter().rev().collect::<Vec<_>>().join(" | ")
    }

    async fn request(&self, id: u64, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        if !self.alive.load(Ordering::SeqCst) {
            bail!("server not running");
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.tx.send(msg.to_string()).is_err() {
            self.pending.lock().remove(&id);
            bail!("server not running");
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!("{e}")),
            Ok(Err(_)) => bail!("server closed connection"),
            Err(_) => {
                self.pending.lock().remove(&id);
                bail!("{method}: timeout")
            }
        }
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self.tx.send(json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string());
    }

    async fn close(&self) {
        let child = self.child.lock().take();
        if let Some(mut c) = child {
            // Closing stdin asks the server to exit; kill if it lingers.
            drop(c.stdin.take());
            if tokio::time::timeout(Duration::from_millis(800), c.wait()).await.is_err() {
                let _ = c.kill().await;
            }
        }
        self.alive.store(false, Ordering::SeqCst);
    }
}

fn dispatch(p: &Pending, tx: &mpsc::UnboundedSender<String>, m: Value) {
    let has_method = m.get("method").and_then(|x| x.as_str()).is_some();
    match (m.get("id"), has_method) {
        // response
        (Some(id), false) => {
            if let Some(id) = id.as_u64() {
                if let Some(s) = p.lock().remove(&id) {
                    let _ = s.send(rpc_result(&m));
                }
            }
        }
        // server -> client request
        (Some(id), true) => {
            let reply = match m["method"].as_str().unwrap_or("") {
                "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
                "roots/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"roots": []}}),
                _ => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not supported"}}),
            };
            let _ = tx.send(reply.to_string());
        }
        _ => {} // notifications ignored
    }
}

fn rpc_result(m: &Value) -> Reply {
    match m.get("error") {
        Some(e) => Err(format!(
            "{} ({})",
            e.get("message").and_then(|x| x.as_str()).unwrap_or("error"),
            e.get("code").and_then(|x| x.as_i64()).unwrap_or(0)
        )),
        None => Ok(m.get("result").cloned().unwrap_or(Value::Null)),
    }
}

#[cfg(windows)]
fn build_command(s: &McpServer) -> tokio::process::Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let resolved = which_windows(&s.command);
    let direct = resolved
        .as_ref()
        .and_then(|p| p.extension())
        .map(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("com"))
        .unwrap_or(false);
    let mut cmd = if direct {
        let mut c = tokio::process::Command::new(resolved.unwrap());
        c.args(&s.args);
        c
    } else {
        // npx/uvx and other .cmd/.bat shims need cmd.exe.
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/d").arg("/s").arg("/c").arg(&s.command).args(&s.args);
        c
    };
    cmd.envs(&s.env);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(windows)]
fn which_windows(cmd: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let p = Path::new(cmd);
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect();
    let try_file = |base: PathBuf| -> Option<PathBuf> {
        if base.extension().is_some() && base.is_file() {
            return Some(base);
        }
        exts.iter().map(|e| PathBuf::from(format!("{}{e}", base.display()))).find(|c| c.is_file())
    };
    if p.is_absolute() || cmd.contains(['/', '\\']) {
        return try_file(p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|d| try_file(d.join(cmd)))
}

#[cfg(not(windows))]
fn build_command(s: &McpServer) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(&s.command);
    c.args(&s.args).envs(&s.env);
    c
}

// ---------------------------------------------------------------- streamable HTTP

pub struct HttpConn {
    client: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
    session: Mutex<Option<String>>,
    proto: Mutex<Option<String>>,
}

impl HttpConn {
    fn new(s: &McpServer) -> Result<HttpConn> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        Ok(HttpConn {
            client,
            url: s.url.trim().to_string(),
            headers: s.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            session: Mutex::new(None),
            proto: Mutex::new(None),
        })
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .body(body.to_string());
        for (k, v) in &self.headers {
            r = r.header(k, v);
        }
        if let Some(s) = self.session.lock().clone() {
            r = r.header("Mcp-Session-Id", s);
        }
        if let Some(p) = self.proto.lock().clone() {
            r = r.header("MCP-Protocol-Version", p);
        }
        r
    }

    async fn request(&self, id: u64, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let fut = async {
            let resp = self.post(&body).send().await?;
            if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
                *self.session.lock() = Some(s.to_string());
            }
            let status = resp.status();
            if !status.is_success() {
                let t = resp.text().await.unwrap_or_default();
                bail!("HTTP {status}: {}", clip(&t, 200));
            }
            let ctype = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ctype.contains("text/event-stream") {
                let mut stream = resp.bytes_stream();
                let mut buf = String::new();
                while let Some(chunk) = stream.next().await {
                    buf.push_str(&String::from_utf8_lossy(&chunk?));
                    for data in sse_take(&mut buf) {
                        if let Some(r) = match_response(&data, id) {
                            return r.map_err(|e| anyhow!(e));
                        }
                    }
                }
                bail!("stream ended without response")
            } else {
                let t = resp.text().await?;
                match_response(&t, id).unwrap_or_else(|| Err(format!("unexpected response: {}", clip(&t, 200)))).map_err(|e| anyhow!(e))
            }
        };
        tokio::time::timeout(timeout, fut).await.map_err(|_| anyhow!("{method}: timeout"))?
    }

    async fn notify(&self, method: &str, params: Value) {
        let _ = self.post(&json!({"jsonrpc": "2.0", "method": method, "params": params})).send().await;
    }

    async fn close(&self) {
        let sid = self.session.lock().clone();
        if let Some(s) = sid {
            let mut r = self.client.delete(&self.url).header("Mcp-Session-Id", s);
            for (k, v) in &self.headers {
                r = r.header(k, v);
            }
            let _ = tokio::time::timeout(Duration::from_secs(3), r.send()).await;
        }
    }
}

/// Pop complete SSE events from `buf`, returning their `data` payloads.
pub fn sse_take(buf: &mut String) -> Vec<String> {
    if buf.contains('\r') {
        *buf = buf.replace("\r\n", "\n").replace('\r', "\n");
    }
    let mut out = vec![];
    while let Some(i) = buf.find("\n\n") {
        let ev: String = buf.drain(..i + 2).collect();
        let data: Vec<&str> = ev
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(|d| d.strip_prefix(' ').unwrap_or(d))
            .collect();
        if !data.is_empty() {
            out.push(data.join("\n"));
        }
    }
    out
}

/// If `text` is (or contains, for batches) the response to `id`, return it.
fn match_response(text: &str, id: u64) -> Option<Reply> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let items = match v {
        Value::Array(a) => a,
        v => vec![v],
    };
    items
        .into_iter()
        .find(|m| m.get("id").and_then(|x| x.as_u64()) == Some(id) && m.get("method").is_none())
        .map(|m| rpc_result(&m))
}

// ---------------------------------------------------------------- connection

enum Conn {
    Stdio(StdioConn),
    Http(HttpConn),
}

impl Conn {
    async fn request(&self, id: u64, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        match self {
            Conn::Stdio(c) => c.request(id, method, params, timeout).await.map_err(|e| {
                let tail = c.stderr_tail();
                if tail.is_empty() || c.alive.load(Ordering::SeqCst) {
                    e
                } else {
                    anyhow!("{e}: {}", clip(&tail, 300))
                }
            }),
            Conn::Http(c) => c.request(id, method, params, timeout).await,
        }
    }
    async fn notify(&self, method: &str, params: Value) {
        match self {
            Conn::Stdio(c) => c.notify(method, params),
            Conn::Http(c) => c.notify(method, params).await,
        }
    }
    fn alive(&self) -> bool {
        match self {
            Conn::Stdio(c) => c.alive.load(Ordering::SeqCst),
            Conn::Http(_) => true,
        }
    }
    async fn close(&self) {
        match self {
            Conn::Stdio(c) => c.close().await,
            Conn::Http(c) => c.close().await,
        }
    }
}

#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub schema: Value,
    pub read_only: bool,
}

pub struct McpClient {
    pub server: McpServer,
    conn: tokio::sync::RwLock<Arc<Conn>>,
    next: AtomicU64,
    tools: Mutex<Vec<McpToolDef>>,
}

impl McpClient {
    pub async fn connect(server: &McpServer) -> Result<Arc<McpClient>> {
        let c = McpClient {
            server: server.clone(),
            conn: tokio::sync::RwLock::new(Arc::new(open(server).await?)),
            next: AtomicU64::new(1),
            tools: Mutex::new(vec![]),
        };
        let conn = c.conn.read().await.clone();
        if let Err(e) = c.handshake(&conn).await {
            conn.close().await;
            return Err(e);
        }
        Ok(Arc::new(c))
    }

    #[cfg(test)]
    async fn from_conn(server: McpServer, conn: Conn) -> Result<Arc<McpClient>> {
        let c = McpClient {
            server,
            conn: tokio::sync::RwLock::new(Arc::new(conn)),
            next: AtomicU64::new(1),
            tools: Mutex::new(vec![]),
        };
        let conn = c.conn.read().await.clone();
        c.handshake(&conn).await?;
        Ok(Arc::new(c))
    }

    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    async fn handshake(&self, conn: &Conn) -> Result<()> {
        let init = conn
            .request(
                self.id(),
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "xode", "version": env!("CARGO_PKG_VERSION")}
                }),
                INIT_TIMEOUT,
            )
            .await
            .context("initialize")?;
        if let Conn::Http(h) = conn {
            let v = init["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION).to_string();
            *h.proto.lock() = Some(v);
        }
        conn.notify("notifications/initialized", json!({})).await;
        let mut defs = vec![];
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = conn.request(self.id(), "tools/list", params, INIT_TIMEOUT).await.context("tools/list")?;
            if let Some(a) = r["tools"].as_array() {
                defs.extend(a.iter().filter_map(parse_tool));
            }
            cursor = r["nextCursor"].as_str().filter(|c| !c.is_empty()).map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        *self.tools.lock() = defs;
        Ok(())
    }

    pub fn tool_defs(&self) -> Vec<McpToolDef> {
        self.tools.lock().clone()
    }

    pub async fn alive(&self) -> bool {
        self.conn.read().await.alive()
    }

    async fn live_conn(&self) -> Result<Arc<Conn>> {
        let c = self.conn.read().await.clone();
        if c.alive() {
            return Ok(c);
        }
        let mut w = self.conn.write().await;
        if w.alive() {
            return Ok(w.clone());
        }
        tracing::info!("mcp[{}] restarting", self.server.name);
        let n = Arc::new(open(&self.server).await?);
        self.handshake(&n).await?;
        *w = n.clone();
        Ok(n)
    }

    pub async fn call_tool(&self, name: &str, args: Value) -> Result<(String, bool)> {
        self.call_tool_id(self.id(), name, args).await
    }

    async fn call_tool_id(&self, id: u64, name: &str, args: Value) -> Result<(String, bool)> {
        let conn = self.live_conn().await?;
        let args = if args.is_object() { args } else { json!({}) };
        let r = conn.request(id, "tools/call", json!({"name": name, "arguments": args}), CALL_TIMEOUT).await?;
        Ok(format_result(&r))
    }

    async fn cancel(&self, id_hint: u64) {
        let c = self.conn.read().await.clone();
        c.notify("notifications/cancelled", json!({"requestId": id_hint, "reason": "cancelled"})).await;
    }

    pub async fn shutdown(&self) {
        self.conn.read().await.close().await;
    }
}

async fn open(s: &McpServer) -> Result<Conn> {
    match s.transport.as_str() {
        "http" | "streamable-http" | "streamable_http" | "sse" => {
            if s.url.trim().is_empty() {
                bail!("url required");
            }
            Ok(Conn::Http(HttpConn::new(s)?))
        }
        _ => {
            if s.command.trim().is_empty() {
                bail!("command required");
            }
            Ok(Conn::Stdio(StdioConn::spawn(s).await?))
        }
    }
}

fn parse_tool(t: &Value) -> Option<McpToolDef> {
    let name = t["name"].as_str()?.to_string();
    let mut schema = t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"}));
    if let Some(o) = schema.as_object_mut() {
        o.remove("$schema");
        o.entry("type").or_insert(json!("object"));
        o.entry("properties").or_insert(json!({}));
    }
    trim_descriptions(&mut schema);
    let desc = t["description"].as_str().or(t["title"].as_str()).unwrap_or("");
    Some(McpToolDef {
        name,
        description: clip(desc, DESC_MAX),
        schema,
        read_only: t["annotations"]["readOnlyHint"].as_bool().unwrap_or(false),
    })
}

fn trim_descriptions(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if k == "description" {
                    if let Value::String(s) = x {
                        if s.chars().count() > DESC_MAX {
                            *s = clip(s, DESC_MAX);
                        }
                    }
                } else {
                    trim_descriptions(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(trim_descriptions),
        _ => {}
    }
}

/// Flatten a tools/call result to text. Returns (text, is_error).
pub fn format_result(r: &Value) -> (String, bool) {
    let mut parts = vec![];
    if let Some(a) = r["content"].as_array() {
        for c in a {
            match c["type"].as_str().unwrap_or("") {
                "text" => parts.push(c["text"].as_str().unwrap_or("").to_string()),
                "image" | "audio" => parts.push(format!(
                    "[{} {}, {} KB]",
                    c["type"].as_str().unwrap_or(""),
                    c["mimeType"].as_str().unwrap_or(""),
                    c["data"].as_str().map(|d| d.len() * 3 / 4 / 1024).unwrap_or(0)
                )),
                "resource" => {
                    let res = &c["resource"];
                    match res["text"].as_str() {
                        Some(t) => parts.push(t.to_string()),
                        None => parts.push(format!("[resource {}]", res["uri"].as_str().unwrap_or(""))),
                    }
                }
                "resource_link" => parts.push(format!(
                    "[link {} {}]",
                    c["name"].as_str().unwrap_or(""),
                    c["uri"].as_str().unwrap_or("")
                )),
                other => parts.push(format!("[{other}]")),
            }
        }
    }
    if parts.is_empty() {
        if let Some(sc) = r.get("structuredContent") {
            parts.push(sc.to_string());
        }
    }
    let text = parts.join("\n");
    (if text.is_empty() { "(no output)".into() } else { text }, r["isError"].as_bool().unwrap_or(false))
}

// ---------------------------------------------------------------- tool wrapper

pub fn tool_name(server: &str, tool: &str) -> String {
    let san = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect::<String>();
    let mut n = format!("mcp__{}__{}", san(server), san(tool));
    n.truncate(64);
    n
}

struct McpTool {
    full: String,
    def: McpToolDef,
    client: Arc<McpClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.full
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: self.full.clone(), description: self.def.description.clone(), parameters: self.def.schema.clone() }
    }
    fn read_only(&self, _args: &Value) -> bool {
        self.def.read_only
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let hint = self.client.id();
        let r = tokio::select! {
            r = self.client.call_tool_id(hint, &self.def.name, args) => r,
            _ = ctx.cancel.cancelled() => {
                self.client.cancel(hint).await;
                return ToolOutput::err("cancelled");
            }
        };
        let max = ctx.config.token_saving.max_tool_output_tokens.max(500);
        match r {
            Ok((text, is_err)) => {
                let t = cap_tokens(&text, max);
                if is_err {
                    ToolOutput::err(t)
                } else {
                    ToolOutput::ok(t)
                }
            }
            Err(e) => ToolOutput::err(format!("{e:#}")),
        }
    }
}

// ---------------------------------------------------------------- manager

#[derive(Debug, Clone, Serialize)]
pub struct McpStatus {
    pub name: String,
    pub connected: bool,
    pub tools: Vec<String>,
    pub error: Option<String>,
}

struct Entry {
    server: McpServer,
    client: Option<Arc<McpClient>>,
    error: Option<String>,
}

pub struct McpManager {
    entries: Vec<Entry>,
}

impl McpManager {
    /// Connect to all enabled servers concurrently; failures are recorded, not fatal.
    pub async fn start(servers: &[McpServer]) -> Arc<McpManager> {
        let futs = servers.iter().filter(|s| s.enabled && !s.name.trim().is_empty()).map(|s| async move {
            let r = match tokio::time::timeout(INIT_TIMEOUT + Duration::from_secs(10), McpClient::connect(s)).await {
                Ok(r) => r,
                Err(_) => Err(anyhow!("connect timeout")),
            };
            match r {
                Ok(c) => Entry { server: s.clone(), client: Some(c), error: None },
                Err(e) => {
                    tracing::warn!("mcp[{}] failed: {e:#}", s.name);
                    Entry { server: s.clone(), client: None, error: Some(format!("{e:#}")) }
                }
            }
        });
        Arc::new(McpManager { entries: futures::future::join_all(futs).await })
    }

    pub fn tools(&self) -> Vec<ToolRef> {
        let mut out: Vec<ToolRef> = vec![];
        for e in &self.entries {
            let Some(c) = &e.client else { continue };
            for d in c.tool_defs() {
                if e.server.disabled_tools.iter().any(|x| x == &d.name) {
                    continue;
                }
                out.push(Arc::new(McpTool { full: tool_name(&e.server.name, &d.name), def: d, client: c.clone() }));
            }
        }
        out
    }

    pub fn status(&self) -> Vec<McpStatus> {
        self.entries
            .iter()
            .map(|e| McpStatus {
                name: e.server.name.clone(),
                connected: e.client.as_ref().map(|c| c.conn.try_read().map(|x| x.alive()).unwrap_or(true)).unwrap_or(false),
                tools: e.client.as_ref().map(|c| c.tool_defs().into_iter().map(|d| d.name).collect()).unwrap_or_default(),
                error: e.error.clone(),
            })
            .collect()
    }

    pub async fn shutdown(&self) {
        futures::future::join_all(self.entries.iter().filter_map(|e| e.client.as_ref()).map(|c| c.shutdown())).await;
    }
}

/// Connect to one server and return its tool names.
pub async fn test_server(s: &McpServer) -> Result<Vec<String>> {
    let c = McpClient::connect(s).await?;
    let names = c.tool_defs().into_iter().map(|d| d.name).collect();
    c.shutdown().await;
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake MCP server speaking newline-delimited JSON-RPC over a duplex pipe.
    async fn fake_server(io: tokio::io::DuplexStream) {
        let (r, mut w) = tokio::io::split(io);
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            let m: Value = serde_json::from_str(&l).unwrap();
            let Some(id) = m.get("id").cloned() else { continue };
            let Some(method) = m["method"].as_str() else { continue };
            let res = match method {
                "initialize" => json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {"tools": {}}, "serverInfo": {"name": "fake"}}),
                "tools/list" if m["params"].get("cursor").is_none() => json!({
                    "tools": [{"name": "echo", "description": "x".repeat(500), "inputSchema": {"$schema": "http://json-schema.org/draft-07/schema#", "type": "object", "properties": {"s": {"type": "string"}}}, "annotations": {"readOnlyHint": true}}],
                    "nextCursor": "p2"
                }),
                "tools/list" => json!({"tools": [{"name": "fail", "inputSchema": {"type": "object"}}]}),
                "tools/call" => {
                    // server->client ping before answering
                    w.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"srv1\",\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n").await.unwrap();
                    if m["params"]["name"] == "echo" {
                        json!({"content": [{"type": "text", "text": format!("echo {}", m["params"]["arguments"]["s"].as_str().unwrap_or(""))}, {"type": "image", "mimeType": "image/png", "data": "AAAA"}]})
                    } else {
                        let e = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "nope"}});
                        w.write_all(format!("{e}\n").as_bytes()).await.unwrap();
                        continue;
                    }
                }
                _ => continue,
            };
            let reply = json!({"jsonrpc": "2.0", "id": id, "result": res});
            w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        }
    }

    #[tokio::test]
    async fn stdio_roundtrip() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        tokio::spawn(fake_server(server_io));
        let (r, w) = tokio::io::split(client_io);
        let conn = Conn::Stdio(StdioConn::from_io(r, w));
        let server = McpServer { name: "my srv".into(), ..Default::default() };
        let c = McpClient::from_conn(server.clone(), conn).await.unwrap();
        let defs = c.tool_defs();
        assert_eq!(defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["echo", "fail"]);
        assert!(defs[0].read_only);
        assert!(defs[0].description.chars().count() <= DESC_MAX);
        assert!(defs[0].schema.get("$schema").is_none());
        assert!(defs[1].schema.get("properties").is_some());
        let (t, err) = c.call_tool("echo", json!({"s": "hi"})).await.unwrap();
        assert!(!err);
        assert_eq!(t, "echo hi\n[image image/png, 0 KB]");
        let e = c.call_tool("fail", json!({})).await.unwrap_err();
        assert!(e.to_string().contains("nope"));

        let m = McpManager { entries: vec![Entry { server: McpServer { disabled_tools: vec!["fail".into()], ..server }, client: Some(c), error: None }] };
        let tools = m.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "mcp__my_srv__echo");
        let st = m.status();
        assert!(st[0].connected);
        assert_eq!(st[0].tools, vec!["echo", "fail"]);
    }

    #[test]
    fn sse_parse() {
        let mut b = String::from("event: message\r\ndata: {\"a\":1}\r\n\r\n: comment\n\ndata: part1\ndata: part2\n\ndata: incompl");
        let d = sse_take(&mut b);
        assert_eq!(d, vec!["{\"a\":1}".to_string(), "part1\npart2".to_string()]);
        assert_eq!(b, "data: incompl");
    }

    #[test]
    fn response_match() {
        let t = r#"[{"jsonrpc":"2.0","method":"x"},{"jsonrpc":"2.0","id":7,"result":{"ok":true}}]"#;
        assert_eq!(match_response(t, 7).unwrap().unwrap()["ok"], true);
        assert!(match_response(t, 8).is_none());
        let e = r#"{"jsonrpc":"2.0","id":3,"error":{"code":-1,"message":"bad"}}"#;
        assert!(match_response(e, 3).unwrap().unwrap_err().contains("bad"));
    }

    #[test]
    fn names() {
        assert_eq!(tool_name("git hub", "list.repos"), "mcp__git_hub__list_repos");
        assert!(tool_name(&"a".repeat(80), "b").len() <= 64);
    }

    /// Live: runs the reference "everything" server via npx if available.
    #[tokio::test]
    #[ignore]
    async fn live_npx() {
        let s = McpServer {
            name: "everything".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-everything".into()],
            ..Default::default()
        };
        println!("{:?}", test_server(&s).await);
    }

    /// Live: python reference server via uvx; on Windows also through a .cmd shim.
    #[tokio::test]
    #[ignore]
    async fn live_uvx() {
        let mut s = McpServer {
            name: "time".into(),
            command: "uvx".into(),
            args: vec!["mcp-server-time".into()],
            ..Default::default()
        };
        if cfg!(windows) {
            let shim = std::env::temp_dir().join("xode-mcp-shim.cmd");
            std::fs::write(&shim, "@uvx %*\r\n").unwrap();
            s.command = shim.to_string_lossy().into();
        }
        let c = McpClient::connect(&s).await.unwrap();
        println!("tools: {:?}", c.tool_defs().iter().map(|d| &d.name).collect::<Vec<_>>());
        println!("{:?}", c.call_tool("get_current_time", json!({"timezone": "UTC"})).await);
        c.shutdown().await;
    }
}

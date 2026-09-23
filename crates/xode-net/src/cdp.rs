//! Minimal Chrome DevTools Protocol client over WebSocket.
//!
//! One browser-level connection; page sessions use flattened `sessionId`s.
//! Responses are routed to callers by `id`; console/network events are
//! buffered per session for the `browser` tool.

use crate::util::clip;
use anyhow::{anyhow, bail, Result};
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

const BUF_CAP: usize = 300;

type Reply = std::result::Result<Value, String>;

#[derive(Debug, Clone, Default)]
pub struct NetEntry {
    pub id: String,
    pub method: String,
    pub url: String,
    pub kind: String,
    pub status: Option<i64>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Buffers {
    pub console: HashMap<String, VecDeque<String>>,
    pub network: HashMap<String, VecDeque<NetEntry>>,
}

struct Shared {
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    alive: AtomicBool,
    bufs: Mutex<Buffers>,
}

pub struct Cdp {
    tx: mpsc::UnboundedSender<String>,
    next: AtomicU64,
    shared: Arc<Shared>,
    pub timeout: Duration,
}

impl Cdp {
    pub async fn connect(ws_url: &str) -> Result<Arc<Cdp>> {
        let cfg = WebSocketConfig { max_message_size: None, max_frame_size: None, ..Default::default() };
        let (ws, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async_with_config(ws_url, Some(cfg), false),
        )
        .await
        .map_err(|_| anyhow!("websocket connect timeout"))??;
        Ok(Self::from_stream(ws))
    }

    pub fn from_stream<S>(ws: WebSocketStream<S>) -> Arc<Cdp>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
            bufs: Mutex::new(Buffers::default()),
        });
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if sink.send(Message::Text(m)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let sh = shared.clone();
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Binary(b)) => String::from_utf8_lossy(&b).into_owned(),
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                handle_message(&sh, v);
            }
            sh.alive.store(false, Ordering::SeqCst);
            for (_, s) in sh.pending.lock().drain() {
                let _ = s.send(Err("browser connection closed".into()));
            }
        });
        Arc::new(Cdp { tx, next: AtomicU64::new(1), shared, timeout: Duration::from_secs(30) })
    }

    pub fn alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst) && !self.tx.is_closed()
    }

    /// Send a command and wait for its result.
    pub async fn call(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        if !self.alive() {
            bail!("browser connection closed");
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().insert(id, tx);
        if self.tx.send(msg.to_string()).is_err() {
            self.shared.pending.lock().remove(&id);
            bail!("browser connection closed");
        }
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!("{method}: {e}")),
            Ok(Err(_)) => bail!("browser connection closed"),
            Err(_) => {
                self.shared.pending.lock().remove(&id);
                bail!("{method}: timeout")
            }
        }
    }

    pub fn take_console(&self, session: &str) -> Vec<String> {
        self.shared.bufs.lock().console.get_mut(session).map(|q| q.drain(..).collect()).unwrap_or_default()
    }

    pub fn take_network(&self, session: &str) -> Vec<NetEntry> {
        self.shared.bufs.lock().network.get_mut(session).map(|q| q.drain(..).collect()).unwrap_or_default()
    }

    pub fn forget(&self, session: &str) {
        let mut b = self.shared.bufs.lock();
        b.console.remove(session);
        b.network.remove(session);
    }
}

impl Drop for Cdp {
    fn drop(&mut self) {
        self.shared.alive.store(false, Ordering::SeqCst);
    }
}

fn handle_message(sh: &Shared, v: Value) {
    if let Some(id) = v.get("id").and_then(|x| x.as_u64()) {
        if let Some(tx) = sh.pending.lock().remove(&id) {
            let r = match v.get("error") {
                Some(e) => {
                    let m = e.get("message").and_then(|m| m.as_str()).unwrap_or("error");
                    let d = e.get("data").and_then(|m| m.as_str()).map(|d| format!(" ({d})")).unwrap_or_default();
                    Err(format!("{m}{d}"))
                }
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(r);
        }
        return;
    }
    let Some(method) = v.get("method").and_then(|m| m.as_str()) else { return };
    let session = v.get("sessionId").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let p = &v["params"];
    let mut b = sh.bufs.lock();
    match method {
        "Runtime.consoleAPICalled" => {
            let kind = p["type"].as_str().unwrap_or("log");
            let args: Vec<String> = p["args"]
                .as_array()
                .map(|a| a.iter().map(remote_obj).collect())
                .unwrap_or_default();
            push_console(&mut b, &session, format!("[{kind}] {}", clip(&args.join(" "), 500)));
        }
        "Runtime.exceptionThrown" => {
            let d = &p["exceptionDetails"];
            let text = d["exception"]["description"].as_str().or(d["text"].as_str()).unwrap_or("exception");
            let at = d["url"].as_str().map(|u| format!(" @{}:{}", clip(u, 80), d["lineNumber"])).unwrap_or_default();
            push_console(&mut b, &session, format!("[exception] {}{at}", clip(text, 500)));
        }
        "Log.entryAdded" => {
            let e = &p["entry"];
            let url = e["url"].as_str().map(|u| format!(" ({})", clip(u, 100))).unwrap_or_default();
            push_console(
                &mut b,
                &session,
                format!("[{}] {}{url}", e["level"].as_str().unwrap_or("log"), clip(e["text"].as_str().unwrap_or(""), 500)),
            );
        }
        "Network.requestWillBeSent" => {
            let url = p["request"]["url"].as_str().unwrap_or("");
            if url.starts_with("data:") || url.starts_with("blob:") {
                return;
            }
            let q = b.network.entry(session).or_default();
            if q.len() >= BUF_CAP {
                q.pop_front();
            }
            q.push_back(NetEntry {
                id: p["requestId"].as_str().unwrap_or("").into(),
                method: p["request"]["method"].as_str().unwrap_or("GET").into(),
                url: url.into(),
                kind: p["type"].as_str().unwrap_or("").to_lowercase(),
                status: None,
                error: None,
            });
        }
        "Network.responseReceived" | "Network.loadingFailed" => {
            let id = p["requestId"].as_str().unwrap_or("");
            if let Some(q) = b.network.get_mut(&session) {
                if let Some(e) = q.iter_mut().rev().find(|e| e.id == id) {
                    if method == "Network.responseReceived" {
                        e.status = p["response"]["status"].as_i64();
                    } else {
                        e.error = Some(p["errorText"].as_str().unwrap_or("failed").into());
                    }
                }
            }
        }
        "Target.detachedFromTarget" => {
            if let Some(s) = p["sessionId"].as_str() {
                b.console.remove(s);
                b.network.remove(s);
            }
        }
        _ => {}
    }
}

fn push_console(b: &mut Buffers, session: &str, line: String) {
    let q = b.console.entry(session.to_string()).or_default();
    if q.len() >= BUF_CAP {
        q.pop_front();
    }
    q.push_back(line);
}

/// Render a CDP RemoteObject compactly.
pub fn remote_obj(o: &Value) -> String {
    if let Some(v) = o.get("value") {
        return match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
    }
    if let Some(u) = o.get("unserializableValue").and_then(|x| x.as_str()) {
        return u.to_string();
    }
    if let Some(d) = o.get("description").and_then(|x| x.as_str()) {
        return d.to_string();
    }
    o.get("type").and_then(|x| x.as_str()).unwrap_or("?").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Fake browser: answers requests out of order and emits events.
    #[tokio::test]
    async fn routes_ids_and_buffers_events() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
            let mut got = vec![];
            while got.len() < 2 {
                if let Some(Ok(Message::Text(t))) = ws.next().await {
                    got.push(serde_json::from_str::<Value>(&t).unwrap());
                }
            }
            // events first
            let ev = [
                json!({"method":"Runtime.consoleAPICalled","sessionId":"S1","params":{"type":"error","args":[{"type":"string","value":"boom"},{"type":"number","value":42}]}}),
                json!({"method":"Network.requestWillBeSent","sessionId":"S1","params":{"requestId":"r1","type":"XHR","request":{"method":"POST","url":"https://a.b/api"}}}),
                json!({"method":"Network.responseReceived","sessionId":"S1","params":{"requestId":"r1","response":{"status":201}}}),
            ];
            for e in ev {
                ws.send(Message::Text(e.to_string())).await.unwrap();
            }
            // reply in reverse order
            for m in got.iter().rev() {
                let id = m["id"].as_u64().unwrap();
                let reply = if m["method"] == "Bad.method" {
                    json!({"id": id, "error": {"code": -32601, "message": "not found"}})
                } else {
                    json!({"id": id, "result": {"echo": m["method"], "session": m["sessionId"]}})
                };
                ws.send(Message::Text(reply.to_string())).await.unwrap();
            }
            while ws.next().await.is_some() {}
        });
        let cdp = Cdp::connect(&format!("ws://{addr}")).await.unwrap();
        let (a, b) = tokio::join!(
            cdp.call("Page.navigate", json!({"url": "x"}), Some("S1")),
            cdp.call("Bad.method", json!({}), None)
        );
        let a = a.unwrap();
        assert_eq!(a["echo"], "Page.navigate");
        assert_eq!(a["session"], "S1");
        assert!(b.unwrap_err().to_string().contains("not found"));
        assert_eq!(cdp.take_console("S1"), vec!["[error] boom 42".to_string()]);
        assert!(cdp.take_console("S1").is_empty());
        let n = cdp.take_network("S1");
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].status, Some(201));
        assert_eq!(n[0].kind, "xhr");
    }
}

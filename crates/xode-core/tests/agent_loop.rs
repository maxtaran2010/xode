//! End-to-end agent loop against a mock OpenAI-compatible SSE server.
//! Verifies: tool loop, silent compaction with a tiny window, PATH.md stays short, loop continues to completion.
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use xode_core::agent::{CtxCache, PermissionBroker, Queued, Runtime};
use xode_core::config::{ApiKind, Config, Gateway, ModelInfo};
use xode_core::store::Store;
use xode_core::tool::{SessionState, Tool, ToolCtx, ToolOutput, ToolRegistry};
use xode_core::types::{Message, MsgKind, ToolSpec};
use xode_core::{AgentEvent, LiveStats};

struct Echo;
#[async_trait]
impl Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "echo".into(), description: "echo".into(), parameters: json!({"type":"object","properties":{"n":{"type":"integer"}}}) }
    }
    fn read_only(&self, _: &Value) -> bool {
        true
    }
    async fn run(&self, args: Value, _ctx: &ToolCtx) -> ToolOutput {
        let n = args["n"].as_u64().unwrap_or(0);
        // ~600 tokens of output per call.
        ToolOutput::ok(format!("result {n}: {}", "lorem ipsum dolor sit amet ".repeat(120)))
    }
}

fn sse(chunks: Vec<Value>) -> String {
    let mut body = String::new();
    for c in chunks {
        body.push_str(&format!("data: {}\n\n", c));
    }
    body.push_str("data: [DONE]\n\n");
    format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body)
}

async fn mock_server(calls: Arc<AtomicUsize>, compactions: Arc<AtomicUsize>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            let calls = calls.clone();
            let compactions = compactions.clone();
            tokio::spawn(async move {
                let mut buf = vec![];
                let mut tmp = [0u8; 65536];
                let body: Value;
                loop {
                    let n = s.read(&mut tmp).await.unwrap();
                    buf.extend_from_slice(&tmp[..n]);
                    let txt = String::from_utf8_lossy(&buf).to_string();
                    if let Some(i) = txt.find("\r\n\r\n") {
                        let cl: usize = txt[..i]
                            .lines()
                            .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                            .unwrap_or(0);
                        if buf.len() >= i + 4 + cl {
                            body = serde_json::from_slice(&buf[i + 4..i + 4 + cl]).unwrap();
                            break;
                        }
                    }
                }
                let msgs = body["messages"].as_array().unwrap();
                let last = msgs.last().unwrap();
                let last_text = last["content"].as_str().unwrap_or("");
                let resp = if last_text.contains("CONTEXT LIMIT REACHED") {
                    compactions.fetch_add(1, Ordering::SeqCst);
                    sse(vec![json!({"choices":[{"index":0,"delta":{"content":"<path>\n- called echo several times\n- nothing else\n</path>\n<state>\ngoal: call echo 12 times\ndone: some\nnow: echoing\nnext: continue\n</state>"}}]})])
                } else {
                    let seeded = msgs.iter().any(|m| m["content"].as_str().map(|c| c.contains("Context was compacted")).unwrap_or(false));
                    let tool_msgs = msgs.iter().filter(|m| m["role"] == "tool").count();
                    let k = calls.fetch_add(1, Ordering::SeqCst);
                    if k >= 12 {
                        sse(vec![
                            json!({"choices":[{"index":0,"delta":{"reasoning_content":"all done"}}]}),
                            json!({"choices":[{"index":0,"delta":{"content":"Finished."},"finish_reason":"stop"}]}),
                            json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5}}),
                        ])
                    } else {
                        let _ = (seeded, tool_msgs);
                        sse(vec![
                            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("c{k}"),"function":{"name":"echo","arguments":""}}]}}]}),
                            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":format!("{{\"n\":{k}}}")}}]}}]}),
                            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
                        ])
                    }
                };
                s.write_all(resp.as_bytes()).await.unwrap();
                let _ = s.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test(flavor = "multi_thread")]
async fn loop_compacts_and_finishes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let compactions = Arc::new(AtomicUsize::new(0));
    let url = mock_server(calls.clone(), compactions.clone()).await;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = Arc::new(Store::open(&root.join("db.sqlite")).unwrap());
    let project = store.add_project(&root.to_string_lossy(), None).unwrap();
    let sess = store.create_session(&project.id).unwrap();

    let mut cfg = Config::default();
    cfg.compaction.context_limit = 4000;
    cfg.compaction.threshold_tokens = 2600;
    cfg.compaction.reserve_tokens = 300;
    cfg.compaction.path_max_lines = 12;
    cfg.compaction.include_repo_map = false;
    let gw = Gateway {
        url: url.clone(),
        kind: ApiKind::Openai,
        flavor: "llamacpp".into(),
        models: vec![ModelInfo { id: "m".into(), context: 4000, vision: false }],
        ..Default::default()
    };
    let mut tools = ToolRegistry::default();
    tools.add(Arc::new(Echo));
    let (tx, mut rx) = tokio::sync::broadcast::channel(4096);
    let rt = Runtime {
        session_id: sess.id.clone(),
        project: project.clone(),
        store: store.clone(),
        config: Arc::new(cfg.clone()),
        gateway: gw,
        model: "m".into(),
        tools,
        state: Arc::new(Mutex::new(SessionState::default())),
        outliner: None,
        repo_map: None,
        events: tx,
        cancel: CancellationToken::new(),
        broker: Arc::new(PermissionBroker::default()),
        always: Arc::new(Mutex::new(HashSet::new())),
        stats: Arc::new(Mutex::new(LiveStats::default())),
        cache: Arc::new(Mutex::new(CtxCache::default())),
        user_stopped: Arc::new(AtomicBool::new(false)),
        queue: Default::default(),
    };
    let collector = tokio::spawn(async move {
        let (mut n, mut progress) = (0, 0);
        while let Ok(e) = rx.recv().await {
            match e {
                AgentEvent::CompactionDone { .. } => n += 1,
                AgentEvent::CompactionProgress { .. } => progress += 1,
                _ => {}
            }
        }
        (n, progress)
    });
    // Steer: a message queued while running is delivered right after the first tool result.
    rt.queue.push(Message::user("steer: also be brief"));
    // A queued /compact runs inline right after it; a queued command stays for the engine.
    rt.queue.push_item(Queued::Compact { id: "q1".into() });
    rt.queue.push_item(Queued::Cmd { id: "q2".into(), line: "/undo".into() });
    let out = tokio::time::timeout(std::time::Duration::from_secs(60), rt.run(Some(Message::user("call echo 12 times"))))
        .await
        .expect("timeout")
        .unwrap();
    assert!(!out.stopped);
    let rt_queue_left = rt.queue.clone();
    drop(rt);
    let (done_events, progress) = collector.await.unwrap();
    assert!(progress >= done_events * 3, "progress events: {progress}");
    assert_eq!(rt_queue_left.snapshot().len(), 1);

    let c = compactions.load(Ordering::SeqCst);
    assert!(c >= 2, "expected several compactions, got {c}");
    assert_eq!(done_events, c);
    let s = store.session(&sess.id).unwrap().unwrap();
    assert_eq!(s.compactions as usize, c);
    assert_eq!(s.segment as usize, c);
    let msgs = store.messages(&sess.id).unwrap();
    assert!(msgs.iter().any(|m| m.kind == MsgKind::Seed));
    assert_eq!(msgs.last().unwrap().text(), "Finished.");
    let first_tool = msgs.iter().position(|m| m.role == xode_core::Role::Tool).unwrap();
    assert_eq!(msgs[first_tool + 1].text(), "steer: also be brief");
    assert_eq!(msgs[first_tool + 2].kind, MsgKind::Compaction);
    let path_md = std::fs::read_to_string(root.join(".xode/PATH.md")).unwrap();
    assert!(path_md.lines().count() <= 14, "PATH.md too long:\n{path_md}");
    assert!(path_md.contains("called echo"));
    // Seed must carry the original request and the state.
    let seed = msgs.iter().find(|m| m.kind == MsgKind::Seed).unwrap().text();
    assert!(seed.contains("call echo 12 times"));
    assert!(seed.contains("goal: call echo 12 times"));
}

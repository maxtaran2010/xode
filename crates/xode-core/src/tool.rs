use crate::config::Config;
use crate::types::ToolSpec;
use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// What a tool did to a file; drives the working set that survives compaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileTouch {
    pub path: String,
    /// read | edit | write
    pub how: String,
    pub hash: String,
    pub step: u32,
    pub segment: u32,
    pub lines: usize,
    /// Ranges read, e.g. "1-200".
    pub ranges: Vec<String>,
}

/// Per-session mutable state shared by all tools.
#[derive(Debug, Default)]
pub struct SessionState {
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub touched: BTreeMap<String, FileTouch>,
    pub step: u32,
    pub segment: u32,
    pub rtk_saved_tokens: u64,
    /// Snapshots of files before the agent changed them this turn (for /undo).
    pub undo: Vec<(String, Option<Vec<u8>>)>,
    pub turn: u32,
}

pub type SharedState = Arc<Mutex<SessionState>>;

/// Structural summary provider (implemented by the code index).
pub trait Outliner: Send + Sync {
    /// Compact symbol outline of a file (one symbol per line with line numbers), if indexable.
    fn outline(&self, path: &Path) -> Option<String>;
    /// Notify that a file changed on disk (after edit/write) so the index refreshes it now.
    fn file_changed(&self, _path: &Path) {}
}

#[derive(Clone)]
pub struct ToolCtx {
    pub session_id: String,
    pub project_root: PathBuf,
    pub extra_roots: Vec<PathBuf>,
    pub config: Arc<Config>,
    pub state: SharedState,
    pub cancel: CancellationToken,
    pub outliner: Option<Arc<dyn Outliner>>,
}

impl ToolCtx {
    pub fn cwd(&self) -> PathBuf {
        self.state.lock().cwd.clone().unwrap_or_else(|| self.project_root.clone())
    }

    /// Resolve a user/model supplied path relative to cwd. Accepts `/` or `\`.
    pub fn resolve(&self, p: &str) -> PathBuf {
        let p = p.trim().trim_matches('"');
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            return pb;
        }
        let from_cwd = self.cwd().join(&pb);
        if from_cwd.exists() {
            return from_cwd;
        }
        // Paths shown to the model are project-relative; fall back to the root.
        let from_root = self.project_root.join(&pb);
        if from_root.exists() || from_root.parent().map(|p| p.exists()).unwrap_or(false) {
            from_root
        } else {
            from_cwd
        }
    }

    /// Path shown to the model: relative to project if inside it, `/` separated.
    pub fn display(&self, p: &Path) -> String {
        let rel = p.strip_prefix(&self.project_root).unwrap_or(p);
        rel.to_string_lossy().replace('\\', "/")
    }

    pub fn touch(&self, path: &Path, how: &str, content: &[u8], range: Option<String>) {
        let key = self.display(path);
        let hash = hash_bytes(content);
        let lines = content.iter().filter(|b| **b == b'\n').count() + 1;
        let mut st = self.state.lock();
        let step = st.step;
        let segment = st.segment;
        let e = st.touched.entry(key.clone()).or_insert(FileTouch {
            path: key,
            how: how.into(),
            hash: hash.clone(),
            step,
            segment,
            lines,
            ranges: vec![],
        });
        if e.hash != hash || e.segment != segment {
            e.ranges.clear();
        }
        e.hash = hash;
        e.step = step;
        e.segment = segment;
        e.lines = lines;
        if how != "read" || e.how == "read" {
            e.how = how.into();
        }
        if let Some(r) = range {
            if !e.ranges.contains(&r) {
                e.ranges.push(r);
            }
        }
    }

    pub fn snapshot_for_undo(&self, path: &Path) {
        let key = path.to_string_lossy().to_string();
        let mut st = self.state.lock();
        if st.undo.iter().any(|(p, _)| *p == key) {
            return;
        }
        let prev = std::fs::read(path).ok();
        st.undo.push((key, prev));
    }

    /// Directory for full tool outputs (`<project>/.xode/out`).
    pub fn out_dir(&self) -> PathBuf {
        let d = self.project_root.join(".xode").join("out");
        let _ = std::fs::create_dir_all(&d);
        d
    }

    pub fn add_rtk_saved(&self, n: u64) {
        self.state.lock().rtk_saved_tokens += n;
    }
}

pub fn hash_bytes(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b);
    hex::encode(&h.finalize()[..8])
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(s: impl Into<String>) -> Self {
        Self { content: s.into(), is_error: false }
    }
    pub fn err(s: impl Into<String>) -> Self {
        Self { content: s.into(), is_error: true }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    /// True if the call cannot modify anything (allowed in plan mode).
    fn read_only(&self, _args: &Value) -> bool {
        false
    }
    /// One-line human summary of the call for UI / permission prompts.
    fn summary(&self, args: &Value) -> String {
        let s = args.to_string();
        s.chars().take(160).collect()
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput;
}

pub type ToolRef = Arc<dyn Tool>;

#[derive(Clone, Default)]
pub struct ToolRegistry {
    pub tools: Vec<ToolRef>,
}

impl ToolRegistry {
    pub fn add(&mut self, t: ToolRef) {
        self.tools.retain(|x| x.name() != t.name());
        self.tools.push(t);
    }
    pub fn get(&self, name: &str) -> Option<ToolRef> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|t| t.spec()).collect()
    }
}

/// Helpers for reading typed args.
pub fn arg_str<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}
pub fn arg_u64(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())))
}
pub fn arg_bool(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(|x| x.as_bool().or_else(|| x.as_str().map(|s| s == "true")))
}

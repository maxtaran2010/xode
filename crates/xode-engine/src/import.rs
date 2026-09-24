//! First-run import of projects and chats from other coding agents:
//! Claude Code (`~/.claude/projects/*/*.jsonl`, full chats), and Codex / OpenCode
//! (project folders discovered from their session data).

use crate::engine::Engine;
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use xode_core::store::Store;
use xode_core::types::{Message, MsgKind, Part, Role};

#[derive(Debug, Clone, Serialize)]
pub struct ImportSource {
    /// Stable key: claude_code | codex | opencode
    pub tool: String,
    pub label: String,
    pub available: bool,
    pub projects: usize,
    pub chats: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub projects: usize,
    pub chats: usize,
    pub messages: usize,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn claude_dir() -> PathBuf {
    home().join(".claude").join("projects")
}

/// Every `cwd` (Claude) or existing directory path found in a tool's session data.
fn scan_roots(tool: &str) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    match tool {
        "claude_code" => {
            let Ok(rd) = std::fs::read_dir(claude_dir()) else { return roots };
            for e in rd.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                if let Some(cwd) = project_cwd(&e.path()) {
                    roots.insert(cwd);
                }
            }
        }
        "codex" => {
            for p in [home().join(".codex"), home().join(".codex").join("sessions"), home().join(".codex").join("archived_sessions")] {
                scan_json_dirs(&p, &mut roots);
            }
        }
        "opencode" => {
            scan_json_dirs(&home().join(".local").join("share").join("opencode").join("storage"), &mut roots);
            scan_json_dirs(&home().join(".config").join("opencode"), &mut roots);
        }
        _ => {}
    }
    roots
}

/// Read a Claude project dir's newest jsonl and return the real cwd it recorded.
fn project_cwd(dir: &Path) -> Option<String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false)).collect();
    files.sort();
    for f in files {
        if let Ok(txt) = std::fs::read_to_string(&f) {
            for line in txt.lines().take(40) {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                        if Path::new(cwd).is_dir() {
                            return Some(cwd.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Heuristic: pull existing directory paths out of any `cwd`/`directory`/`path`/`worktree`/`root`
/// field across JSON/JSONL files under `dir` (used for Codex/OpenCode project discovery).
fn scan_json_dirs(dir: &Path, roots: &mut BTreeSet<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten().take(4000) {
        let p = e.path();
        if p.is_dir() {
            if roots.len() < 500 {
                scan_json_dirs(&p, roots);
            }
            continue;
        }
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
        if !matches!(ext, "json" | "jsonl") {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&p) else { continue };
        for line in txt.lines().take(200) {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                collect_dirs(&v, roots, 0);
            }
        }
    }
}

fn collect_dirs(v: &Value, roots: &mut BTreeSet<String>, depth: usize) {
    if depth > 6 || roots.len() > 800 {
        return;
    }
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                if matches!(k.as_str(), "cwd" | "directory" | "path" | "worktree" | "root" | "projectRoot") {
                    if let Some(s) = val.as_str() {
                        if s.starts_with('/') && Path::new(s).is_dir() {
                            roots.insert(s.to_string());
                        }
                    }
                }
                collect_dirs(val, roots, depth + 1);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_dirs(x, roots, depth + 1)),
        _ => {}
    }
}

fn ts_to_millis(v: &Value) -> i64 {
    v.get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis())
}

/// Convert Claude Code message content (array of typed blocks, or a bare string) into Xode
/// messages. Tool results (which Claude nests inside user turns) become a separate Tool message.
fn claude_line_to_messages(v: &Value, at: i64) -> Vec<Message> {
    let msg = v.get("message").cloned().unwrap_or(Value::Null);
    let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
    let content = msg.get("content");
    let mut text_parts: Vec<Part> = vec![];
    let mut tool_results: Vec<Part> = vec![];

    let push_block = |b: &Value, text_parts: &mut Vec<Part>, tool_results: &mut Vec<Part>| {
        match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "text" => {
                if let Some(t) = b.get("text").and_then(|t| t.as_str()).filter(|t| !t.trim().is_empty()) {
                    text_parts.push(Part::text(t));
                }
            }
            "thinking" => {
                if let Some(t) = b.get("thinking").and_then(|t| t.as_str()).or_else(|| b.get("text").and_then(|t| t.as_str())) {
                    text_parts.push(Part::Thinking { text: t.to_string() });
                }
            }
            "tool_use" => {
                text_parts.push(Part::ToolCall {
                    id: b.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    name: b.get("name").and_then(|x| x.as_str()).unwrap_or("tool").to_string(),
                    args: b.get("input").cloned().unwrap_or(Value::Null),
                });
            }
            "tool_result" => {
                let content = match b.get("content") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(a)) => a.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
                    other => other.map(|o| o.to_string()).unwrap_or_default(),
                };
                tool_results.push(Part::ToolResult {
                    id: b.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    name: String::new(),
                    content,
                    is_error: b.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
                });
            }
            "image" => {
                if let Some(src) = b.get("source") {
                    if let (Some(mime), Some(data)) = (src.get("media_type").and_then(|m| m.as_str()), src.get("data").and_then(|d| d.as_str())) {
                        text_parts.push(Part::Image { mime: mime.to_string(), data: data.to_string() });
                    }
                }
            }
            _ => {}
        }
    };

    match content {
        Some(Value::String(s)) if !s.trim().is_empty() => text_parts.push(Part::text(s)),
        Some(Value::Array(a)) => a.iter().for_each(|b| push_block(b, &mut text_parts, &mut tool_results)),
        _ => {}
    }

    let mut out = vec![];
    if !tool_results.is_empty() {
        out.push(Message { id: new_id(), role: Role::Tool, parts: tool_results, kind: MsgKind::Normal, segment: 0, meta: None, created_at: at });
    }
    if !text_parts.is_empty() {
        let r = if role == "assistant" { Role::Assistant } else { Role::User };
        out.push(Message { id: new_id(), role: r, parts: text_parts, kind: MsgKind::Normal, segment: 0, meta: None, created_at: at });
    }
    out
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Engine {
    pub fn import_detect(&self) -> Vec<ImportSource> {
        let mut out = vec![];
        // Claude Code
        {
            let dir = claude_dir();
            let roots = scan_roots("claude_code");
            let chats = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().is_dir())
                        .map(|e| std::fs::read_dir(e.path()).map(|r| r.flatten().filter(|f| f.path().extension().map(|x| x == "jsonl").unwrap_or(false)).count()).unwrap_or(0))
                        .sum()
                })
                .unwrap_or(0);
            out.push(ImportSource { tool: "claude_code".into(), label: "Claude Code".into(), available: dir.is_dir(), projects: roots.len(), chats, path: dir.to_string_lossy().into() });
        }
        for (tool, label, dir) in [
            ("codex", "Codex", home().join(".codex")),
            ("opencode", "OpenCode", home().join(".local").join("share").join("opencode")),
        ] {
            let roots = if dir.is_dir() { scan_roots(tool) } else { BTreeSet::new() };
            out.push(ImportSource { tool: tool.into(), label: label.into(), available: dir.is_dir(), projects: roots.len(), chats: 0, path: dir.to_string_lossy().into() });
        }
        out
    }

    /// Import projects (all tools) and, for Claude Code, chats too.
    pub fn import_run(&self, tool: &str, projects: bool, chats: bool) -> Result<ImportResult> {
        let mut res = ImportResult { projects: 0, chats: 0, messages: 0 };
        if projects || (chats && tool == "claude_code") {
            for root in scan_roots(tool) {
                if self.store.add_project(&root, None).is_ok() {
                    res.projects += 1;
                }
            }
        }
        if chats && tool == "claude_code" {
            res.messages += self.import_claude_chats(&mut res)?;
        }
        Ok(res)
    }

    fn import_claude_chats(&self, res: &mut ImportResult) -> Result<usize> {
        let mut messages = 0;
        let store: &Store = &self.store;
        // Map cwd -> project id for the projects we can see.
        let projects = store.projects()?;
        let dir = claude_dir();
        let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(0) };
        let existing: BTreeSet<String> = store.sessions(None)?.into_iter().map(|s| s.title).collect();
        for pdir in rd.flatten().filter(|e| e.path().is_dir()) {
            let Some(cwd) = project_cwd(&pdir.path()) else { continue };
            let Some(project) = projects.iter().find(|p| p.root == cwd) else { continue };
            let Ok(files) = std::fs::read_dir(pdir.path()) else { continue };
            for f in files.flatten() {
                let fp = f.path();
                if fp.extension().map(|x| x != "jsonl").unwrap_or(true) {
                    continue;
                }
                let Ok(txt) = std::fs::read_to_string(&fp) else { continue };
                let mut msgs: Vec<Message> = vec![];
                let mut title = String::new();
                let mut created = chrono::Utc::now().timestamp_millis();
                for (i, line) in txt.lines().enumerate() {
                    let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if ty != "user" && ty != "assistant" {
                        continue;
                    }
                    if title.is_empty() {
                        title = v.get("slug").and_then(|s| s.as_str()).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_default();
                        created = ts_to_millis(&v);
                    }
                    let at = ts_to_millis(&v);
                    for m in claude_line_to_messages(&v, at) {
                        // Skip Claude's internal command/meta noise in the very first user line.
                        if i == 0 && m.role == Role::User && m.parts.iter().all(|p| matches!(p, Part::Text { text } if text.starts_with("<"))) {
                            continue;
                        }
                        msgs.push(m);
                    }
                }
                if msgs.iter().filter(|m| m.role == Role::User || m.role == Role::Assistant).count() < 2 {
                    continue;
                }
                if title.is_empty() {
                    title = msgs
                        .iter()
                        .find(|m| m.role == Role::User)
                        .and_then(|m| m.parts.iter().find_map(|p| if let Part::Text { text } = p { Some(text.clone()) } else { None }))
                        .map(|t| t.lines().next().unwrap_or("").chars().take(60).collect())
                        .unwrap_or_else(|| "Imported chat".into());
                }
                let tag = format!("{title} ⤵");
                if existing.contains(&tag) {
                    continue; // already imported
                }
                let mut s = store.create_session(&project.id)?;
                s.title = tag;
                s.created_at = created;
                s.updated_at = msgs.last().map(|m| m.created_at).unwrap_or(created);
                store.save_session(&s)?;
                for m in &msgs {
                    store.append_message(&s.id, m)?;
                    messages += 1;
                }
                res.chats += 1;
            }
        }
        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_blocks_map_to_parts() {
        let v = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": { "role": "assistant", "content": [
                { "type": "thinking", "thinking": "let me think" },
                { "type": "text", "text": "hello" },
                { "type": "tool_use", "id": "t1", "name": "read", "input": {"path": "a.rs"} }
            ]}
        });
        let msgs = claude_line_to_messages(&v, 0);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert!(matches!(msgs[0].parts[0], Part::Thinking { .. }));
        assert!(matches!(msgs[0].parts[2], Part::ToolCall { .. }));

        let u = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": "ok" },
                { "type": "text", "text": "thanks" }
            ]}
        });
        let m2 = claude_line_to_messages(&u, 0);
        assert_eq!(m2.len(), 2);
        assert_eq!(m2[0].role, Role::Tool); // tool result split out
        assert_eq!(m2[1].role, Role::User);
    }

    #[test]
    #[ignore] // reads real ~/.claude data
    fn detect_real() {
        for t in ["claude_code", "codex", "opencode"] {
            println!("{t}: {} roots", scan_roots(t).len());
        }
    }
}

//! Import of projects, chats and gateways from other coding agents:
//! - Claude Code: `~/.claude/projects/*/*.jsonl`
//! - Codex: `~/.codex/sessions/**/*.jsonl` (+ archived), providers from `~/.codex/config.toml`
//! - OpenCode: `~/.local/share/opencode/opencode.db` (SQLite), providers from `~/.config/opencode/opencode.json`

use crate::engine::Engine;
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use xode_core::store::Store;
use xode_core::config::{ApiKind, Gateway, ModelInfo};
use xode_core::types::{Message, MsgKind, Part, Role};

#[derive(Debug, Clone, Serialize)]
pub struct ImportSource {
    /// Stable key: claude_code | codex | opencode
    pub tool: String,
    pub label: String,
    pub available: bool,
    pub projects: usize,
    pub chats: usize,
    pub gateways: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub projects: usize,
    pub chats: usize,
    pub messages: usize,
    pub gateways: usize,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn claude_dir() -> PathBuf {
    home().join(".claude").join("projects")
}

fn codex_dir() -> PathBuf {
    home().join(".codex")
}

fn opencode_db() -> PathBuf {
    home().join(".local").join("share").join("opencode").join("opencode.db")
}

fn opencode_config() -> Option<PathBuf> {
    let d = home().join(".config").join("opencode");
    ["opencode.json", "opencode.jsonc"].iter().map(|f| d.join(f)).find(|p| p.is_file())
}

/// A directory worth turning into a project: exists, not `/`, not inside `.git` or a temp dir.
fn good_root(s: &str) -> bool {
    let p = Path::new(s);
    if !p.is_absolute() || !p.is_dir() || p.parent().is_none() {
        return false;
    }
    if p.components().any(|c| c.as_os_str() == ".git") || in_agent_data(p) {
        return false;
    }
    let tmp = std::env::temp_dir();
    !(p.starts_with(&tmp) || s.starts_with("/tmp/") || s.starts_with("/private/") || s.starts_with("/var/folders/"))
}

/// Inside another agent's own data folder (`~/.codex/visualizations/<uuid>` etc.).
fn in_agent_data(p: &Path) -> bool {
    let h = home();
    [h.join(".codex"), h.join(".claude"), h.join(".local").join("share").join("opencode"), h.join(".config").join("opencode")].iter().any(|d| p.starts_with(d))
}

/// Project folders a tool has worked in.
fn scan_roots(tool: &str) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    match tool {
        "claude_code" => {
            let Ok(rd) = std::fs::read_dir(claude_dir()) else { return roots };
            for e in rd.flatten().filter(|e| e.path().is_dir()) {
                if let Some(cwd) = project_cwd(&e.path()) {
                    roots.insert(cwd);
                }
            }
        }
        "codex" => {
            for f in codex_files() {
                if let Some(cwd) = codex_meta_cwd(&f) {
                    roots.insert(cwd);
                }
            }
        }
        "opencode" => {
            if let Ok(c) = open_opencode() {
                for sql in ["SELECT worktree FROM project", "SELECT DISTINCT directory FROM session WHERE parent_id IS NULL"] {
                    if let Ok(mut st) = c.prepare(sql) {
                        if let Ok(rows) = st.query_map([], |r| r.get::<_, String>(0)) {
                            roots.extend(rows.flatten());
                        }
                    }
                }
            }
        }
        _ => {}
    }
    roots.retain(|r| good_root(r));
    roots
}

/// Read a Claude project dir's jsonl files and return the real cwd they recorded.
fn project_cwd(dir: &Path) -> Option<String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false)).collect();
    files.sort();
    files.iter().find_map(|f| jsonl_cwd(f))
}

fn jsonl_cwd(f: &Path) -> Option<String> {
    let txt = std::fs::read_to_string(f).ok()?;
    txt.lines().take(40).filter_map(|l| serde_json::from_str::<Value>(l).ok()).find_map(|v| v.get("cwd").and_then(|c| c.as_str()).filter(|c| good_root(c)).map(str::to_string))
}

fn codex_files() -> Vec<PathBuf> {
    let mut out = vec![];
    for d in [codex_dir().join("sessions"), codex_dir().join("archived_sessions")] {
        walk_jsonl(&d, &mut out, 0);
    }
    out.sort();
    out
}

fn walk_jsonl(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 5 {
            walk_jsonl(&p, out, depth + 1);
        } else if p.extension().map(|x| x == "jsonl").unwrap_or(false) {
            out.push(p);
        }
    }
}

fn codex_meta_cwd(f: &Path) -> Option<String> {
    use std::io::BufRead;
    let r = std::io::BufReader::new(std::fs::File::open(f).ok()?);
    let line = r.lines().next()?.ok()?;
    let v: Value = serde_json::from_str(&line).ok()?;
    v.pointer("/payload/cwd").and_then(|c| c.as_str()).map(str::to_string)
}

fn open_opencode() -> rusqlite::Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(opencode_db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
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
                if let Some(t) = b.get("thinking").and_then(|t| t.as_str()).or_else(|| b.get("text").and_then(|t| t.as_str())).filter(|t| !t.trim().is_empty()) {
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

/// Append a part, merging into the previous message when it has the same role
/// (assistant text + tool calls, or several tool results in a row).
fn push_part(msgs: &mut Vec<Message>, role: Role, part: Part, at: i64) {
    if role != Role::User {
        if let Some(last) = msgs.last_mut() {
            if last.role == role {
                last.parts.push(part);
                return;
            }
        }
    }
    msgs.push(Message { id: new_id(), role, parts: vec![part], kind: MsgKind::Normal, segment: 0, meta: None, created_at: at });
}

/// Chat title: the first line of the first real prompt (not a slash command or injected tag).
fn first_user_line(msgs: &[Message]) -> String {
    msgs.iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| if let Part::Text { text } = p { Some(text.trim()) } else { None })
        .flat_map(|t| t.lines())
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('<') && !l.starts_with('/'))
        .map(|l| l.chars().take(60).collect())
        .unwrap_or_else(|| "Imported chat".into())
}

fn bad_title(t: &str) -> bool {
    let t = t.trim();
    t.is_empty() || t.starts_with('<') || t.starts_with('/') || t.starts_with("New session")
}

/// Add (or find) the project for a folder; the home folder is named "Home".
fn ensure_project(store: &Store, root: &str) -> Result<xode_core::store::Project> {
    store.add_project(root, (Path::new(root) == home()).then_some("Home"))
}

/// Agent-injected context that is not a real user prompt.
fn is_injected(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with('<') || t.starts_with("# AGENTS.md") || t.starts_with("# Context from my IDE")
}

/// One chat ready to be written: messages plus the folder it ran in.
struct Chat {
    cwd: String,
    title: String,
    created: i64,
    msgs: Vec<Message>,
}

fn parse_claude(f: &Path, cwd: &str) -> Option<Chat> {
    let txt = std::fs::read_to_string(f).ok()?;
    let mut msgs: Vec<Message> = vec![];
    let mut title = String::new();
    let mut created = 0;
    for line in txt.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty == "summary" && title.is_empty() {
            title = v.get("summary").and_then(|s| s.as_str()).unwrap_or("").chars().take(80).collect();
            continue;
        }
        if ty != "user" && ty != "assistant" || v.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) {
            continue;
        }
        let at = ts_to_millis(&v);
        if created == 0 {
            created = at;
        }
        for m in claude_line_to_messages(&v, at) {
            if m.role == Role::User && m.parts.iter().all(|p| matches!(p, Part::Text { text } if is_injected(text))) {
                continue;
            }
            // Claude streams one assistant block per line: merge them back into one turn.
            match msgs.last_mut() {
                Some(last) if last.role == m.role && m.role != Role::User => last.parts.extend(m.parts),
                _ => msgs.push(m),
            }
        }
    }
    Some(Chat { cwd: cwd.to_string(), title: if bad_title(&title) { first_user_line(&msgs) } else { title }, created, msgs })
}

fn parse_codex(f: &Path) -> Option<Chat> {
    let txt = std::fs::read_to_string(f).ok()?;
    let mut cwd = String::new();
    let mut created = 0;
    let mut msgs: Vec<Message> = vec![];
    let mut names: std::collections::HashMap<String, String> = Default::default();
    for line in txt.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let at = ts_to_millis(&v);
        let p = v.get("payload").cloned().unwrap_or(Value::Null);
        match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "session_meta" => {
                cwd = p.get("cwd").and_then(|c| c.as_str()).unwrap_or("").to_string();
                created = at;
            }
            "response_item" => {
                let s = |k: &str| p.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                match p.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "message" => {
                        let role = match s("role").as_str() {
                            "user" => Role::User,
                            "assistant" => Role::Assistant,
                            _ => continue,
                        };
                        let text = p.get("content").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                        if text.trim().is_empty() || role == Role::User && is_injected(&text) {
                            continue;
                        }
                        push_part(&mut msgs, role, Part::text(&text), at);
                    }
                    "reasoning" => {
                        let t = p.get("summary").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                        if !t.trim().is_empty() {
                            push_part(&mut msgs, Role::Assistant, Part::Thinking { text: t }, at);
                        }
                    }
                    "function_call" | "custom_tool_call" | "local_shell_call" => {
                        let id = s("call_id");
                        let name = if s("name").is_empty() { "shell".to_string() } else { s("name") };
                        let args = match p.get("arguments").or_else(|| p.get("input")).or_else(|| p.get("action")) {
                            Some(Value::String(a)) => serde_json::from_str(a).unwrap_or(Value::String(a.clone())),
                            Some(o) => o.clone(),
                            None => Value::Null,
                        };
                        names.insert(id.clone(), name.clone());
                        push_part(&mut msgs, Role::Assistant, Part::ToolCall { id, name, args }, at);
                    }
                    "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
                        let id = s("call_id");
                        let content = match p.get("output") {
                            Some(Value::String(o)) => o.clone(),
                            Some(o) => o.get("content").and_then(|c| c.as_str()).map(str::to_string).unwrap_or_else(|| o.to_string()),
                            None => String::new(),
                        };
                        let name = names.get(&id).cloned().unwrap_or_default();
                        push_part(&mut msgs, Role::Tool, Part::ToolResult { id, name, content, is_error: false }, at);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Some(Chat { title: first_user_line(&msgs), cwd, created, msgs })
}

fn opencode_chats(c: &rusqlite::Connection) -> Result<Vec<Chat>> {
    let mut st = c.prepare("SELECT id, directory, title, time_created FROM session WHERE parent_id IS NULL AND time_archived IS NULL ORDER BY time_created")?;
    let sessions: Vec<(String, String, String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.flatten().collect();
    let mut mst = c.prepare("SELECT id, data FROM message WHERE session_id=?1 ORDER BY time_created, id")?;
    let mut pst = c.prepare("SELECT message_id, data FROM part WHERE session_id=?1 ORDER BY time_created, id")?;
    let mut out = vec![];
    for (sid, dir, title, created) in sessions {
        let roles: Vec<(String, String, i64)> = mst
            .query_map([&sid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .flatten()
            .filter_map(|(id, d)| {
                let v: Value = serde_json::from_str(&d).ok()?;
                let at = v.pointer("/time/created").and_then(|t| t.as_i64()).unwrap_or(created);
                Some((id, v.get("role")?.as_str()?.to_string(), at))
            })
            .collect();
        let mut parts: std::collections::HashMap<String, Vec<Value>> = Default::default();
        for (mid, d) in pst.query_map([&sid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.flatten() {
            if let Ok(v) = serde_json::from_str::<Value>(&d) {
                parts.entry(mid).or_default().push(v);
            }
        }
        let mut msgs: Vec<Message> = vec![];
        for (mid, role, at) in roles {
            let role = if role == "user" { Role::User } else { Role::Assistant };
            for p in parts.remove(&mid).unwrap_or_default() {
                let s = |k: &str| p.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                match s("type").as_str() {
                    "text" if !p.get("synthetic").and_then(|x| x.as_bool()).unwrap_or(false) && !s("text").trim().is_empty() => {
                        push_part(&mut msgs, role, Part::text(&s("text")), at);
                    }
                    "reasoning" if !s("text").trim().is_empty() => push_part(&mut msgs, Role::Assistant, Part::Thinking { text: s("text") }, at),
                    "tool" => {
                        let id = s("callID");
                        let name = s("tool");
                        let st = p.get("state").cloned().unwrap_or(Value::Null);
                        push_part(&mut msgs, Role::Assistant, Part::ToolCall { id: id.clone(), name: name.clone(), args: st.get("input").cloned().unwrap_or(Value::Null) }, at);
                        let status = st.get("status").and_then(|x| x.as_str()).unwrap_or("");
                        let content = st.get("output").or_else(|| st.get("error")).and_then(|x| x.as_str()).unwrap_or("").to_string();
                        push_part(&mut msgs, Role::Tool, Part::ToolResult { id, name, content, is_error: status == "error" }, at);
                    }
                    "file" => {
                        let url = s("url");
                        if let Some((mime, data)) = url.strip_prefix("data:").and_then(|u| u.split_once(";base64,")) {
                            if mime.starts_with("image/") {
                                push_part(&mut msgs, role, Part::Image { mime: mime.to_string(), data: data.to_string() }, at);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let title = if bad_title(&title) { first_user_line(&msgs) } else { title };
        out.push(Chat { cwd: dir, title, created, msgs });
    }
    Ok(out)
}

fn collect_chats(tool: &str) -> Result<Vec<Chat>> {
    Ok(match tool {
        "claude_code" => {
            let mut v = vec![];
            for pdir in std::fs::read_dir(claude_dir())?.flatten().filter(|e| e.path().is_dir()) {
                for f in std::fs::read_dir(pdir.path())?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false)) {
                    if let Some(cwd) = jsonl_cwd(&f).or_else(|| project_cwd(&pdir.path())) {
                        v.extend(parse_claude(&f, &cwd));
                    }
                }
            }
            v
        }
        "codex" => codex_files().iter().filter_map(|f| parse_codex(f)).collect(),
        "opencode" => opencode_chats(&open_opencode()?)?,
        _ => vec![],
    })
}

/// Write chats as sessions (skipping ones imported before); their folders become projects.
fn save_chats(store: &Store, list: Vec<Chat>, res: &mut ImportResult) -> Result<()> {
    let existing: BTreeSet<(String, i64)> = store.sessions(None)?.into_iter().map(|s| (s.title, s.created_at)).collect();
    for chat in list {
        if !good_root(&chat.cwd) || chat.msgs.iter().filter(|m| m.role != Role::Tool).count() < 2 {
            continue;
        }
        if existing.contains(&(chat.title.clone(), chat.created)) {
            continue; // already imported
        }
        let project = ensure_project(store, &chat.cwd)?;
        let mut s = store.create_session(&project.id)?;
        s.title = chat.title;
        s.created_at = chat.created;
        s.updated_at = chat.msgs.last().map(|m| m.created_at).unwrap_or(chat.created);
        s.cwd = Some(chat.cwd);
        store.save_session(&s)?;
        store.append_messages(&s.id, &chat.msgs)?;
        res.messages += chat.msgs.len();
        res.chats += 1;
    }
    Ok(())
}

// ---------------------------------------------------------------- gateways

/// Strip `//` and `/* */` comments (outside strings) so JSONC parses.
fn strip_jsonc(s: &str) -> String {
    let (mut out, mut it, mut in_str) = (String::with_capacity(s.len()), s.chars().peekable(), false);
    while let Some(c) = it.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = it.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && it.peek() == Some(&'/') {
            while it.next().is_some_and(|n| n != '\n') {}
            out.push('\n');
        } else if c == '/' && it.peek() == Some(&'*') {
            it.next();
            let mut prev = ' ';
            for n in it.by_ref() {
                if prev == '*' && n == '/' {
                    break;
                }
                prev = n;
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `{env:NAME}` (OpenCode) or a bare env var name → its value.
fn resolve_key(raw: &str) -> String {
    match raw.strip_prefix("{env:").and_then(|r| r.strip_suffix('}')) {
        Some(name) => std::env::var(name).unwrap_or_default(),
        None => raw.to_string(),
    }
}

fn scan_gateways(tool: &str) -> Vec<Gateway> {
    let mut out = vec![];
    match tool {
        "opencode" => {
            let Some(p) = opencode_config() else { return out };
            let Ok(txt) = std::fs::read_to_string(p) else { return out };
            let Ok(v) = serde_json::from_str::<Value>(&strip_jsonc(&txt)) else { return out };
            for (key, pv) in v.get("provider").and_then(|x| x.as_object()).into_iter().flatten() {
                let Some(url) = pv.pointer("/options/baseURL").and_then(|x| x.as_str()) else { continue };
                let npm = pv.get("npm").and_then(|x| x.as_str()).unwrap_or("");
                let models = pv
                    .get("models")
                    .and_then(|m| m.as_object())
                    .map(|m| {
                        m.iter()
                            .map(|(id, mv)| ModelInfo {
                                id: mv.get("id").and_then(|x| x.as_str()).unwrap_or(id).to_string(),
                                context: mv.pointer("/limit/context").and_then(|x| x.as_u64()).unwrap_or(0),
                                vision: mv.pointer("/modalities/input").and_then(|x| x.as_array()).is_some_and(|a| a.iter().any(|i| i == "image")) || mv.get("attachment").and_then(|x| x.as_bool()).unwrap_or(false),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                out.push(Gateway {
                    name: pv.get("name").and_then(|x| x.as_str()).unwrap_or(key).to_string(),
                    url: url.to_string(),
                    kind: if npm.contains("anthropic") { ApiKind::Anthropic } else { ApiKind::Openai },
                    api_key: resolve_key(pv.pointer("/options/apiKey").and_then(|x| x.as_str()).unwrap_or("")),
                    flavor: "opencode".into(),
                    models,
                    ..Default::default()
                });
            }
        }
        "codex" => {
            let Ok(txt) = std::fs::read_to_string(codex_dir().join("config.toml")) else { return out };
            let Ok(v) = txt.parse::<toml::Value>() else { return out };
            let (def_provider, def_model) = (v.get("model_provider").and_then(|x| x.as_str()).unwrap_or(""), v.get("model").and_then(|x| x.as_str()).unwrap_or(""));
            for (key, pv) in v.get("model_providers").and_then(|x| x.as_table()).into_iter().flatten() {
                let Some(url) = pv.get("base_url").and_then(|x| x.as_str()) else { continue };
                let header = |h: &str| pv.get("http_headers").and_then(|t| t.as_table()).and_then(|t| t.iter().find(|(k, _)| k.eq_ignore_ascii_case(h))).and_then(|(_, x)| x.as_str()).map(str::to_string);
                let api_key = pv
                    .get("env_key")
                    .and_then(|x| x.as_str())
                    .and_then(|e| std::env::var(e).ok())
                    .or_else(|| pv.get("experimental_bearer_token").and_then(|x| x.as_str()).map(str::to_string))
                    .or_else(|| header("authorization").map(|a| a.trim_start_matches("Bearer ").to_string()))
                    .or_else(|| header("x-api-key"))
                    .unwrap_or_default();
                let models = if key == def_provider && !def_model.is_empty() { vec![ModelInfo { id: def_model.into(), context: 0, vision: false }] } else { vec![] };
                out.push(Gateway {
                    name: pv.get("name").and_then(|x| x.as_str()).unwrap_or(key).to_string(),
                    url: url.to_string(),
                    api_key,
                    flavor: "codex".into(),
                    models,
                    ..Default::default()
                });
            }
        }
        _ => {}
    }
    out
}

fn norm_url(u: &str) -> String {
    xode_core::provider::base_url(u).to_lowercase()
}

impl Engine {
    pub fn import_detect(&self) -> Vec<ImportSource> {
        let count_claude = || {
            std::fs::read_dir(claude_dir())
                .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| std::fs::read_dir(e.path()).map(|r| r.flatten().filter(|f| f.path().extension().map(|x| x == "jsonl").unwrap_or(false)).count()).unwrap_or(0)).sum())
                .unwrap_or(0)
        };
        let count_opencode = || open_opencode().ok().and_then(|c| c.query_row("SELECT count(*) FROM session WHERE parent_id IS NULL AND time_archived IS NULL", [], |r| r.get::<_, i64>(0)).ok()).unwrap_or(0) as usize;
        [
            ("claude_code", "Claude Code", claude_dir(), count_claude()),
            ("codex", "Codex", codex_dir(), codex_files().len()),
            ("opencode", "OpenCode", opencode_db(), count_opencode()),
        ]
        .into_iter()
        .map(|(tool, label, path, chats)| ImportSource {
            tool: tool.into(),
            label: label.into(),
            available: path.exists(),
            projects: scan_roots(tool).len(),
            chats,
            gateways: scan_gateways(tool).len(),
            path: path.to_string_lossy().into(),
        })
        .collect()
    }

    /// Import a tool's projects, chats (their folders become projects too) and gateways.
    pub fn import_run(&self, tool: &str, projects: bool, chats: bool, gateways: bool) -> Result<ImportResult> {
        let mut res = ImportResult { projects: 0, chats: 0, messages: 0, gateways: 0 };
        let store: &Store = &self.store;
        let before: BTreeSet<String> = store.projects()?.into_iter().map(|p| p.id).collect();
        // Earlier versions produced junk: chats titled "… ⤵" with raw tool noise, and projects
        // for `.git` / agent data folders. Drop them; this import brings clean copies.
        if chats {
            for s in store.sessions(None)? {
                if s.title.ends_with(" \u{2935}") {
                    store.delete_session(&s.id)?;
                }
            }
        }
        for p in store.projects()? {
            let r = Path::new(&p.root);
            if (r.components().any(|c| c.as_os_str() == ".git") || in_agent_data(r)) && store.sessions(Some(&p.id))?.is_empty() {
                store.remove_project(&p.id)?;
            }
        }
        if projects {
            for root in scan_roots(tool) {
                ensure_project(store, &root)?;
            }
        }
        if chats {
            save_chats(store, collect_chats(tool)?, &mut res)?;
        }
        if gateways {
            let mut cfg = self.config();
            for g in scan_gateways(tool) {
                if cfg.gateways.iter().any(|x| norm_url(&x.url) == norm_url(&g.url)) {
                    continue;
                }
                if cfg.selected.gateway.is_empty() {
                    if let Some(m) = g.models.first() {
                        cfg.selected.gateway = g.id.clone();
                        cfg.selected.model = m.id.clone();
                    }
                }
                cfg.gateways.push(g);
                res.gateways += 1;
            }
            if res.gateways > 0 {
                self.set_config(cfg)?;
            }
        }
        res.projects = store.projects()?.into_iter().filter(|p| !before.contains(&p.id)).count();
        Ok(res)
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
    fn jsonc_and_roots() {
        let v: Value = serde_json::from_str(&strip_jsonc("{ // c\n \"a\": \"http://x//y\", /* b */ \"b\": 1 }")).unwrap();
        assert_eq!(v["a"], "http://x//y");
        assert!(!good_root("/"));
        assert!(!good_root(&home().join("x/.git").to_string_lossy()));
        assert!(bad_title("<lang primary=ru>x") && !bad_title("fix login"));
    }

    #[test]
    #[ignore] // reads real agent data, writes a temp store
    fn import_real_into_temp() {
        let dir = std::env::temp_dir().join(format!("xode-import-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("x.db")).unwrap();
        for t in ["claude_code", "codex", "opencode"] {
            let mut res = ImportResult { projects: 0, chats: 0, messages: 0, gateways: 0 };
            save_chats(&store, collect_chats(t).unwrap(), &mut res).unwrap();
            let again = { let mut r = ImportResult { projects: 0, chats: 0, messages: 0, gateways: 0 }; save_chats(&store, collect_chats(t).unwrap(), &mut r).unwrap(); r.chats };
            println!("{t}: {} chats, {} msgs (re-run adds {again})", res.chats, res.messages);
        }
        let s = store.sessions(None).unwrap();
        let m = store.messages(&s[0].id).unwrap();
        println!("sample {:?}: {} msgs, roles {:?}", s[0].title, m.len(), m.iter().take(6).map(|m| m.role).collect::<Vec<_>>());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore] // reads real ~/.claude data
    fn detect_real() {
        for t in ["claude_code", "codex", "opencode"] {
            println!("{t}: {} roots, {} gateways", scan_roots(t).len(), scan_gateways(t).len());
        }
        let oc = opencode_chats(&open_opencode().unwrap()).unwrap();
        let cx: Vec<Chat> = codex_files().iter().filter_map(|f| parse_codex(f)).collect();
        for (n, l) in [("opencode", &oc), ("codex", &cx)] {
            let ok = l.iter().filter(|c| good_root(&c.cwd) && c.msgs.len() >= 2).count();
            println!("{n}: {} chats, {ok} importable, {} msgs", l.len(), l.iter().map(|c| c.msgs.len()).sum::<usize>());
            if let Some(c) = l.iter().find(|c| c.msgs.len() > 4) {
                println!("  e.g. {:?} in {} — {} msgs", c.title, c.cwd, c.msgs.len());
            }
        }
    }
}

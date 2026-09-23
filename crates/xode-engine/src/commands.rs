//! Slash commands shared by desktop and CLI.
use crate::api::*;
use crate::engine::Engine;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use xode_core::agent::Queued;
use xode_core::{Message, Mode, MsgKind, Part, Role};

const BUILTIN: &[(&str, &str, &str)] = &[
    ("new", "", "New chat"),
    ("clear", "", "Clear this chat"),
    ("resume", "[query]", "Resume a chat"),
    ("sessions", "", "List chats"),
    ("rename", "<title>", "Rename chat"),
    ("compact", "", "Compact context now"),
    ("goal", "<goal|clear>", "Work until the goal is met"),
    ("connect", "[url] [api key]", "Add a gateway (no url: scan local ports)"),
    ("model", "[name]", "Switch model"),
    ("mode", "[plan|normal]", "Switch mode"),
    ("effort", "[auto|off|low|medium|high]", "Reasoning effort"),
    ("plan", "", "Toggle plan mode"),
    ("context", "", "Context usage"),
    ("tokens", "", "Token totals"),
    ("init", "", "Write project brief"),
    ("undo", "", "Revert last run's file changes"),
    ("redo", "", "Reapply reverted changes"),
    ("export", "", "Export chat as markdown"),
    ("project", "[path]", "Switch project"),
    ("permissions", "", "Permissions"),
    ("mcp", "", "MCP servers"),
    ("tools", "", "Tools"),
    ("theme", "", "Theme"),
    ("config", "", "Settings"),
    ("exit", "", "Quit"),
];

fn custom_dirs(root: Option<&Path>) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(r) = root {
        v.push(r.join(".xode").join("commands"));
    }
    v.push(xode_core::config::data_dir().join("commands"));
    v
}

fn custom_commands(root: Option<&Path>) -> Vec<(String, String, PathBuf)> {
    let mut out = vec![];
    for d in custom_dirs(root) {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "md").unwrap_or(false) {
                let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                if out.iter().any(|(n, _, _): &(String, String, PathBuf)| *n == name) {
                    continue;
                }
                let body = std::fs::read_to_string(&p).unwrap_or_default();
                // Description: frontmatter `description:` or first non-empty line.
                let desc = body
                    .lines()
                    .find_map(|l| l.strip_prefix("description:").map(|s| s.trim().to_string()))
                    .or_else(|| body.lines().find(|l| !l.trim().is_empty() && l.trim() != "---").map(|l| l.trim().to_string()))
                    .unwrap_or_default();
                out.push((name, xode_core::context::clip(&desc, 60), p));
            }
        }
    }
    out
}

pub fn list(root: Option<&Path>) -> Vec<CommandInfo> {
    let mut v: Vec<CommandInfo> = BUILTIN
        .iter()
        .map(|(n, a, d)| CommandInfo { name: n.to_string(), args: a.to_string(), description: d.to_string(), source: "builtin".into() })
        .collect();
    for (n, d, _) in custom_commands(root) {
        if !v.iter().any(|c| c.name == n) {
            v.push(CommandInfo { name: n, args: "[args]".into(), description: d, source: "custom".into() });
        }
    }
    v
}

fn notice(s: impl Into<String>) -> Result<CommandResult> {
    Ok(CommandResult::Notice { text: s.into() })
}

fn k(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Commands that need the agent idle: while it runs they wait in the steer queue.
const QUEUED: &[&str] = &["compact", "clear", "undo", "redo", "init"];

pub async fn run(e: &Arc<Engine>, sid: &str, line: &str) -> Result<CommandResult> {
    let line = line.trim().trim_start_matches('/');
    let (cmd, arg) = match line.split_once(char::is_whitespace) {
        Some((c, a)) => (c.to_lowercase(), a.trim().to_string()),
        None => (line.to_lowercase(), String::new()),
    };
    if e.is_running(sid) && QUEUED.contains(&cmd.as_str()) {
        let id = uuid::Uuid::new_v4().to_string();
        e.enqueue(
            sid,
            if cmd == "compact" { Queued::Compact { id } } else { Queued::Cmd { id, line: format!("/{line}") } },
        );
        return Ok(CommandResult::Done);
    }
    match cmd.as_str() {
        "new" => {
            let p = e.project_of(sid)?;
            let s = e.new_session(&p.id)?;
            Ok(CommandResult::SwitchSession { session_id: s.id })
        }
        "clear" => {
            e.clear_session(sid)?;
            Ok(CommandResult::SwitchSession { session_id: sid.into() })
        }
        "resume" | "sessions" => {
            if arg.is_empty() {
                return Ok(CommandResult::Open { panel: "sessions".into() });
            }
            let p = e.project_of(sid)?;
            let q = arg.to_lowercase();
            let found = e
                .sessions(Some(&p.id))?
                .into_iter()
                .find(|s| s.id.starts_with(&arg) || s.title.to_lowercase().contains(&q));
            match found {
                Some(s) => Ok(CommandResult::SwitchSession { session_id: s.id }),
                None => notice(format!("no chat matching \"{arg}\"")),
            }
        }
        "rename" => {
            if arg.is_empty() {
                return notice("usage: /rename <title>");
            }
            e.rename_session(sid, &arg)?;
            notice(format!("renamed to \"{arg}\""))
        }
        "compact" => {
            compact_now(e, sid)?;
            Ok(CommandResult::Done)
        }
        "goal" => {
            let mut s = e.session(sid)?;
            if arg.is_empty() {
                return notice(match &s.goal {
                    Some(g) => format!("goal: {g}"),
                    None => "no goal set · /goal <text>".into(),
                });
            }
            if matches!(arg.as_str(), "clear" | "off" | "none" | "stop") {
                s.goal = None;
                e.store.save_session(&s)?;
                return notice("goal cleared");
            }
            s.goal = Some(arg.clone());
            e.store.save_session(&s)?;
            if !e.is_running(sid) {
                e.send(sid, format!("Goal: {arg}"), vec![]).await?;
                Ok(CommandResult::Done)
            } else {
                notice(format!("goal set: {arg}"))
            }
        }
        "connect" => connect(e, sid, &arg).await,
        "model" => {
            let cfg = e.config();
            if arg.is_empty() {
                let mut lines = vec![];
                for g in &cfg.gateways {
                    for m in &g.models {
                        lines.push(format!("{} · {}", m.id, g.name));
                    }
                }
                if lines.is_empty() {
                    return Ok(CommandResult::Open { panel: "settings:gateway".into() });
                }
                return notice(lines.join("\n"));
            }
            let q = arg.to_lowercase();
            for g in &cfg.gateways {
                if let Some(m) = g.models.iter().find(|m| m.id.to_lowercase() == q).or_else(|| g.models.iter().find(|m| m.id.to_lowercase().contains(&q))) {
                    e.set_model(sid, &g.id, &m.id)?;
                    return notice(format!("model: {}", m.id));
                }
            }
            notice(format!("no model matching \"{arg}\""))
        }
        "mode" | "plan" => {
            let s = e.session(sid)?;
            let mode = match (cmd.as_str(), arg.as_str()) {
                ("plan", _) => {
                    if s.mode == Mode::Plan {
                        Mode::Normal
                    } else {
                        Mode::Plan
                    }
                }
                (_, "plan") => Mode::Plan,
                (_, "normal") => Mode::Normal,
                _ => {
                    if s.mode == Mode::Plan {
                        Mode::Normal
                    } else {
                        Mode::Plan
                    }
                }
            };
            e.set_mode(sid, mode)?;
            notice(format!("mode: {}", if mode == Mode::Plan { "plan" } else { "normal" }))
        }
        "effort" | "think" | "reasoning" => {
            let mut cfg = e.config();
            let cur = |c: &xode_core::Config| match c.generation.effort() {
                "" => "auto".to_string(),
                x => x.to_string(),
            };
            if arg.is_empty() {
                return notice(format!("effort: {}", cur(&cfg)));
            }
            let v = match arg.to_lowercase().as_str() {
                "auto" | "default" => "",
                "off" | "none" | "no" => "off",
                "low" | "l" => "low",
                "medium" | "med" | "m" => "medium",
                "high" | "h" | "max" => "high",
                _ => return notice("usage: /effort auto|off|low|medium|high"),
            };
            cfg.generation.reasoning_effort = v.into();
            e.set_config(cfg.clone())?;
            notice(format!("effort: {}", cur(&cfg)))
        }
        "context" => Ok(CommandResult::Open { panel: "context".into() }),
        "tokens" => {
            let v = e.context_view(sid)?;
            let s = e.session(sid)?;
            let mut t = format!(
                "context {} / {} (compacts at {}) · session in {} · out {} · {} compactions · {} tool calls",
                k(v.used),
                k(v.limit),
                k(v.threshold),
                k(s.tokens_in),
                k(s.tokens_out),
                s.compactions,
                s.tool_calls
            );
            for sec in v.sections.iter().filter(|s| s.tokens > 0) {
                t.push_str(&format!("\n  {:<13}{}", sec.label, k(sec.tokens)));
            }
            notice(t)
        }
        "init" => {
            let p = e.project_of(sid)?;
            let root = PathBuf::from(&p.root);
            let _ = std::fs::create_dir_all(root.join(".xode"));
            let cfg = e.config();
            let pm = root.join(&cfg.compaction.path_file);
            if !pm.exists() {
                let _ = std::fs::write(&pm, xode_core::compaction::PATH_HEADER);
            }
            ensure_gitignore(&root);
            let _ = e.index(&p);
            e.send(
                sid,
                "Survey this project quickly (use `code` map/outline, read only key files like manifests and README). Then write `.xode/BRIEF.md`, max 30 lines: purpose, stack, layout (key dirs/files), build/run/test commands, conventions. Terse bullet points, no fluff.".into(),
                vec![],
            )
            .await?;
            Ok(CommandResult::Done)
        }
        "undo" | "redo" => undo_redo(e, sid, cmd == "undo"),
        "export" => {
            let s = e.session(sid)?;
            let p = e.project_of(sid)?;
            let md = export_markdown(&s.title, &e.messages(sid)?);
            let dir = PathBuf::from(&p.root).join(".xode").join("exports");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join(format!("{}-{}.md", chrono::Local::now().format("%Y%m%d-%H%M"), slug(&s.title)));
            std::fs::write(&file, &md)?;
            Ok(CommandResult::Export { markdown: md, path: file.to_string_lossy().to_string() })
        }
        "project" => {
            if arg.is_empty() {
                return Ok(CommandResult::Open { panel: "project".into() });
            }
            let p = e.add_project(&arg)?;
            let s = e.new_session(&p.id)?;
            Ok(CommandResult::SwitchSession { session_id: s.id })
        }
        "permissions" | "mcp" | "tools" | "theme" => Ok(CommandResult::Open { panel: format!("settings:{cmd}") }),
        "config" | "settings" => Ok(CommandResult::Open { panel: "settings".into() }),
        "exit" | "quit" => Ok(CommandResult::Exit),
        _ => {
            let root = e.project_of(sid).ok().map(|p| PathBuf::from(p.root));
            if let Some((_, _, path)) = custom_commands(root.as_deref()).into_iter().find(|(n, _, _)| *n == cmd) {
                let body = std::fs::read_to_string(path)?;
                let body = strip_frontmatter(&body);
                let prompt = if body.contains("$ARGUMENTS") {
                    body.replace("$ARGUMENTS", &arg)
                } else if arg.is_empty() {
                    body
                } else {
                    format!("{body}\n\n{arg}")
                };
                e.send(sid, prompt, vec![]).await?;
                return Ok(CommandResult::Done);
            }
            notice(format!("unknown command /{cmd}"))
        }
    }
}

pub(crate) fn compact_now(e: &Arc<Engine>, sid: &str) -> Result<()> {
    let rt_s = e.srt(sid);
    if rt_s.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
        anyhow::bail!("already running");
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    *rt_s.cancel.lock() = Some(cancel);
    let e2 = e.clone();
    let sid2 = sid.to_string();
    e2.emit_state(&sid2, true);
    tokio::spawn(async move {
        let res: Result<()> = async {
            let rt = e2.runtime(&sid2).await?;
            let mut sess = e2.session(&sid2)?;
            let mut all = e2.messages(&sid2)?;
            let active = xode_core::context::active_messages(&all, sess.segment);
            if active.is_empty() {
                e2.emit_notice(&sid2, "nothing to compact");
                return Ok(());
            }
            let v = e2.context_view(&sid2).map(|v| v.used).unwrap_or(0);
            rt.compact(&mut sess, &mut all, v).await
        }
        .await;
        if let Err(err) = &res {
            e2.emit_notice(&sid2, format!("compaction failed: {err}"));
        }
        e2.after_run(&sid2, res.is_err()).await;
    });
    Ok(())
}

fn undo_redo(e: &Arc<Engine>, sid: &str, undo: bool) -> Result<CommandResult> {
    if e.is_running(sid) {
        return notice("stop the current run first");
    }
    let rt = e.srt(sid);
    let set = if undo { rt.undo.lock().pop() } else { rt.redo.lock().pop() };
    let Some(set) = set else {
        return notice(if undo { "nothing to undo" } else { "nothing to redo" });
    };
    let (inverse, n) = apply_snapshot(&set.1)?;
    if undo {
        rt.redo.lock().push((set.0, inverse));
    } else {
        rt.undo.lock().push((set.0, inverse));
    }
    // Let the agent know files changed under it.
    let s = e.session(sid)?;
    let mut m = Message::user(format!(
        "[user {} {} file change(s): {}]",
        if undo { "reverted" } else { "reapplied" },
        n,
        set.1.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(", ")
    ))
    .with_kind(MsgKind::Note);
    m.segment = s.segment;
    e.store.append_message(sid, &m)?;
    notice(format!("{} {n} file(s)", if undo { "reverted" } else { "reapplied" }))
}

/// Writes back the saved contents (None = file did not exist). Returns the inverse set.
pub(crate) fn apply_snapshot(set: &[(String, Option<Vec<u8>>)]) -> Result<(Vec<(String, Option<Vec<u8>>)>, usize)> {
    let mut inverse = vec![];
    let mut n = 0;
    for (path, prev) in set.iter().rev() {
        let cur = std::fs::read(path).ok();
        inverse.push((path.clone(), cur));
        match prev {
            Some(bytes) => {
                if let Some(d) = Path::new(path).parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                std::fs::write(path, bytes)?;
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
        n += 1;
    }
    Ok((inverse, n))
}

fn export_markdown(title: &str, msgs: &[Message]) -> String {
    let mut s = format!("# {}\n\n", if title.is_empty() { "Chat" } else { title });
    for m in msgs {
        if matches!(m.kind, MsgKind::Compaction | MsgKind::Note) {
            continue;
        }
        match (m.role, m.kind) {
            (_, MsgKind::Seed) => s.push_str("---\n*context compacted*\n\n"),
            (Role::User, _) => s.push_str(&format!("## User\n\n{}\n\n", m.text())),
            (Role::Assistant, _) => {
                let mut body = String::new();
                for p in &m.parts {
                    match p {
                        Part::Text { text } => body.push_str(&format!("{text}\n\n")),
                        Part::ToolCall { name, args, .. } => body.push_str(&format!("`{name}` `{}`\n\n", xode_core::context::clip(&args.to_string(), 160))),
                        _ => {}
                    }
                }
                if !body.trim().is_empty() {
                    s.push_str(&format!("## Xode\n\n{body}"));
                }
            }
            _ => {}
        }
    }
    s
}

fn slug(s: &str) -> String {
    let v: String = s.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let v = v.split('-').filter(|x| !x.is_empty()).collect::<Vec<_>>().join("-");
    if v.is_empty() {
        "chat".into()
    } else {
        v.chars().take(40).collect()
    }
}

fn strip_frontmatter(s: &str) -> String {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim().to_string();
        }
    }
    s.trim().to_string()
}

fn ensure_gitignore(root: &Path) {
    let gi = root.join(".xode").join(".gitignore");
    if !gi.exists() {
        let _ = std::fs::write(gi, "index.db*\nout/\nattachments/\nexports/\n");
    }
}

/// `/connect <url> [key]` adds one gateway; `/connect` scans local ports and adds what it finds.
async fn connect(e: &Arc<Engine>, sid: &str, arg: &str) -> Result<CommandResult> {
    let mut parts = arg.split_whitespace();
    let url = parts.next().unwrap_or("");
    let key = parts.next().unwrap_or("");
    let found = if url.is_empty() {
        let f = e.scan_gateways(vec![]).await;
        if f.is_empty() {
            return notice("no local gateways found · /connect <url> [api key]");
        }
        f
    } else {
        vec![e.detect_gateway(url, key).await?]
    };
    let mut cfg = e.config();
    let norm = |u: &str| u.trim_end_matches('/').to_lowercase();
    let mut lines = vec![];
    let mut first: Option<(String, String)> = None;
    for g in found {
        let model = g.models.first().map(|m| m.id.clone()).unwrap_or_default();
        let id = match cfg.gateways.iter_mut().find(|x| norm(&x.url) == norm(&g.url)) {
            Some(existing) => {
                existing.models = g.models.clone();
                existing.kind = g.kind;
                existing.flavor = g.flavor.clone();
                if !key.is_empty() {
                    existing.api_key = key.to_string();
                }
                existing.enabled = true;
                existing.id.clone()
            }
            None => {
                let id = g.id.clone();
                cfg.gateways.push(g.clone());
                id
            }
        };
        let ctx = g.models.first().map(|m| m.context).unwrap_or(0);
        lines.push(format!(
            "{} · {} model{}{}",
            g.name,
            g.models.len(),
            if g.models.len() == 1 { "" } else { "s" },
            if ctx > 0 { format!(" · {}k ctx", ctx / 1000) } else { String::new() }
        ));
        if first.is_none() && !model.is_empty() {
            first = Some((id, model));
        }
    }
    e.set_config(cfg)?;
    if let Some((gid, model)) = first {
        e.set_model(sid, &gid, &model)?;
        lines.push(format!("using {model}"));
    }
    notice(lines.join("\n"))
}

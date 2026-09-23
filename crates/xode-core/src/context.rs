//! Prompt assembly and token accounting.
use crate::config::Config;
use crate::tokens;
use crate::types::{Message, Mode, MsgKind, Part, Role, ToolSpec};

pub const BASE_PROMPT: &str = "You are Xode, an autonomous coding agent with full access to the user's computer: shell, filesystem, browser and web. Work until the task is completely done, then reply with a short summary.

Context is small and precious:
- Explore with `code` (map/outline/find/refs/read symbol) before reading files. Read files with offset/limit when they are large.
- Never re-read files you already saw unless they changed or you need unseen lines. Trust the working set and PATH notes.
- Prefer `edit` (exact snippet replace) or `code` edit over rewriting whole files. Do not echo file contents back.
- Keep shell output small (filter, use quiet flags). Batch independent tool calls in one turn.
- Be terse in text. No preambles or recaps between tool calls.
Verify your work (build/tests) when possible. If blocked, try another approach before asking the user.
When you create something the user should open (a page, image, report, document), link it in your reply: [name](path/to/file.html), or ![alt](path/to/image.png) to show an image inline. Paths may be relative to the project.";

pub const PLAN_PROMPT: &str = "PLAN MODE: you may only read and research (no edits, no state-changing commands). End with a concise, numbered implementation plan.";

pub struct Env<'a> {
    pub project_root: &'a str,
    pub extra_roots: &'a [String],
    pub cwd: &'a str,
    pub shell: &'a str,
}

pub fn os_line(shell: &str) -> String {
    let os = if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    };
    format!("OS: {os} ({}); shell: {shell}", std::env::consts::ARCH)
}

/// System prompt. Stable within a segment (keeps the server's prompt cache warm).
pub fn system_prompt(cfg: &Config, mode: Mode, env: &Env, brief: &str, path_md: Option<&str>) -> String {
    let mut s = String::with_capacity(4096);
    s.push_str(BASE_PROMPT);
    s.push_str("\n\n");
    s.push_str(&os_line(env.shell));
    s.push_str(&format!("\nProject: {}", env.project_root.replace('\\', "/")));
    for r in env.extra_roots {
        s.push_str(&format!("\nAlso: {}", r.replace('\\', "/")));
    }
    if env.cwd != env.project_root {
        s.push_str(&format!("\ncwd: {}", env.cwd.replace('\\', "/")));
    }
    s.push_str(&format!("\nDate: {}", chrono::Local::now().format("%Y-%m-%d")));
    if !brief.trim().is_empty() {
        s.push_str("\n\n## Repo map\n");
        s.push_str(brief.trim());
    }
    if let Some(p) = path_md {
        if !p.trim().is_empty() {
            s.push_str("\n\n## Project path (.xode/PATH.md, earlier work)\n");
            s.push_str(p.trim());
        }
    }
    if !cfg.system_prompt_extra.trim().is_empty() {
        s.push_str("\n\n");
        s.push_str(cfg.system_prompt_extra.trim());
    }
    if mode == Mode::Plan {
        s.push_str("\n\n");
        s.push_str(PLAN_PROMPT);
    }
    s
}

/// Messages of the active segment, as sent to the model.
pub fn active_messages(all: &[Message], segment: u32) -> Vec<Message> {
    all.iter().filter(|m| m.segment == segment && m.kind != MsgKind::Compaction).cloned().collect()
}

/// Apply token-saving transforms to request messages.
pub fn shape_messages(msgs: &[Message], cfg: &Config) -> Vec<Message> {
    let ts = &cfg.token_saving;
    let n_asst = msgs.iter().filter(|m| m.role == Role::Assistant).count();
    let mut seen_asst = 0usize;
    let mut out = Vec::with_capacity(msgs.len());
    for m in msgs {
        if m.role == Role::Assistant {
            seen_asst += 1;
        }
        let age = n_asst.saturating_sub(seen_asst);
        let mut m = m.clone();
        if ts.stub_old_tool_results_after > 0 && m.role == Role::Tool && age >= ts.stub_old_tool_results_after {
            for p in &mut m.parts {
                if let Part::ToolResult { content, .. } = p {
                    if content.len() > 300 {
                        let lines = content.lines().count();
                        let head: String = content.chars().take(160).collect();
                        *content = format!("{head}… [old result trimmed, {lines} lines]");
                    }
                }
            }
        }
        if ts.drop_old_thinking && !cfg.generation.keep_thinking && m.role == Role::Assistant {
            m.parts.retain(|p| !matches!(p, Part::Thinking { .. }));
        }
        out.push(m);
    }
    out
}

pub fn message_tokens(m: &Message) -> u64 {
    let mut n = 4;
    for p in &m.parts {
        n += match p {
            Part::Text { text } | Part::Thinking { text } => tokens::count(text),
            Part::Image { .. } => 800,
            Part::ToolCall { name, args, .. } => 8 + tokens::count(name) + tokens::count(&args.to_string()),
            Part::ToolResult { content, .. } => 6 + tokens::count(content),
        };
    }
    n
}

pub fn tools_tokens(specs: &[ToolSpec]) -> u64 {
    specs
        .iter()
        .map(|t| 10 + tokens::count(&t.name) + tokens::count(&t.description) + tokens::count(&t.parameters.to_string()))
        .sum()
}

#[derive(Debug, Clone, Default)]
pub struct Breakdown {
    pub system: u64,
    pub tools: u64,
    pub brief: u64,
    pub path: u64,
    pub seed: u64,
    pub messages: u64,
    pub tool_results: u64,
    pub thinking: u64,
    pub attachments: u64,
    /// Tool results by tool: (name, tokens, calls), largest first.
    pub per_tool: Vec<(String, u64, u32)>,
}

impl Breakdown {
    pub fn total(&self) -> u64 {
        self.system + self.tools + self.brief + self.path + self.seed + self.messages + self.tool_results + self.thinking + self.attachments
    }
}

pub fn breakdown(system_base: u64, brief: &str, path: &str, specs: &[ToolSpec], msgs: &[Message]) -> Breakdown {
    let mut b = Breakdown {
        system: system_base,
        tools: tools_tokens(specs),
        brief: tokens::count(brief),
        path: tokens::count(path),
        ..Default::default()
    };
    for m in msgs {
        if m.kind == MsgKind::Seed {
            b.seed += message_tokens(m);
            continue;
        }
        for p in &m.parts {
            match p {
                Part::Text { text } => b.messages += tokens::count(text),
                Part::Thinking { text } => b.thinking += tokens::count(text),
                Part::Image { .. } => b.attachments += 800,
                Part::ToolCall { name, args, .. } => b.messages += 8 + tokens::count(name) + tokens::count(&args.to_string()),
                Part::ToolResult { name, content, .. } => {
                    let t = 6 + tokens::count(content);
                    b.tool_results += t;
                    match b.per_tool.iter_mut().find(|(n, _, _)| n == name) {
                        Some(e) => {
                            e.1 += t;
                            e.2 += 1;
                        }
                        None => b.per_tool.push((name.clone(), t, 1)),
                    }
                }
            }
        }
        b.messages += 4;
    }
    b.per_tool.sort_by(|a, b| b.1.cmp(&a.1));
    b
}

/// Render a message compactly as text (used in seeds and goal judging).
pub fn render_brief(m: &Message, max_chars: usize) -> String {
    let mut s = String::new();
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::System => "system",
    };
    for p in &m.parts {
        match p {
            Part::Text { text } => s.push_str(&format!("{role}: {}\n", clip(text, max_chars))),
            Part::ToolCall { name, args, .. } => s.push_str(&format!("call {name} {}\n", clip(&args.to_string(), 200))),
            Part::ToolResult { name, content, is_error, .. } => s.push_str(&format!(
                "{} {name}: {}\n",
                if *is_error { "error" } else { "result" },
                clip(content, max_chars / 2)
            )),
            _ => {}
        }
    }
    s
}

pub fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

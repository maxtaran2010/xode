//! `xode -p "<prompt>"`: stream a single run to stdout/stderr and exit.
use crate::render::{fmt_ms, fmt_tok};
use crate::view::tool_summary;
use anyhow::Result;
use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use xode_engine::xode_core::AgentEvent;
use xode_engine::{CommandResult, Engine, PermDecision};

const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

#[derive(PartialEq)]
enum Stream {
    None,
    Thinking,
    Text,
}

/// Returns the process exit code.
pub async fn run(engine: Arc<Engine>, session: &str, prompt: String, quiet: bool) -> Result<i32> {
    let color = std::io::stderr().is_terminal();
    let (dim, red, reset) = if color { (DIM, RED, RESET) } else { ("", "", "") };

    let mut rx = engine.subscribe();
    if prompt.trim_start().starts_with('/') {
        let r = engine.command(session, prompt.trim()).await?;
        match r {
            CommandResult::Notice { text } => println!("{text}"),
            CommandResult::Export { markdown, path } => {
                if path.is_empty() {
                    print!("{markdown}");
                } else {
                    std::fs::write(&path, markdown)?;
                    eprintln!("exported to {path}");
                }
            }
            CommandResult::SwitchSession { session_id } => eprintln!("session {session_id}"),
            CommandResult::Open { panel } => eprintln!("'{panel}' is only available in the interactive UI"),
            CommandResult::Done | CommandResult::Exit => {}
        }
        if !engine.is_running(session) {
            return Ok(0);
        }
    }

    if !prompt.trim_start().starts_with('/') {
        engine.send(session, prompt, Vec::new()).await?;
    }

    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut last = Stream::None;
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    let mut code = 0;
    let interactive = std::io::stdin().is_terminal();

    loop {
        let ev = match rx.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(n)) => {
                let _ = writeln!(err, "{dim}[{n} events dropped]{reset}");
                continue;
            }
            Err(RecvError::Closed) => break,
        };
        if ev.session() != session {
            continue;
        }
        match ev {
            AgentEvent::ThinkingDelta { text, .. } => {
                if !quiet {
                    if last != Stream::Thinking && last != Stream::None {
                        let _ = writeln!(err);
                    }
                    let _ = write!(err, "{dim}{text}{reset}");
                    let _ = err.flush();
                }
                last = Stream::Thinking;
            }
            AgentEvent::TextDelta { text, .. } => {
                if last == Stream::Thinking && !quiet {
                    let _ = writeln!(err);
                }
                let _ = write!(out, "{text}");
                let _ = out.flush();
                last = Stream::Text;
            }
            AgentEvent::ToolCallArgs { call_id, args: a, .. } => {
                args.insert(call_id, a);
            }
            AgentEvent::TurnEnd { message, .. } => {
                for (id, _, a) in message.tool_calls() {
                    args.entry(id).or_insert(a);
                }
                if last == Stream::Text {
                    let _ = writeln!(out);
                    let _ = out.flush();
                } else if last == Stream::Thinking && !quiet {
                    let _ = writeln!(err);
                }
                last = Stream::None;
            }
            AgentEvent::ToolResult { call_id, name, is_error, ms, .. } => {
                let a = args.remove(&call_id).unwrap_or(serde_json::Value::Null);
                let mark = if is_error { format!("{red}✗{reset}") } else { "✓".into() };
                let _ = writeln!(err, "{mark} {}{dim}  {}{reset}", tool_summary(&name, &a), fmt_ms(ms));
            }
            AgentEvent::CompactionDone { before, after, .. } => {
                let _ = writeln!(err, "{dim}── context compacted {} → {} ──{reset}", fmt_tok(before), fmt_tok(after));
            }
            AgentEvent::Notice { text, .. } => {
                if !quiet {
                    let _ = writeln!(err, "{dim}{text}{reset}");
                }
            }
            AgentEvent::GoalCheck { done, reason, .. } => {
                if !quiet {
                    let _ = writeln!(err, "{dim}goal {}: {reason}{reset}", if done { "met" } else { "not met" });
                }
            }
            AgentEvent::Error { text, .. } => {
                let _ = writeln!(err, "{red}error:{reset} {text}");
                code = 1;
            }
            AgentEvent::PermissionAsk { req_id, tool, summary, .. } => {
                let decision = if interactive {
                    let _ = write!(err, "Allow {tool}: {summary}?  [y] once  [a] always  [n] deny: ");
                    let _ = err.flush();
                    let answer = tokio::task::spawn_blocking(|| {
                        let mut s = String::new();
                        let _ = std::io::stdin().read_line(&mut s);
                        s
                    })
                    .await
                    .unwrap_or_default();
                    match answer.trim().to_lowercase().as_str() {
                        "y" | "yes" => PermDecision::Once,
                        "a" | "always" => PermDecision::Always,
                        _ => PermDecision::Deny,
                    }
                } else {
                    let _ = writeln!(err, "{dim}denied {tool}: {summary} (non-interactive){reset}");
                    PermDecision::Deny
                };
                engine.permission_reply(&req_id, decision);
            }
            AgentEvent::Finished { .. } => break,
            _ => {}
        }
    }
    if last == Stream::Text {
        let _ = writeln!(out);
    }
    let _ = out.flush();
    Ok(code)
}

//! `xode` — terminal frontend for the Xode coding agent.
mod app;
mod headless;
mod render;
mod theme;
mod tui;
mod ui;
mod view;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use xode_engine::xode_core::config::Config;
use xode_engine::xode_core::store::SessionInfo;
use xode_engine::xode_core::Mode;
use xode_engine::Engine;

#[derive(Parser, Debug)]
#[command(name = "xode", version, about = "Coding agent for local models")]
struct Args {
    /// Project directory (default: current directory).
    path: Option<PathBuf>,
    /// Resume the latest session of this project.
    #[arg(short = 'c', long = "continue")]
    cont: bool,
    /// Resume a specific session.
    #[arg(long, value_name = "ID")]
    session: Option<String>,
    /// Headless: send this prompt, stream the answer to stdout and exit.
    #[arg(short, long, value_name = "TEXT")]
    prompt: Option<String>,
    /// Headless: hide thinking and notices.
    #[arg(short, long)]
    quiet: bool,
    /// Model to use: `<gateway>:<model>` or `<model>`.
    #[arg(short, long)]
    model: Option<String>,
    /// Start in plan mode.
    #[arg(long)]
    plan: bool,
}

/// Resolve `--model` into (gateway id, model id). Model ids may themselves contain ':' (ollama tags).
fn resolve_model(cfg: &Config, spec: &str) -> Result<(String, String)> {
    if let Some((gw, model)) = spec.split_once(':') {
        if let Some(g) = cfg.gateways.iter().find(|g| g.id == gw || g.name.eq_ignore_ascii_case(gw)) {
            return Ok((g.id.clone(), model.to_string()));
        }
    }
    if let Some(g) = cfg.gateways.iter().find(|g| g.enabled && g.models.iter().any(|m| m.id == spec)) {
        return Ok((g.id.clone(), spec.to_string()));
    }
    if !cfg.selected.gateway.is_empty() {
        return Ok((cfg.selected.gateway.clone(), spec.to_string()));
    }
    match cfg.gateways.iter().find(|g| g.enabled) {
        Some(g) => Ok((g.id.clone(), spec.to_string())),
        None => Err(anyhow!("no gateway configured for model '{spec}'")),
    }
}

fn pick_session(engine: &Engine, project_id: &str, args: &Args) -> Result<SessionInfo> {
    if let Some(id) = &args.session {
        return engine.session(id).with_context(|| format!("session {id} not found"));
    }
    if args.cont {
        let latest = engine.sessions(Some(project_id))?.into_iter().max_by_key(|s| s.updated_at);
        if let Some(s) = latest {
            return Ok(s);
        }
    }
    engine.new_session(project_id)
}

async fn run(args: Args) -> Result<i32> {
    let root = match &args.path {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    let root = std::fs::canonicalize(&root).with_context(|| format!("no such directory: {}", root.display()))?;
    let mut root_s = root.to_string_lossy().to_string();
    // Strip the Windows verbatim prefix produced by canonicalize.
    if let Some(s) = root_s.strip_prefix(r"\\?\") {
        root_s = s.to_string();
    }

    let engine = Engine::new()?;
    let project = engine.add_project(&root_s)?;
    let session = pick_session(&engine, &project.id, &args)?;

    if let Some(spec) = &args.model {
        let (gw, model) = resolve_model(&engine.config(), spec)?;
        engine.set_model(&session.id, &gw, &model)?;
    }
    if args.plan {
        engine.set_mode(&session.id, Mode::Plan)?;
    }

    if let Some(prompt) = args.prompt {
        return headless::run(engine, &session.id, prompt, args.quiet).await;
    }
    tui::run(Arc::clone(&engine), project, session).await?;
    Ok(0)
}

fn main() {
    let args = Args::parse();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let code = match rt.block_on(run(args)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("xode: {e:#}");
            1
        }
    };
    rt.shutdown_timeout(std::time::Duration::from_millis(200));
    std::process::exit(code);
}

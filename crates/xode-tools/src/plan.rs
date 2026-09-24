//! `plan`: the only write allowed in plan mode. Keeps the session's plan in
//! `.xode/plans/<session>.md`; the engine offers to build it when the run ends.
use crate::util::spec;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use xode_core::tool::{arg_str, Tool, ToolCtx, ToolOutput};
use xode_core::types::ToolSpec;

pub struct PlanTool;

/// Where a session's plan lives.
pub fn plan_path(root: &Path, session_id: &str) -> PathBuf {
    let short: String = session_id.chars().take(8).collect();
    root.join(".xode").join("plans").join(format!("{short}.md"))
}

#[async_trait]
impl Tool for PlanTool {
    fn name(&self) -> &str {
        "plan"
    }
    fn spec(&self) -> ToolSpec {
        spec(
            "plan",
            "Write the implementation plan (markdown). `text` replaces the whole plan; `old`+`new` edits part of it.",
            &[("text", "string", ""), ("old", "string", ""), ("new", "string", "")],
            &[],
        )
    }
    fn read_only(&self, _: &Value) -> bool {
        true // only touches the plan file
    }
    fn summary(&self, a: &Value) -> String {
        if arg_str(a, "old").is_some() { "edit plan".into() } else { "write plan".into() }
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let path = plan_path(&ctx.project_root, &ctx.session_id);
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        let next = match (arg_str(&args, "text"), arg_str(&args, "old"), arg_str(&args, "new")) {
            (Some(t), _, _) => t.to_string(),
            (None, Some(old), new) => {
                if old.is_empty() || !current.contains(old) {
                    return ToolOutput::err("`old` not found in the plan");
                }
                current.replacen(old, new.unwrap_or(""), 1)
            }
            _ => return ToolOutput::err("pass `text`, or `old` and `new`"),
        };
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return ToolOutput::err(e.to_string());
            }
        }
        match std::fs::write(&path, next.trim_end().to_string() + "\n") {
            Ok(()) => ToolOutput::ok(format!("plan saved: {} ({} lines)", ctx.display(&path), next.trim_end().lines().count())),
            Err(e) => ToolOutput::err(e.to_string()),
        }
    }
}

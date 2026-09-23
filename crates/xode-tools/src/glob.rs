use crate::util::{self, spec};
use async_trait::async_trait;
use serde_json::Value;
use std::time::SystemTime;
use xode_core::tool::{arg_str, Tool, ToolCtx, ToolOutput};
use xode_core::types::ToolSpec;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn spec(&self) -> ToolSpec {
        spec(
            "glob",
            "Find files by glob (e.g. **/*.rs), newest first.",
            &[("pattern", "string", ""), ("path", "string", "base dir")],
            &["pattern"],
        )
    }
    fn read_only(&self, _: &Value) -> bool {
        true
    }
    fn summary(&self, a: &Value) -> String {
        let p = arg_str(a, "pattern").unwrap_or("");
        match arg_str(a, "path") {
            Some(d) => format!("{p} in {d}"),
            None => p.to_string(),
        }
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let Some(pat) = arg_str(&args, "pattern") else { return ToolOutput::err("missing pattern") };
        let base = match arg_str(&args, "path") {
            Some(p) if !p.trim().is_empty() => util::resolve_existing(ctx, p),
            _ => ctx.cwd(),
        };
        let max = ctx.config.tools.glob_max_results.max(1);
        let pat = pat.trim().trim_start_matches("./").to_string();
        let ctx2 = ctx.clone();
        tokio::task::spawn_blocking(move || glob(&ctx2, &base, &pat, max))
            .await
            .unwrap_or_else(|e| ToolOutput::err(e.to_string()))
    }
}

fn glob(ctx: &ToolCtx, base: &std::path::Path, pat: &str, max: usize) -> ToolOutput {
    if !base.is_dir() {
        return ToolOutput::err(format!("not a directory: {}", ctx.display(base)));
    }
    let m = match util::build_glob(pat) {
        Ok(m) => m,
        Err(e) => return ToolOutput::err(e),
    };
    let mut found: Vec<(SystemTime, String)> = vec![];
    for e in util::walker(base).build().flatten() {
        if ctx.cancel.is_cancelled() {
            break;
        }
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = e.path().strip_prefix(base) else { continue };
        if !m.is_match(rel) {
            continue;
        }
        let t = e.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
        found.push((t, ctx.display(e.path())));
    }
    if found.is_empty() {
        return ToolOutput::ok("no matches");
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let n = found.len();
    let mut s = found.into_iter().take(max).map(|x| x.1).collect::<Vec<_>>().join("\n");
    if n > max {
        s.push_str(&format!("\n[+{} more; narrow the pattern]", n - max));
    }
    ToolOutput::ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test::ctx;

    #[tokio::test]
    async fn finds_and_skips() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for f in ["src/a.rs", "src/deep/b.rs", "target/x.rs", "node_modules/y.rs", "c.txt", "ign/z.rs"] {
            std::fs::create_dir_all(r.join(f).parent().unwrap()).unwrap();
            std::fs::write(r.join(f), "x").unwrap();
        }
        std::fs::write(r.join(".gitignore"), "ign/\n").unwrap();
        let c = ctx(r);
        let out = GlobTool.run(serde_json::json!({"pattern": "**/*.rs"}), &c).await.content;
        let mut v: Vec<&str> = out.lines().collect();
        v.sort();
        assert_eq!(v, vec!["src/a.rs", "src/deep/b.rs"]);
        let out = GlobTool.run(serde_json::json!({"pattern": "*.rs", "path": "src/deep"}), &c).await.content;
        assert_eq!(out, "src/deep/b.rs");
    }
}

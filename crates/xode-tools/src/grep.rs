use crate::util::{self, spec};
use async_trait::async_trait;
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use serde_json::Value;
use std::path::Path;
use xode_core::tool::{arg_bool, arg_str, arg_u64, Tool, ToolCtx, ToolOutput};
use xode_core::types::ToolSpec;

const LINE_MAX: usize = 200;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn spec(&self) -> ToolSpec {
        spec(
            "grep",
            "Regex search in files (ripgrep).",
            &[
                ("pattern", "string", ""),
                ("path", "string", "file or dir"),
                ("glob", "string", "file filter, e.g. *.rs"),
                ("i", "boolean", "ignore case"),
                ("context", "integer", "context lines"),
            ],
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
        let q = Query {
            pattern: pat.to_string(),
            glob: arg_str(&args, "glob").filter(|g| !g.trim().is_empty()).map(String::from),
            icase: arg_bool(&args, "i").unwrap_or(false),
            context: arg_u64(&args, "context").unwrap_or(0).min(10) as usize,
            max: ctx.config.tools.grep_max_results.max(1),
        };
        let ctx2 = ctx.clone();
        tokio::task::spawn_blocking(move || grep(&ctx2, &base, &q)).await.unwrap_or_else(|e| ToolOutput::err(e.to_string()))
    }
}

struct Query {
    pattern: String,
    glob: Option<String>,
    icase: bool,
    context: usize,
    max: usize,
}

fn grep(ctx: &ToolCtx, base: &Path, q: &Query) -> ToolOutput {
    if !base.exists() {
        return ToolOutput::err(format!("not found: {}", ctx.display(base)));
    }
    let mut note = "";
    let matcher = match RegexMatcherBuilder::new().case_insensitive(q.icase).build(&q.pattern) {
        Ok(m) => m,
        Err(_) => {
            note = "[invalid regex; searched literally]\n";
            match RegexMatcherBuilder::new().case_insensitive(q.icase).build(&regex::escape(&q.pattern)) {
                Ok(m) => m,
                Err(e) => return ToolOutput::err(e.to_string()),
            }
        }
    };
    let filter = match q.glob.as_deref().map(util::build_glob).transpose() {
        Ok(f) => f,
        Err(e) => return ToolOutput::err(e),
    };
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .before_context(q.context)
        .after_context(q.context)
        .binary_detection(BinaryDetection::quit(0))
        .build();
    let mut sink = Collect { matcher: &matcher, rows: vec![], matches: 0, shown: 0, max: q.max, skip: false };
    let mut out = String::new();
    let mut files = 0usize;
    for e in util::walker(base).build().flatten() {
        if ctx.cancel.is_cancelled() || sink.matches > q.max * 50 {
            break;
        }
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if e.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
            continue;
        }
        if let Some(f) = &filter {
            let rel = e.path().strip_prefix(base).unwrap_or(e.path());
            if !(f.is_match(rel) || e.path().file_name().is_some_and(|n| f.is_match(n))) {
                continue;
            }
        }
        sink.rows.clear();
        let before = sink.matches;
        if searcher.search_path(&matcher, e.path(), &mut sink).is_err() {
            continue;
        }
        if sink.matches > before {
            files += 1;
        }
        if !sink.rows.is_empty() {
            out.push_str(&ctx.display(e.path()));
            out.push('\n');
            for r in &sink.rows {
                out.push_str(r);
                out.push('\n');
            }
        }
    }
    if sink.matches == 0 {
        return ToolOutput::ok(format!("{note}no matches"));
    }
    if sink.matches > sink.shown {
        out.push_str(&format!(
            "[{} matches in {files} files, {} not shown; narrow with path/glob]",
            sink.matches,
            sink.matches - sink.shown
        ));
    }
    ToolOutput::ok(format!("{note}{}", out.trim_end()))
}

struct Collect<'a> {
    matcher: &'a RegexMatcher,
    rows: Vec<String>,
    matches: usize,
    shown: usize,
    max: usize,
    /// Set once the cap is hit, so no more context rows are shown.
    skip: bool,
}

impl Collect<'_> {
    fn line(&self, bytes: &[u8], is_match: bool) -> String {
        let s = String::from_utf8_lossy(bytes);
        let s = s.trim_end_matches(['\n', '\r']);
        if s.chars().count() <= LINE_MAX {
            return s.to_string();
        }
        // Center long lines on the match.
        let at = if is_match { self.matcher.find(s.as_bytes()).ok().flatten().map(|m| m.start()).unwrap_or(0) } else { 0 };
        let mut start = at.saturating_sub(LINE_MAX / 3);
        while !s.is_char_boundary(start) {
            start -= 1;
        }
        let body: String = s[start..].chars().take(LINE_MAX).collect();
        let pre = if start > 0 { "…" } else { "" };
        let post = if start + body.len() < s.len() { "…" } else { "" };
        format!("{pre}{body}{post}")
    }
}

impl Sink for Collect<'_> {
    type Error = std::io::Error;
    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, std::io::Error> {
        self.matches += 1;
        if self.shown < self.max {
            self.shown += 1;
            let l = self.line(m.bytes(), true);
            self.rows.push(format!("{}:{l}", m.line_number().unwrap_or(0)));
        } else {
            self.skip = true;
        }
        Ok(true)
    }
    fn context(&mut self, _: &Searcher, c: &SinkContext<'_>) -> Result<bool, std::io::Error> {
        if !self.skip && (self.shown < self.max || matches!(c.kind(), grep_searcher::SinkContextKind::After)) {
            let l = self.line(c.bytes(), false);
            self.rows.push(format!("{}-{l}", c.line_number().unwrap_or(0)));
        }
        Ok(true)
    }
    fn context_break(&mut self, _: &Searcher) -> Result<bool, std::io::Error> {
        if !self.skip && !self.rows.is_empty() && self.shown < self.max {
            self.rows.push("--".into());
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test::ctx;

    #[tokio::test]
    async fn grep_basic() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::write(r.join("src/a.rs"), "fn foo() {}\nlet x = 1;\nfn bar() {}\n").unwrap();
        std::fs::write(r.join("b.txt"), "FOO here\n").unwrap();
        std::fs::write(r.join("long.txt"), format!("{}needle{}\n", "a".repeat(500), "b".repeat(500))).unwrap();
        let c = ctx(r);
        let out = GrepTool.run(serde_json::json!({"pattern": "fn \\w+", "glob": "*.rs"}), &c).await.content;
        assert_eq!(out, "src/a.rs\n1:fn foo() {}\n3:fn bar() {}");
        let out = GrepTool.run(serde_json::json!({"pattern": "foo", "i": true, "path": "b.txt"}), &c).await.content;
        assert_eq!(out, "b.txt\n1:FOO here");
        let out = GrepTool.run(serde_json::json!({"pattern": "x =", "context": 1}), &c).await.content;
        assert_eq!(out, "src/a.rs\n1-fn foo() {}\n2:let x = 1;\n3-fn bar() {}");
        let out = GrepTool.run(serde_json::json!({"pattern": "needle"}), &c).await.content;
        assert!(out.contains("needle") && out.len() < 260, "{out}");
        let out = GrepTool.run(serde_json::json!({"pattern": "fn ("}), &c).await.content;
        assert!(out.starts_with("[invalid regex"), "{out}");
    }
}

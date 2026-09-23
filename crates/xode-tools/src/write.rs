use crate::util::{self, spec};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use xode_core::tool::{arg_str, Tool, ToolCtx, ToolOutput};
use xode_core::types::ToolSpec;

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    fn spec(&self) -> ToolSpec {
        spec("write", "Create or overwrite a file.", &[("path", "string", ""), ("content", "string", "")], &["path", "content"])
    }
    fn summary(&self, a: &Value) -> String {
        arg_str(a, "path").unwrap_or("").to_string()
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let Some(p) = arg_str(&args, "path") else { return ToolOutput::err("missing path") };
        let Some(content) = arg_str(&args, "content") else { return ToolOutput::err("missing content") };
        let path = ctx.resolve(p);
        match write_file(ctx, &path, content) {
            Ok((existed, n)) => {
                let verb = if existed { "wrote" } else { "created" };
                ToolOutput::ok(format!("{verb} {} ({n} lines)", ctx.display(&path)))
            }
            Err(e) => ToolOutput::err(format!("{}: {e}", ctx.display(&path))),
        }
    }
}

/// Encode `content` in the style of the existing file at `path` (line endings, BOM).
pub(crate) fn match_style(existing: Option<&[u8]>, content: &str) -> Vec<u8> {
    let Some(old) = existing else { return content.as_bytes().to_vec() };
    let bom = old.starts_with(&[0xEF, 0xBB, 0xBF]);
    let old_text = util::decode(old);
    let body = if util::is_crlf(&old_text) { util::to_crlf(content) } else if old_text.contains('\n') { util::to_lf(content) } else { content.to_string() };
    let mut out = Vec::with_capacity(body.len() + 3);
    if bom && !body.starts_with('\u{feff}') {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(body.as_bytes());
    out
}

/// Returns (existed, line count).
pub(crate) fn write_file(ctx: &ToolCtx, path: &Path, content: &str) -> std::io::Result<(bool, usize)> {
    if path.is_dir() {
        return Err(std::io::Error::other("is a directory"));
    }
    let existing = std::fs::read(path).ok();
    let bytes = match_style(existing.as_deref(), content);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    ctx.snapshot_for_undo(path);
    std::fs::write(path, &bytes)?;
    let n = util::line_count(content);
    // The model wrote the whole file, so it is fully "in context".
    ctx.touch(path, "write", &bytes, Some(format!("1-{n}")));
    if let Some(o) = &ctx.outliner {
        o.file_changed(path);
    }
    Ok((existing.is_some(), n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test::ctx;

    #[tokio::test]
    async fn preserves_crlf_and_creates_dirs() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        std::fs::write(d.path().join("w.txt"), "a\r\nb\r\n").unwrap();
        let r = WriteTool.run(serde_json::json!({"path": "w.txt", "content": "x\ny\nz\n"}), &c).await;
        assert_eq!(r.content, "wrote w.txt (3 lines)");
        assert_eq!(std::fs::read_to_string(d.path().join("w.txt")).unwrap(), "x\r\ny\r\nz\r\n");

        let r = WriteTool.run(serde_json::json!({"path": "n/e/w.txt", "content": "q\n"}), &c).await;
        assert_eq!(r.content, "created n/e/w.txt (1 lines)");
        assert_eq!(std::fs::read_to_string(d.path().join("n/e/w.txt")).unwrap(), "q\n");

        let st = c.state.lock();
        assert_eq!(st.undo.len(), 2);
        assert!(st.undo[1].1.is_none());
        assert_eq!(st.touched["w.txt"].ranges, vec!["1-3".to_string()]);
    }

    #[test]
    fn bom_and_lf() {
        let old = b"\xEF\xBB\xBFa\nb\n";
        assert_eq!(match_style(Some(old), "x\r\ny\r\n"), b"\xEF\xBB\xBFx\ny\n");
    }
}

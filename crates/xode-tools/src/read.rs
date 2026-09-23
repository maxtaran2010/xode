use crate::util::{self, spec};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use xode_core::tool::{arg_bool, arg_str, arg_u64, hash_bytes, FileTouch, Tool, ToolCtx, ToolOutput};
use xode_core::types::ToolSpec;

/// Files up to this many lines are simply re-sent after compaction (cheaper than a round trip).
const SMALL_FILE_LINES: usize = 120;
const MAX_LINE_CHARS: usize = 2000;
const MAX_DIR_ENTRIES: usize = 300;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    fn spec(&self) -> ToolSpec {
        spec(
            "read",
            "Read a file as numbered lines, or list a directory.",
            &[
                ("path", "string", ""),
                ("offset", "integer", "1-based start line"),
                ("limit", "integer", "max lines"),
                ("force", "boolean", "re-read even if unchanged"),
            ],
            &["path"],
        )
    }
    fn read_only(&self, _: &Value) -> bool {
        true
    }
    fn summary(&self, a: &Value) -> String {
        let p = arg_str(a, "path").unwrap_or("");
        match (arg_u64(a, "offset"), arg_u64(a, "limit")) {
            (None, None) => p.to_string(),
            (o, l) => format!("{p}:{}+{}", o.unwrap_or(1), l.map(|x| x.to_string()).unwrap_or_default()),
        }
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let Some(p) = arg_str(&args, "path") else { return ToolOutput::err("missing path") };
        let path = util::resolve_existing(ctx, p);
        let req = ReadReq {
            offset: arg_u64(&args, "offset").map(|x| x as usize),
            limit: arg_u64(&args, "limit").map(|x| x as usize),
            force: arg_bool(&args, "force").unwrap_or(false),
        };
        read_path(ctx, &path, req)
    }
}

#[derive(Default, Clone, Copy)]
pub(crate) struct ReadReq {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub force: bool,
}

pub(crate) fn read_path(ctx: &ToolCtx, path: &Path, req: ReadReq) -> ToolOutput {
    let shown = ctx.display(path);
    if path.is_dir() {
        return list_dir(path);
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ToolOutput::err(format!("not found: {shown}")),
        Err(e) => return ToolOutput::err(format!("{shown}: {e}")),
    };
    if util::is_binary(&bytes) {
        return ToolOutput::ok(format!("binary file {shown} ({} bytes)", bytes.len()));
    }
    let text = util::decode(&bytes);
    let lines: Vec<&str> = text.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let total = lines.len();
    if total == 0 {
        ctx.touch(path, "read", &bytes, Some("1-0".into()));
        return ToolOutput::ok(format!("(empty file {shown})"));
    }

    let ts = &ctx.config.token_saving;
    let explicit = req.offset.is_some() || req.limit.is_some();
    let start = req.offset.unwrap_or(1).max(1);
    if start > total {
        return ToolOutput::err(format!("offset {start} > {total} lines"));
    }
    let limit = req.limit.unwrap_or(ts.read_max_lines.max(1)).max(1);
    let end = (start + limit - 1).min(total);

    // Dedup check.
    let hash = hash_bytes(&bytes);
    let (prev, segment) = {
        let st = ctx.state.lock();
        (st.touched.get(&shown).cloned(), st.segment)
    };
    match dedup_decision(prev.as_ref(), &hash, start, end, segment, req.force, explicit, ts.read_dedup) {
        Dedup::Fresh => {}
        Dedup::Unchanged { step } => {
            let mut s = format!("{shown} unchanged since step {step} (already in context or summarized).");
            if let Some(o) = outline(ctx, path) {
                s.push_str(" outline:\n");
                s.push_str(&o);
                s.push('\n');
            } else {
                s.push(' ');
            }
            s.push_str("pass force=true or a new range to re-read");
            return ToolOutput::ok(s);
        }
        Dedup::Compacted => {
            if total > SMALL_FILE_LINES {
                let mut s = format!("{shown} ({total} lines) was read before compaction; its content is no longer in context.");
                if let Some(o) = outline(ctx, path) {
                    s.push_str(" outline:\n");
                    s.push_str(&o);
                    s.push('\n');
                } else {
                    s.push(' ');
                }
                s.push_str("read only what you need with offset/limit (or force=true)");
                return ToolOutput::ok(s);
            }
        }
    }

    // Render with a token budget so a file of very long lines cannot blow the context.
    let budget = (ts.max_tool_output_tokens.max(1000) * 2) as usize;
    let mut out = String::new();
    let mut used = 0usize;
    let mut last = start - 1;
    for (i, l) in lines[start - 1..end].iter().enumerate() {
        let n = start + i;
        let line = util::clip(l, MAX_LINE_CHARS);
        let row = format!("{n}│{line}\n");
        used += (row.len() + 2) / 3;
        if used > budget && n > start {
            break;
        }
        out.push_str(&row);
        last = n;
    }
    let range = format!("{start}-{last}");
    if start > 1 || last < total {
        out.push_str(&format!("[lines {range} of {total}"));
        if last < total {
            out.push_str(&format!("; offset={} for more", last + 1));
        }
        out.push_str("]\n");
    }
    touch_read(ctx, path, &shown, &bytes, range);
    while out.ends_with('\n') {
        out.pop();
    }
    ToolOutput::ok(out)
}

fn outline(ctx: &ToolCtx, path: &Path) -> Option<String> {
    ctx.outliner.as_ref()?.outline(path).filter(|o| !o.trim().is_empty())
}

/// `touch` that forgets ranges read in an earlier (compacted) segment.
fn touch_read(ctx: &ToolCtx, path: &Path, key: &str, bytes: &[u8], range: String) {
    {
        let mut st = ctx.state.lock();
        let seg = st.segment;
        if let Some(t) = st.touched.get_mut(key) {
            if t.segment < seg {
                t.ranges.clear();
            }
        }
    }
    ctx.touch(path, "read", bytes, Some(range));
}

#[derive(Debug, PartialEq)]
pub(crate) enum Dedup {
    /// Return content.
    Fresh,
    /// Same content, same range already in the live context.
    Unchanged { step: u32 },
    /// Same content read in an earlier segment (compacted away).
    Compacted,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dedup_decision(
    prev: Option<&FileTouch>,
    hash: &str,
    start: usize,
    end: usize,
    segment: u32,
    force: bool,
    explicit: bool,
    enabled: bool,
) -> Dedup {
    if !enabled || force {
        return Dedup::Fresh;
    }
    let Some(t) = prev else { return Dedup::Fresh };
    if t.hash != hash {
        return Dedup::Fresh;
    }
    if t.segment < segment {
        return if explicit { Dedup::Fresh } else { Dedup::Compacted };
    }
    if covered(&t.ranges, start, end) {
        Dedup::Unchanged { step: t.step }
    } else {
        Dedup::Fresh
    }
}

/// Is `[a, b]` fully inside the union of the `"x-y"` ranges?
pub(crate) fn covered(ranges: &[String], a: usize, b: usize) -> bool {
    let mut rs: Vec<(usize, usize)> = ranges
        .iter()
        .filter_map(|r| {
            let (x, y) = r.split_once('-')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect();
    rs.sort();
    let mut need = a;
    for (x, y) in rs {
        if x > need {
            break;
        }
        if y >= need {
            need = y + 1;
        }
        if need > b {
            return true;
        }
    }
    need > b
}

fn list_dir(path: &Path) -> ToolOutput {
    let rd = match std::fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => return ToolOutput::err(e.to_string()),
    };
    let mut dirs = vec![];
    let mut files = vec![];
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(format!("{name}/"));
        } else {
            files.push(name);
        }
    }
    dirs.sort_by_key(|a| a.to_lowercase());
    files.sort_by_key(|a| a.to_lowercase());
    let all: Vec<String> = dirs.into_iter().chain(files).collect();
    if all.is_empty() {
        return ToolOutput::ok("(empty dir)");
    }
    let n = all.len();
    let mut s = all.into_iter().take(MAX_DIR_ENTRIES).collect::<Vec<_>>().join("\n");
    if n > MAX_DIR_ENTRIES {
        s.push_str(&format!("\n[+{} more]", n - MAX_DIR_ENTRIES));
    }
    ToolOutput::ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test::ctx;
    use std::sync::Arc;
    use xode_core::tool::Outliner;

    fn t(hash: &str, seg: u32, ranges: &[&str]) -> FileTouch {
        FileTouch {
            path: "a".into(),
            how: "read".into(),
            hash: hash.into(),
            step: 3,
            segment: seg,
            lines: 10,
            ranges: ranges.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn coverage() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(covered(&r(&["1-100"]), 1, 100));
        assert!(covered(&r(&["1-100"]), 10, 20));
        assert!(covered(&r(&["1-50", "51-100"]), 40, 90));
        assert!(!covered(&r(&["1-50", "60-100"]), 40, 90));
        assert!(!covered(&r(&[]), 1, 1));
        assert!(!covered(&r(&["1-100"]), 50, 101));
    }

    #[test]
    fn decision() {
        use Dedup::*;
        let tc = t("h", 0, &["1-10"]);
        assert_eq!(dedup_decision(None, "h", 1, 10, 0, false, false, true), Fresh);
        assert_eq!(dedup_decision(Some(&tc), "h", 1, 10, 0, false, false, true), Unchanged { step: 3 });
        assert_eq!(dedup_decision(Some(&tc), "h", 2, 5, 0, false, true, true), Unchanged { step: 3 });
        assert_eq!(dedup_decision(Some(&tc), "x", 1, 10, 0, false, false, true), Fresh);
        assert_eq!(dedup_decision(Some(&tc), "h", 1, 10, 0, true, false, true), Fresh);
        assert_eq!(dedup_decision(Some(&tc), "h", 1, 10, 0, false, false, false), Fresh);
        assert_eq!(dedup_decision(Some(&tc), "h", 1, 20, 0, false, true, true), Fresh);
        // Earlier segment: content compacted away.
        assert_eq!(dedup_decision(Some(&tc), "h", 1, 10, 1, false, false, true), Compacted);
        assert_eq!(dedup_decision(Some(&tc), "h", 1, 10, 1, false, true, true), Fresh);
    }

    struct O;
    impl Outliner for O {
        fn outline(&self, _: &Path) -> Option<String> {
            Some("fn main 1".into())
        }
    }

    #[tokio::test]
    async fn read_dedup_flow() {
        let d = tempfile::tempdir().unwrap();
        let body: String = (1..=200).map(|i| format!("line {i}\r\n")).collect();
        std::fs::write(d.path().join("a.txt"), &body).unwrap();
        let mut c = ctx(d.path());
        c.outliner = Some(Arc::new(O));
        let run = |a: Value| {
            let c = c.clone();
            async move { ReadTool.run(a, &c).await }
        };

        let r = run(serde_json::json!({"path": "a.txt"})).await;
        assert!(r.content.starts_with("1│line 1\n2│line 2\n"), "{}", r.content);
        assert!(!r.content.contains('\r'));
        assert!(!r.content.contains("[lines"));

        let r = run(serde_json::json!({"path": "a.txt"})).await;
        assert!(r.content.contains("unchanged since step 0"), "{}", r.content);
        assert!(r.content.contains("fn main 1"));

        let r = run(serde_json::json!({"path": "a.txt", "offset": 5, "limit": 3})).await;
        assert!(r.content.contains("unchanged"), "{}", r.content);

        let r = run(serde_json::json!({"path": "a.txt", "force": true})).await;
        assert!(r.content.starts_with("1│"));

        // File changes -> content again.
        std::fs::write(d.path().join("a.txt"), format!("{body}more\r\n")).unwrap();
        let r = run(serde_json::json!({"path": "a.txt", "offset": 199})).await;
        assert_eq!(r.content, "199│line 199\n200│line 200\n201│more\n[lines 199-201 of 201]");

        // Compaction: new segment.
        c.state.lock().segment = 1;
        let r = run(serde_json::json!({"path": "a.txt"})).await;
        assert!(r.content.contains("before compaction"), "{}", r.content);
        let r = run(serde_json::json!({"path": "a.txt", "offset": 199})).await;
        assert!(r.content.starts_with("199│"), "explicit range after compaction returns content");
        // Old-segment ranges were dropped on the re-touch, so only 199-201 counts now.
        let st = c.state.lock();
        assert_eq!(st.touched["a.txt"].ranges, vec!["199-201".to_string()]);
    }

    #[tokio::test]
    async fn partial_binary_dir() {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = xode_core::Config::default();
        cfg.token_saving.read_max_lines = 2;
        let c = crate::util::test::ctx_with(d.path(), cfg);
        std::fs::write(d.path().join("x.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(d.path().join("b.bin"), [1u8, 0, 2]).unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let r = ReadTool.run(serde_json::json!({"path": "x.txt"}), &c).await;
        assert_eq!(r.content, "1│a\n2│b\n[lines 1-2 of 3; offset=3 for more]");
        let r = ReadTool.run(serde_json::json!({"path": "b.bin"}), &c).await;
        assert!(r.content.starts_with("binary file"));
        let r = ReadTool.run(serde_json::json!({"path": "."}), &c).await;
        assert_eq!(r.content, "sub/\nb.bin\nx.txt");
        let r = ReadTool.run(serde_json::json!({"path": "nope"}), &c).await;
        assert!(r.is_error);
    }
}

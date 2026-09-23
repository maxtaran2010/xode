//! The `code` tool: structural queries and symbol-level read/edit over the index.

use crate::parse::{self, clip, Sym};
use crate::ProjectIndex;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use xode_core::tool::{arg_str, Tool, ToolCtx, ToolOutput, ToolRef};
use xode_core::types::ToolSpec;

const FIND_MAX: usize = 30;
const REFS_MAX: usize = 40;
const DIR_MAX: usize = 60;

pub fn code_tool(index: Arc<ProjectIndex>) -> ToolRef {
    Arc::new(CodeTool { index })
}

struct CodeTool {
    index: Arc<ProjectIndex>,
}

#[async_trait]
impl Tool for CodeTool {
    fn name(&self) -> &str {
        "code"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "code".into(),
            description: "Code index. map: ranked repo overview. outline{path}: symbols of file/dir. find{symbol}: definitions. refs{symbol}: usages.\nread{symbol,path?}: one symbol's source. edit{symbol,path?,content}: replace that symbol's whole source with content. symbol may be Parent.name; path may be file:line.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["map", "outline", "find", "refs", "read", "edit"]},
                    "path": {"type": "string"},
                    "symbol": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["action"]
            }),
        }
    }

    fn read_only(&self, args: &Value) -> bool {
        arg_str(args, "action") != Some("edit")
    }

    fn summary(&self, args: &Value) -> String {
        let a = arg_str(args, "action").unwrap_or("?");
        let s = [arg_str(args, "symbol"), arg_str(args, "path")].into_iter().flatten().collect::<Vec<_>>().join(" ");
        clip(&format!("code {a} {s}").trim().to_string(), 160)
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let idx = self.index.clone();
        let ctx = ctx.clone();
        tokio::task::spawn_blocking(move || run_sync(&idx, &args, &ctx))
            .await
            .unwrap_or_else(|e| ToolOutput::err(format!("code: {e}")))
    }
}

pub(crate) fn run_sync(idx: &ProjectIndex, args: &Value, ctx: &ToolCtx) -> ToolOutput {
    let action = arg_str(args, "action").unwrap_or("").trim();
    let path = arg_str(args, "path").map(str::trim).filter(|s| !s.is_empty());
    let symbol = arg_str(args, "symbol").map(str::trim).filter(|s| !s.is_empty());
    let r = match action {
        "map" => Ok(do_map(idx, ctx)),
        "outline" => match path.or(symbol) {
            Some(p) => do_outline(idx, ctx, p),
            None => Err("outline needs path".into()),
        },
        "find" => match symbol.or(path) {
            Some(s) => Ok(do_find(idx, s)),
            None => Err("find needs symbol".into()),
        },
        "refs" => match symbol {
            Some(s) => Ok(do_refs(idx, s)),
            None => Err("refs needs symbol".into()),
        },
        "read" => match symbol {
            Some(s) => do_read(idx, ctx, s, path),
            None => Err("read needs symbol (use the read tool for whole files)".into()),
        },
        "edit" => match (symbol, arg_str(args, "content")) {
            (Some(s), Some(c)) => do_edit(idx, ctx, s, path, c),
            _ => Err("edit needs symbol and content".into()),
        },
        _ => Err("action must be map|outline|find|refs|read|edit".into()),
    };
    match r {
        Ok(s) => ToolOutput::ok(s),
        Err(e) => ToolOutput::err(e),
    }
}

fn not_ready_note(idx: &ProjectIndex) -> String {
    if idx.is_ready() {
        String::new()
    } else {
        format!("\n(index building: {} files so far)", idx.stats().files)
    }
}

fn do_map(idx: &ProjectIndex, ctx: &ToolCtx) -> String {
    let focus: Vec<String> = {
        let st = ctx.state.lock();
        let mut t: Vec<_> = st.touched.values().collect();
        t.sort_by(|a, b| b.step.cmp(&a.step));
        t.into_iter().take(10).map(|f| f.path.clone()).collect()
    };
    let m = idx.repo_map(ctx.config.token_saving.repo_map_tokens.max(200), &focus);
    if m.is_empty() {
        return format!("index empty{}", not_ready_note(idx));
    }
    format!("{m}{}", not_ready_note(idx))
}

/// Split `path[:line]`.
fn split_line(p: &str) -> (&str, Option<u32>) {
    if let Some((a, b)) = p.rsplit_once(':') {
        let b = b.trim_start_matches(['L', 'l']);
        let b = b.split('-').next().unwrap_or(b);
        if let Ok(n) = b.parse::<u32>() {
            if !a.is_empty() {
                return (a, Some(n));
            }
        }
    }
    (p, None)
}

fn resolve(idx: &ProjectIndex, ctx: &ToolCtx, p: &str) -> PathBuf {
    let r = ctx.resolve(p);
    if r.exists() {
        return r;
    }
    let alt = idx.abs(p.trim_start_matches("./"));
    if alt.exists() {
        alt
    } else {
        r
    }
}

fn show(idx: &ProjectIndex, ctx: &ToolCtx, p: &std::path::Path) -> String {
    idx.rel(p).unwrap_or_else(|| ctx.display(p))
}

fn do_outline(idx: &ProjectIndex, ctx: &ToolCtx, p: &str) -> Result<String, String> {
    let (p, _) = split_line(p);
    let abs = resolve(idx, ctx, p);
    if abs.is_dir() {
        let rel = idx.rel(&abs).ok_or("directory outside project")?;
        let files = idx.files_under(&rel);
        if files.is_empty() {
            return Ok(format!("no indexed files under {}{}", if rel.is_empty() { "." } else { &rel }, not_ready_note(idx)));
        }
        let mut out = String::new();
        for f in files.iter().take(DIR_MAX) {
            let syms = idx.file_symbols(f);
            let top: Vec<&str> = syms.iter().filter(|s| s.depth == 0).map(|s| s.name.as_str()).collect();
            let shown: Vec<&str> = top.iter().take(8).copied().collect();
            let mut line = format!("{f}: {}", shown.join(", "));
            if top.len() > 8 {
                line.push_str(&format!(" …+{}", top.len() - 8));
            }
            out.push_str(&clip(line.trim_end_matches([':', ' ']), 200));
            out.push('\n');
        }
        if files.len() > DIR_MAX {
            out.push_str(&format!("…+{} more files\n", files.len() - DIR_MAX));
        }
        return Ok(format!("{}{}", out.trim_end(), not_ready_note(idx)));
    }
    if !abs.is_file() {
        return Err(format!("not found: {p}"));
    }
    let lines = std::fs::read(&abs).map(|b| b.iter().filter(|c| **c == b'\n').count() + 1).unwrap_or(0);
    match idx.outline_file(&abs) {
        Some(o) => Ok(format!("{} ({lines} lines)\n{o}", show(idx, ctx, &abs))),
        None => Ok(format!("{} ({lines} lines): no symbols", show(idx, ctx, &abs))),
    }
}

fn fmt_row(file: &str, kind: &str, parent: Option<&str>, a: u32, b: u32, sig: &str) -> String {
    let par = parent.map(|p| format!(" [in {p}]")).unwrap_or_default();
    if a == b {
        format!("{file}:L{a} {kind} {sig}{par}")
    } else {
        format!("{file}:L{a}-{b} {kind} {sig}{par}")
    }
}

fn do_find(idx: &ProjectIndex, q: &str) -> String {
    let (parent, name) = split_symbol(idx, q);
    let mut rows = idx.find(name, FIND_MAX + 1);
    if let Some(p) = parent {
        let f: Vec<_> = rows.iter().filter(|r| r.parent.as_deref().map(|x| x.eq_ignore_ascii_case(p)).unwrap_or(false)).cloned().collect();
        if !f.is_empty() {
            rows = f;
        }
    }
    if rows.is_empty() {
        return format!("no definitions matching '{q}'{}", not_ready_note(idx));
    }
    let more = rows.len() > FIND_MAX;
    let mut out: Vec<String> = rows
        .iter()
        .take(FIND_MAX)
        .map(|r| fmt_row(&r.file, &r.kind, r.parent.as_deref(), r.line_start, r.line_end, &r.signature))
        .collect();
    if more {
        out.push("…more; refine the query".into());
    }
    out.join("\n")
}

fn do_refs(idx: &ProjectIndex, q: &str) -> String {
    let (_, name) = split_symbol(idx, q);
    let refs = idx.refs(name);
    if refs.is_empty() {
        return format!("no references to '{name}'{}", not_ready_note(idx));
    }
    let mut by_file: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for (f, l) in &refs {
        by_file.entry(f.as_str()).or_default().push(*l);
    }
    let mut out = format!("{} refs in {} files\n", refs.len(), by_file.len());
    let mut shown = 0;
    for (f, lines) in &by_file {
        if shown >= REFS_MAX {
            break;
        }
        let text = std::fs::read(idx.abs(f)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let src: Vec<&str> = text.split('\n').collect();
        out.push_str(f);
        out.push('\n');
        for l in lines {
            if shown >= REFS_MAX {
                break;
            }
            let t = src.get(*l as usize - 1).map(|s| s.trim()).unwrap_or("");
            out.push_str(&format!("  {l}: {}\n", clip(t, 100)));
            shown += 1;
        }
    }
    if refs.len() > shown {
        out.push_str(&format!("…+{} more\n", refs.len() - shown));
    }
    out.trim_end().to_string()
}

/// `Parent.name` / `Parent::name` → (Some(parent), name), unless the full string is itself a symbol.
fn split_symbol<'a>(idx: &ProjectIndex, q: &'a str) -> (Option<&'a str>, &'a str) {
    let q = q.trim().trim_end_matches("()");
    let split = q.rsplit_once("::").or_else(|| q.rsplit_once('.'));
    match split {
        Some((p, n)) if !p.is_empty() && !n.is_empty() && idx.defs(q, None).is_empty() => {
            (Some(p.rsplit(['.', ':']).next().unwrap_or(p)), n)
        }
        _ => (None, q),
    }
}

struct Found {
    rel: String,
    abs: PathBuf,
    src: String,
    sym: Sym,
}

fn matches_in(syms: &[Sym], name: &str, parent: Option<&str>) -> Vec<Sym> {
    let pm = |s: &Sym| match parent {
        Some(p) => s.parent.as_deref().map(|x| x.eq_ignore_ascii_case(p)).unwrap_or(false),
        None => true,
    };
    let exact: Vec<Sym> = syms.iter().filter(|s| s.name == name && pm(s)).cloned().collect();
    if !exact.is_empty() {
        return exact;
    }
    syms.iter().filter(|s| s.name.eq_ignore_ascii_case(name) && pm(s)).cloned().collect()
}

/// Locate symbol(s) by parsing the current file contents (never trusts stale line numbers).
fn locate(idx: &ProjectIndex, ctx: &ToolCtx, symbol: &str, path: Option<&str>) -> Result<Vec<Found>, String> {
    let (parent, name) = split_symbol(idx, symbol);
    let (files, line_hint): (Vec<PathBuf>, Option<u32>) = match path {
        Some(p) => {
            let (p, line) = split_line(p);
            let abs = resolve(idx, ctx, p);
            if !abs.is_file() {
                return Err(format!("not found: {p}"));
            }
            (vec![abs], line)
        }
        None => {
            let mut rows = idx.defs(name, parent);
            if rows.is_empty() && parent.is_some() {
                rows = idx.defs(name, None);
            }
            let mut fs: Vec<String> = vec![];
            for r in rows {
                if !fs.contains(&r.file) {
                    fs.push(r.file);
                }
            }
            (fs.iter().take(10).map(|f| idx.abs(f)).collect(), None)
        }
    };
    let mut found = vec![];
    for abs in files {
        let Some((_, src, p)) = ProjectIndex::parse_file(&abs) else { continue };
        let rel = show(idx, ctx, &abs);
        for sym in matches_in(&p.syms, name, parent) {
            found.push(Found { rel: rel.clone(), abs: abs.clone(), src: src.clone(), sym });
        }
    }
    if let Some(l) = line_hint {
        if found.len() > 1 {
            let inside: Vec<usize> =
                (0..found.len()).filter(|i| found[*i].sym.line_start <= l && l <= found[*i].sym.line_end).collect();
            let best = if let Some(i) = inside.iter().max_by_key(|i| found[**i].sym.line_start) {
                *i
            } else {
                (0..found.len()).min_by_key(|i| (found[*i].sym.line_start as i64 - l as i64).abs()).unwrap_or(0)
            };
            found = vec![found.swap_remove(best)];
        }
    }
    if found.is_empty() {
        let sugg = idx.find(name, 5);
        let mut msg = format!("symbol '{symbol}' not found");
        if let Some(p) = path {
            msg.push_str(&format!(" in {p}"));
        }
        if !sugg.is_empty() {
            msg.push_str("; similar:\n");
            msg.push_str(
                &sugg
                    .iter()
                    .map(|r| fmt_row(&r.file, &r.kind, r.parent.as_deref(), r.line_start, r.line_end, &r.signature))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        msg.push_str(&not_ready_note(idx));
        return Err(msg);
    }
    Ok(found)
}

fn ambiguous(found: &[Found]) -> String {
    let mut s = format!("{} matches; pass path=file:line or Parent.name:\n", found.len());
    for f in found.iter().take(15) {
        s.push_str(&fmt_row(&f.rel, f.sym.kind, f.sym.parent.as_deref(), f.sym.line_start, f.sym.line_end, &f.sym.signature));
        s.push('\n');
    }
    s.trim_end().to_string()
}

fn do_read(idx: &ProjectIndex, ctx: &ToolCtx, symbol: &str, path: Option<&str>) -> Result<String, String> {
    let found = locate(idx, ctx, symbol, path)?;
    let total: u32 = found.iter().map(|f| f.sym.line_end - f.sym.line_start + 1).sum();
    let max = ctx.config.token_saving.read_max_lines.max(50);
    if found.len() > 3 || (found.len() > 1 && total as usize > max) {
        return Err(ambiguous(&found));
    }
    let mut out = String::new();
    for f in &found {
        let lines: Vec<&str> = f.src.split('\n').collect();
        let (a, b) = (f.sym.line_start, f.sym.line_end);
        out.push_str(&fmt_row(&f.rel, f.sym.kind, f.sym.parent.as_deref(), a, b, &f.sym.signature));
        out.push('\n');
        let end = (b as usize).min(lines.len());
        let shown_end = end.min(a as usize - 1 + max);
        for i in (a as usize - 1)..shown_end {
            out.push_str(&format!("{}│{}\n", i + 1, lines[i].trim_end_matches('\r')));
        }
        if shown_end < end {
            out.push_str(&format!("…truncated at L{shown_end}; read L{}-{end} with the read tool\n", shown_end + 1));
        }
        ctx.touch(&f.abs, "read", f.src.as_bytes(), Some(format!("{a}-{shown_end}")));
    }
    Ok(out.trim_end().to_string())
}

fn do_edit(idx: &ProjectIndex, ctx: &ToolCtx, symbol: &str, path: Option<&str>, content: &str) -> Result<String, String> {
    let mut found = locate(idx, ctx, symbol, path)?;
    if found.len() > 1 {
        return Err(ambiguous(&found));
    }
    let f = found.remove(0);
    let bytes = std::fs::read(&f.abs).map_err(|e| format!("read {}: {e}", f.rel))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8; use the edit tool", f.rel))?;
    if text != f.src {
        return Err("file changed during edit; retry".into());
    }
    let crlf = text.contains("\r\n");
    let eol = if crlf { "\r\n" } else { "\n" };
    let trailing = text.ends_with('\n');
    let body = text.strip_suffix('\n').unwrap_or(&text);
    let old: Vec<&str> = body.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let (a, b) = (f.sym.line_start as usize, (f.sym.line_end as usize).min(old.len()));
    let c = content.strip_suffix('\n').unwrap_or(content);
    let c = c.strip_suffix('\r').unwrap_or(c);
    let new_mid: Vec<&str> = if content.is_empty() { vec![] } else { c.split('\n').map(|l| l.trim_end_matches('\r')).collect() };
    let old_mid = &old[a - 1..b];
    let mut lines: Vec<&str> = Vec::with_capacity(old.len() + new_mid.len());
    lines.extend_from_slice(&old[..a - 1]);
    lines.extend_from_slice(&new_mid);
    lines.extend_from_slice(&old[b..]);
    let mut new_text = lines.join(eol);
    if trailing {
        new_text.push_str(eol);
    }
    if new_text == text {
        return Ok(format!("{}: no change", f.rel));
    }
    let before_err = crate::Lang::from_path(&f.abs).map(|l| parse::parse(l, &text).error_line.is_some()).unwrap_or(false);
    ctx.snapshot_for_undo(&f.abs);
    std::fs::write(&f.abs, new_text.as_bytes()).map_err(|e| format!("write {}: {e}", f.rel))?;
    if let Some(rel) = idx.rel(&f.abs) {
        idx.reindex(&rel, true);
    }
    // Changed region within the symbol.
    let pre = old_mid.iter().zip(new_mid.iter()).take_while(|(x, y)| x == y).count();
    let suf = old_mid[pre..].iter().rev().zip(new_mid[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let removed = old_mid.len() - pre - suf;
    let added = new_mid.len() - pre - suf;
    let new_end = a + new_mid.len();
    ctx.touch(&f.abs, "edit", new_text.as_bytes(), Some(format!("{a}-{}", new_end.saturating_sub(1).max(a))));
    let mut out = if new_mid.is_empty() {
        format!("{}: deleted {} {} (L{a}-{b})", f.rel, f.sym.kind, f.sym.name)
    } else {
        format!(
            "{}: {} {} L{a}-{b} → L{a}-{}; changed L{}-{} (-{removed} +{added})",
            f.rel,
            f.sym.kind,
            f.sym.name,
            new_end - 1,
            a + pre,
            (a + pre + added).saturating_sub(1).max(a + pre),
        )
    };
    if let Some(l) = crate::Lang::from_path(&f.abs) {
        if let Some(e) = parse::parse(l, &new_text).error_line {
            if !before_err {
                out.push_str(&format!("\nwarning: syntax error near L{e}"));
            }
        }
    }
    Ok(out)
}

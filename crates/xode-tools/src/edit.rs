use crate::util::{self, spec};
use crate::write::match_style;
use async_trait::async_trait;
use serde_json::Value;
use similar::{ChangeTag, TextDiff};
use xode_core::tool::{arg_bool, arg_str, Tool, ToolCtx, ToolOutput};
use xode_core::types::ToolSpec;

const MAX_DIFF_LINES: usize = 60;

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn spec(&self) -> ToolSpec {
        spec(
            "edit",
            "Replace exact text `old` with `new` in a file. `old` must be unique unless all=true.",
            &[("path", "string", ""), ("old", "string", ""), ("new", "string", ""), ("all", "boolean", "replace every match")],
            &["path", "old", "new"],
        )
    }
    fn summary(&self, a: &Value) -> String {
        arg_str(a, "path").unwrap_or("").to_string()
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let (Some(p), Some(old), Some(new)) = (arg_str(&args, "path"), arg_str(&args, "old"), arg_str(&args, "new")) else {
            return ToolOutput::err("need path, old, new");
        };
        let all = arg_bool(&args, "all").unwrap_or(false);
        let path = util::resolve_existing(ctx, p);
        let shown = ctx.display(&path);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ToolOutput::err(format!("not found: {shown} (use write to create)"))
            }
            Err(e) => return ToolOutput::err(format!("{shown}: {e}")),
        };
        if util::is_binary(&bytes) {
            return ToolOutput::err(format!("{shown} is binary"));
        }
        let text = util::to_lf(&util::decode(&bytes));
        let new_text = match apply_edit(&text, old, new, all, ctx.config.tools.edit_fuzzy) {
            Ok(x) => x,
            Err(e) => return ToolOutput::err(format!("{shown}: {e}")),
        };
        let out = match_style(Some(&bytes), &new_text.text);
        ctx.snapshot_for_undo(&path);
        if let Err(e) = std::fs::write(&path, &out) {
            return ToolOutput::err(format!("{shown}: {e}"));
        }
        ctx.touch(&path, "edit", &out, None);
        if let Some(o) = &ctx.outliner {
            o.file_changed(&path);
        }
        let (diff, add, del) = compact_diff(&text, &new_text.text);
        let mut head = format!("edited {shown} (+{add} -{del})");
        if new_text.count > 1 {
            head.push_str(&format!(" {} replacements", new_text.count));
        }
        if new_text.fuzzy {
            head.push_str(" [matched ignoring whitespace]");
        }
        ToolOutput::ok(format!("{head}\n{diff}").trim_end().to_string())
    }
}

#[derive(Debug)]
pub(crate) struct Edited {
    pub text: String,
    pub count: usize,
    pub fuzzy: bool,
}

/// A match: byte range in the (LF) text plus the replacement for it.
struct Hit {
    start: usize,
    end: usize,
    repl: String,
}

/// Apply an edit to LF-normalized `text`. `old`/`new` may contain CRLF.
pub(crate) fn apply_edit(text: &str, old: &str, new: &str, all: bool, fuzzy: bool) -> Result<Edited, String> {
    let old = util::to_lf(old);
    let new = util::to_lf(new);
    if old.is_empty() {
        return Err("old is empty".into());
    }
    if old == new {
        return Err("old and new are identical".into());
    }
    let mut used_fuzzy = false;
    let mut hits: Vec<Hit> =
        text.match_indices(old.as_str()).map(|(i, m)| Hit { start: i, end: i + m.len(), repl: new.clone() }).collect();
    if hits.is_empty() && fuzzy {
        for norm in [norm_trim as fn(&str) -> String, norm_ws] {
            hits = line_matches(text, &old, &new, norm);
            if !hits.is_empty() {
                used_fuzzy = true;
                break;
            }
        }
    }
    if hits.is_empty() {
        return Err(not_found_hint(text, &old));
    }
    if hits.len() > 1 && !all {
        let lines: Vec<String> = hits.iter().take(12).map(|h| line_of(text, h.start).to_string()).collect();
        return Err(format!(
            "old matches {} times (lines {}); add context to make it unique or set all=true",
            hits.len(),
            lines.join(", ")
        ));
    }
    let mut out = text.to_string();
    for h in hits.iter().rev() {
        out.replace_range(h.start..h.end, &h.repl);
    }
    Ok(Edited { text: out, count: hits.len(), fuzzy: used_fuzzy })
}

fn norm_trim(s: &str) -> String {
    s.trim().to_string()
}

fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|b| *b == b'\n').count() + 1
}

fn indent_of(s: &str) -> &str {
    &s[..s.len() - s.trim_start().len()]
}

/// Whole-line matching after normalizing each line; re-indents `new` to the file's indentation.
fn line_matches(text: &str, old: &str, new: &str, norm: fn(&str) -> String) -> Vec<Hit> {
    let old_lines: Vec<&str> = {
        let v: Vec<&str> = old.split('\n').collect();
        let a = v.iter().position(|l| !l.trim().is_empty());
        let b = v.iter().rposition(|l| !l.trim().is_empty());
        match (a, b) {
            (Some(a), Some(b)) => v[a..=b].to_vec(),
            _ => return vec![],
        }
    };
    let want: Vec<String> = old_lines.iter().map(|l| norm(l)).collect();
    // (start, end) byte offsets of each file line, excluding '\n'.
    let mut spans = vec![];
    let mut pos = 0;
    for l in text.split('\n') {
        spans.push((pos, pos + l.len()));
        pos += l.len() + 1;
    }
    let k = want.len();
    let mut hits = vec![];
    let mut i = 0;
    while i + k <= spans.len() {
        let ok = (0..k).all(|j| {
            let (s, e) = spans[i + j];
            norm(&text[s..e]) == want[j]
        });
        if ok {
            let file_indent = indent_of(&text[spans[i].0..spans[i].1]);
            let old_indent = indent_of(old_lines[0]);
            let new_body = new.trim_matches('\n');
            let repl = new_body
                .split('\n')
                .map(|l| match l.strip_prefix(old_indent) {
                    Some(rest) if !l.trim().is_empty() => format!("{file_indent}{rest}"),
                    _ => l.to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            hits.push(Hit { start: spans[i].0, end: spans[i + k - 1].1, repl });
            i += k;
        } else {
            i += 1;
        }
    }
    hits
}

fn not_found_hint(text: &str, old: &str) -> String {
    let first = old.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let near: Vec<String> = if first.len() >= 3 {
        text.lines().enumerate().filter(|(_, l)| l.contains(first)).take(5).map(|(i, _)| (i + 1).to_string()).collect()
    } else {
        vec![]
    };
    if near.is_empty() {
        "old not found; re-read the file and copy the text exactly".into()
    } else {
        format!("old not found; its first line appears at line {}; re-read those lines and copy exactly", near.join(", "))
    }
}

/// Unified diff with 2 lines of context, without file headers. Returns (diff, added, removed).
pub(crate) fn compact_diff(a: &str, b: &str) -> (String, usize, usize) {
    let d = TextDiff::from_lines(a, b);
    let (mut add, mut del) = (0, 0);
    let mut lines: Vec<String> = vec![];
    for hunk in d.unified_diff().context_radius(2).iter_hunks() {
        lines.push(hunk.header().to_string());
        for c in hunk.iter_changes() {
            let sign = match c.tag() {
                ChangeTag::Delete => {
                    del += 1;
                    '-'
                }
                ChangeTag::Insert => {
                    add += 1;
                    '+'
                }
                ChangeTag::Equal => ' ',
            };
            let v = c.value();
            lines.push(format!("{sign}{}", util::clip(v.strip_suffix('\n').unwrap_or(v), 300)));
        }
    }
    let n = lines.len();
    if n > MAX_DIFF_LINES {
        lines.truncate(MAX_DIFF_LINES);
        lines.push(format!("…[+{} diff lines]", n - MAX_DIFF_LINES));
    }
    (lines.join("\n"), add, del)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test::ctx;

    #[test]
    fn exact_and_ambiguous() {
        let t = "a\nfoo\nb\nfoo\n";
        let e = apply_edit(t, "foo", "bar", false, true).unwrap_err();
        assert!(e.contains("2 times (lines 2, 4)"), "{e}");
        let r = apply_edit(t, "foo", "bar", true, true).unwrap();
        assert_eq!(r.text, "a\nbar\nb\nbar\n");
        assert_eq!(r.count, 2);
        assert!(!r.fuzzy);
    }

    #[test]
    fn fuzzy_indent() {
        let t = "fn a() {\n        let x = 1;\n        let y  = 2;\n}\n";
        // Model got indentation and inner spacing wrong.
        let r = apply_edit(t, "  let x = 1;\n  let y = 2;\n", "  let x = 10;\n  let y = 20;\n", false, true).unwrap();
        assert!(r.fuzzy);
        assert_eq!(r.text, "fn a() {\n        let x = 10;\n        let y = 20;\n}\n");
        assert!(apply_edit(t, "  let x = 1;\n  let y = 2;\n", "z", false, false).is_err());
    }

    #[test]
    fn not_found_hint_lines() {
        let t = "one\ntwo three\nfour\n";
        let e = apply_edit(t, "two three\nfive", "x", false, true).unwrap_err();
        assert!(e.contains("line 2"), "{e}");
    }

    #[tokio::test]
    async fn crlf_file_edit() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let p = d.path().join("c.rs");
        std::fs::write(&p, "fn a() {\r\n    1\r\n}\r\n").unwrap();
        // Model sends LF; file is CRLF.
        let r = EditTool.run(serde_json::json!({"path": "c.rs", "old": "fn a() {\n    1\n", "new": "fn a() {\n    2\n"}), &c).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "fn a() {\r\n    2\r\n}\r\n");
        assert!(r.content.starts_with("edited c.rs (+1 -1)"), "{}", r.content);
        assert!(r.content.contains("-    1\n+    2"), "{}", r.content);
        let st = c.state.lock();
        assert_eq!(st.touched["c.rs"].how, "edit");
        assert_eq!(st.undo.len(), 1);
    }
}

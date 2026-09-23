//! The `kb` tool: compact search / outline-first reading / browsing / memory writes.

use crate::md::est_tokens;
use crate::search::SearchOpts;
use crate::store::NoteView;
use crate::{Kb, Layer, Sel};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use xode_core::context::clip;
use xode_core::tool::{arg_str, Tool, ToolCtx, ToolOutput, ToolRef};
use xode_core::types::ToolSpec;

const LIST_MAX: usize = 60;
const LINKS_MAX: usize = 30;

pub fn kb_tool(kb: Kb) -> ToolRef {
    Arc::new(KbTool { kb })
}

struct KbTool {
    kb: Kb,
}

fn ktok(n: u32) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[async_trait]
impl Tool for KbTool {
    fn name(&self) -> &str {
        "kb"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "kb".into(),
            description: "Knowledge base of notes (docs, skills, templates, memory). search{q,k?,tag?,layer?}: ranked hits with ids. read{id,section?|lines?}: long notes give an outline first; then read a section. list{path?|tag?}: browse sources/folders/tags. links{id}: linked notes. write{title,body,tags?,global?}|{id,body}: save a memory note (project by default; [[Title]] links). delete{id}.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["search", "read", "list", "links", "write", "delete"]},
                    "q": {"type": "string"},
                    "id": {"type": "string"},
                    "section": {"type": "string"},
                    "lines": {"type": "string"},
                    "path": {"type": "string"},
                    "tag": {"type": "string"},
                    "layer": {"type": "string", "enum": ["library", "docs", "memory", "project_memory"]},
                    "k": {"type": "integer"},
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "tags": {"type": "string"},
                    "global": {"type": "boolean"}
                },
                "required": ["action"]
            }),
        }
    }

    fn read_only(&self, args: &Value) -> bool {
        !matches!(arg_str(args, "action"), Some("write" | "delete"))
    }

    fn summary(&self, args: &Value) -> String {
        let a = arg_str(args, "action").unwrap_or("?");
        let s = ["q", "id", "section", "path", "tag", "title"]
            .iter()
            .filter_map(|k| arg_str(args, k))
            .collect::<Vec<_>>()
            .join(" ");
        clip(format!("kb {a} {s}").trim(), 160)
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let kb = self.kb.clone();
        let sel = Sel::new(&ctx.state.lock().kb_off);
        let cfg = ctx.config.knowledge.clone();
        let r = tokio::task::spawn_blocking(move || run(&kb, &sel, &cfg, &args)).await;
        match r {
            Ok(Ok(s)) => ToolOutput::ok(s),
            Ok(Err(e)) => ToolOutput::err(e),
            Err(e) => ToolOutput::err(format!("kb failed: {e}")),
        }
    }
}

fn run(kb: &Kb, sel: &Sel, cfg: &xode_core::config::Knowledge, args: &Value) -> Result<String, String> {
    let s = |k: &str| arg_str(args, k).map(str::trim).filter(|x| !x.is_empty());
    match s("action").unwrap_or("") {
        "search" => {
            let q = s("q").ok_or("q required")?;
            let opts = SearchOpts {
                k: args.get("k").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(cfg.k as usize),
                layer: s("layer").and_then(Layer::parse),
                tag: s("tag").map(String::from),
                text_only: false,
            };
            let hits = kb.search(sel, q, &opts);
            if hits.is_empty() {
                return Ok("no matches (try other words, fewer terms, or kb list)".into());
            }
            let mut out = String::new();
            for h in hits {
                let head = if h.heading.is_empty() { String::new() } else { format!(" › {}", h.heading) };
                out.push_str(&format!(
                    "{} {}{} · L{}-{} · {}/{} tok\n  {}\n",
                    h.id,
                    h.title,
                    head,
                    h.line_start,
                    h.line_end,
                    ktok(h.tokens),
                    ktok(h.note_tokens),
                    h.snippet
                ));
            }
            Ok(out.trim_end().to_string())
        }
        "read" => {
            let id = s("id").ok_or("id required")?;
            let n = note(kb, sel, id)?;
            Ok(read(&n, s("section"), s("lines"), cfg))
        }
        "list" => Ok(list(kb, sel, s("path").unwrap_or(""), s("tag").unwrap_or(""))),
        "links" => {
            let id = s("id").ok_or("id required")?;
            let n = note(kb, sel, id)?;
            let fmt = |v: &[crate::store::NoteLink]| {
                let mut parts: Vec<String> = v
                    .iter()
                    .take(LINKS_MAX)
                    .map(|l| match &l.id {
                        Some(i) => format!("{i} {}", l.title),
                        None => match kb.stores().find_map(|s| s.find_by_key(&l.title)) {
                            Some((i, t)) => format!("{i} {t}"),
                            None => format!("{} (missing)", l.title),
                        },
                    })
                    .collect();
                if v.len() > LINKS_MAX {
                    parts.push(format!("+{} more", v.len() - LINKS_MAX));
                }
                if parts.is_empty() {
                    "-".to_string()
                } else {
                    parts.join("; ")
                }
            };
            Ok(format!("{} {}\n→ {}\n← {}", n.id, n.title, fmt(&n.links), fmt(&n.backlinks)))
        }
        "write" => {
            let body = s("body").ok_or("body required")?;
            let tags: Vec<String> = s("tags")
                .map(|t| t.split([',', ' ']).map(|x| x.trim().trim_start_matches('#').to_lowercase()).filter(|x| !x.is_empty()).collect())
                .unwrap_or_default();
            let (st, layer) = match s("id") {
                Some(id) => {
                    let st = kb.store_for(id).ok_or_else(|| format!("no note {id}"))?;
                    (st.clone(), st.memory_layer())
                }
                None => {
                    let global = args.get("global").and_then(|v| v.as_bool()).unwrap_or(false) || kb.project.is_none();
                    let st = if global { kb.global.as_ref() } else { kb.project.as_ref() };
                    let st = st.ok_or("no memory store available")?;
                    (st.clone(), st.memory_layer())
                }
            };
            let allowed_cfg = if layer == Layer::Memory { cfg.ai_write_global } else { cfg.ai_write_project };
            if !allowed_cfg {
                return Err(format!("writing to {} is disabled in settings", layer.label()));
            }
            if !sel.layer_on(layer) {
                return Err(format!("{} is switched off for this chat", layer.label()));
            }
            let (nid, _) = st.write_note(s("id"), s("title").unwrap_or(""), body, &tags).map_err(|e| e.to_string())?;
            Ok(match nid {
                Some(i) => format!("saved {i} ({})", layer.label().to_lowercase()),
                None => format!("saved to {} (indexing in another Xode window)", layer.label().to_lowercase()),
            })
        }
        "delete" => {
            let id = s("id").ok_or("id required")?;
            let n = note(kb, sel, id)?;
            if !sel.layer_on(n.layer) {
                return Err(format!("{} is switched off for this chat", n.layer.label()));
            }
            kb.store_for(id).ok_or("no store")?.delete_note(id).map_err(|e| e.to_string())?;
            Ok(format!("deleted {id}"))
        }
        other => Err(format!("unknown action `{other}` (search, read, list, links, write, delete)")),
    }
}

fn note(kb: &Kb, sel: &Sel, id: &str) -> Result<NoteView, String> {
    let n = kb
        .store_for(id)
        .and_then(|s| s.note(id))
        .ok_or_else(|| format!("no note `{id}` (ids look like g12 / p3; use kb search)"))?;
    if !sel.allows(n.layer, &n.source) {
        return Err(format!("`{id}` is in a source switched off for this chat"));
    }
    Ok(n)
}

/// Line span of each heading's section: (heading idx) → (start, end) 1-based inclusive.
fn sections(n: &NoteView, total: u32) -> Vec<(u32, u32)> {
    let hs = &n.headings;
    hs.iter()
        .enumerate()
        .map(|(i, (lv, _, line))| {
            let end = hs[i + 1..].iter().find(|(l2, _, _)| l2 <= lv).map(|(_, _, l)| l - 1).unwrap_or(total);
            (*line, end.max(*line))
        })
        .collect()
}

fn slice(lines: &[&str], a: u32, b: u32) -> String {
    let a = a.max(1) as usize;
    let b = (b as usize).min(lines.len());
    if a > b {
        return String::new();
    }
    lines[a - 1..b].join("\n")
}

/// Text of lines a..=b capped at `max` tokens, with a continuation hint.
fn capped(lines: &[&str], a: u32, b: u32, max: u32) -> String {
    let mut out = String::new();
    let mut tok = 0;
    let mut last = a.saturating_sub(1);
    for ln in a.max(1)..=b.min(lines.len() as u32) {
        let l = lines[ln as usize - 1];
        let t = est_tokens(l);
        if tok + t > max && ln > a {
            out.push_str(&format!("\n… cut at L{last}; continue with lines={}-{b}", last + 1));
            return out;
        }
        tok += t;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(l);
        last = ln;
    }
    out
}

fn read(n: &NoteView, section: Option<&str>, lines_arg: Option<&str>, cfg: &xode_core::config::Knowledge) -> String {
    let lines: Vec<&str> = n.text.lines().collect();
    let total = lines.len() as u32;
    let tags = if n.tags.is_empty() { String::new() } else { format!(" · #{}", n.tags.join(" #")) };
    let header = format!("{} {} · {}/{} · {} tok{}", n.id, n.title, n.source_name, n.rel, ktok(n.tokens), tags);
    let max = cfg.read_max_tokens.max(300);
    if let Some(r) = lines_arg {
        let (a, b) = r.split_once('-').unwrap_or((r, r));
        let a: u32 = a.trim().trim_start_matches('L').parse().unwrap_or(1);
        let b: u32 = b.trim().trim_start_matches('L').parse().unwrap_or(total);
        return format!("{header} · L{a}-{}\n{}", b.min(total), capped(&lines, a, b.min(total), max));
    }
    if let Some(sec) = section {
        let want = sec.trim().trim_start_matches('#').trim().to_lowercase();
        let spans = sections(n, total);
        let idx = n
            .headings
            .iter()
            .position(|(_, t, _)| t.to_lowercase() == want)
            .or_else(|| n.headings.iter().position(|(_, t, _)| t.to_lowercase().contains(&want)));
        return match idx {
            Some(i) => {
                let (a, b) = spans[i];
                format!("{header} · L{a}-{b}\n{}", capped(&lines, a, b, max))
            }
            None => {
                let hs: Vec<&str> = n.headings.iter().map(|(_, t, _)| t.as_str()).take(40).collect();
                format!("{header}\nno section `{sec}`. sections: {}", hs.join(" | "))
            }
        };
    }
    if n.tokens <= cfg.outline_tokens || n.headings.len() < 2 {
        return format!("{header}\n{}", capped(&lines, 1, total, max));
    }
    // Outline first for long notes.
    let spans = sections(n, total);
    let mut out = format!("{header}\n");
    if !n.summary.is_empty() {
        out.push_str(&format!("{}\n", n.summary));
    }
    out.push_str("outline:\n");
    for (i, (lv, t, line)) in n.headings.iter().enumerate().take(80) {
        let (a, b) = spans[i];
        let tok = est_tokens(&slice(&lines, a, b));
        out.push_str(&format!("L{line} {} {t} ~{}\n", "#".repeat(*lv as usize), ktok(tok)));
    }
    if n.headings.len() > 80 {
        out.push_str(&format!("…+{} more headings\n", n.headings.len() - 80));
    }
    if !n.links.is_empty() || !n.backlinks.is_empty() {
        out.push_str(&format!("links: {} out, {} in (kb links)\n", n.links.len(), n.backlinks.len()));
    }
    out.push_str("read a section (section=) or lines= to see text");
    out
}

fn list(kb: &Kb, sel: &Sel, path: &str, tag: &str) -> String {
    let sources: Vec<crate::Source> = kb.sources().into_iter().filter(|s| sel.allows(s.layer, &s.key)).collect();
    if sources.is_empty() {
        return "knowledge base is empty or switched off for this chat".into();
    }
    if !tag.is_empty() {
        let mut out = format!("#{}\n", tag.trim_start_matches('#'));
        let mut n = 0;
        for st in kb.stores() {
            let ids: Vec<i64> = sources.iter().filter(|s| st.parse_id(&s.key).is_some()).map(|s| s.id).collect();
            let (_, notes) = st.list(&ids, "", tag, LIST_MAX);
            for r in notes {
                n += 1;
                if n <= LIST_MAX {
                    out.push_str(&format!("{} {} · {}\n", r.id, r.title, ktok(r.tokens)));
                }
            }
        }
        if n == 0 {
            return format!("no notes tagged #{}", tag.trim_start_matches('#'));
        }
        return out.trim_end().to_string();
    }
    let path = path.trim().trim_matches('/');
    if path.is_empty() {
        let mut out = String::new();
        for l in Layer::ALL {
            let ss: Vec<&crate::Source> = sources.iter().filter(|s| s.layer == l).collect();
            if ss.is_empty() {
                continue;
            }
            let parts: Vec<String> = ss.iter().map(|s| format!("{} ({}) {} notes", s.name, s.key, s.notes)).collect();
            out.push_str(&format!("{}: {}\n", l.label(), parts.join(", ")));
        }
        let mut tags: Vec<(String, u64)> = vec![];
        for st in kb.stores() {
            let ids: Vec<i64> = sources.iter().filter(|s| st.parse_id(&s.key).is_some()).map(|s| s.id).collect();
            for (t, c) in st.top_tags(&ids, 25) {
                match tags.iter_mut().find(|x| x.0 == t) {
                    Some(x) => x.1 += c,
                    None => tags.push((t, c)),
                }
            }
        }
        tags.sort_by(|a, b| b.1.cmp(&a.1));
        if !tags.is_empty() {
            let t: Vec<String> = tags.iter().take(25).map(|(t, c)| format!("#{t} {c}")).collect();
            out.push_str(&format!("tags: {}\n", t.join(", ")));
        }
        out.push_str("list{path:\"<source>/<folder>\"} to browse");
        return out;
    }
    // "<source name or key>/<folder…>"
    let (head, rest) = path.split_once('/').unwrap_or((path, ""));
    let Some(src) = sources
        .iter()
        .find(|s| s.key.eq_ignore_ascii_case(head) || s.name.eq_ignore_ascii_case(head))
        .or_else(|| sources.iter().find(|s| s.name.to_lowercase().starts_with(&head.to_lowercase())))
    else {
        let names: Vec<&str> = sources.iter().map(|s| s.name.as_str()).collect();
        return format!("no source `{head}`. sources: {}", names.join(", "));
    };
    let Some(st) = kb.store_for(&src.key) else { return "no store".into() };
    let (folders, notes) = st.list(&[src.id], rest, "", LIST_MAX);
    let mut out = format!("{}/{} ({})\n", src.name, rest, src.key);
    for (f, c) in &folders {
        let name = f.rsplit('/').next().unwrap_or(f);
        out.push_str(&format!("  {name}/ {c}\n"));
    }
    for r in &notes {
        out.push_str(&format!("  {} {} · {}\n", r.id, r.title, ktok(r.tokens)));
    }
    if folders.is_empty() && notes.is_empty() {
        out.push_str("  (empty)");
    }
    if notes.len() >= LIST_MAX {
        out.push_str("  … more (narrow the folder or search)");
    }
    out.trim_end().to_string()
}

#[cfg(test)]
pub(crate) fn run_for_test(kb: &Kb, sel: &Sel, cfg: &xode_core::config::Knowledge, args: &Value) -> Result<String, String> {
    run(kb, sel, cfg, args)
}

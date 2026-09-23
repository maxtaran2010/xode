//! Pure compaction helpers: reply parsing, PATH.md maintenance, working set and seed rendering.
use crate::config::Compaction;
use crate::tool::{FileTouch, Outliner};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::Path;

static PATH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<path>\s*(.*?)\s*(</path>|<state>|$)").unwrap());
static STATE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<state>\s*(.*?)\s*(</state>|$)").unwrap());

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Handoff {
    pub path: Vec<String>,
    pub state: String,
}

/// Parse the model's compaction reply. Tolerates missing closing tags and markdown headers.
pub fn parse_reply(text: &str, cfg: &Compaction) -> Option<Handoff> {
    let text = strip_think(text);
    let path_raw = PATH_RE.captures(&text).map(|c| c[1].to_string());
    let state_raw = STATE_RE.captures(&text).map(|c| c[1].to_string());
    let (path_raw, state_raw) = match (path_raw, state_raw) {
        (Some(p), Some(s)) => (p, s),
        (Some(p), None) => (p, String::new()),
        (None, Some(s)) => (String::new(), s),
        (None, None) => {
            // Fallback: headings "PATH"/"STATE".
            let l = text.to_lowercase();
            let pi = l.find("path")?;
            let si = l.find("state").unwrap_or(text.len());
            if si > pi {
                (text[pi + 4..si].to_string(), text[si.min(text.len())..].trim_start_matches(|c: char| c.is_alphabetic() || c == ':').to_string())
            } else {
                return None;
            }
        }
    };
    let mut path: Vec<String> = path_raw
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim())
        .filter(|l| !l.is_empty() && !l.starts_with('<'))
        .map(|l| crate::context::clip(l, 160))
        .collect();
    path.truncate(cfg.path_entry_max_lines.max(1));
    let state = limit_words(state_raw.trim(), cfg.state_max_words.max(20));
    if path.is_empty() && state.is_empty() {
        return None;
    }
    Some(Handoff { path, state })
}

fn strip_think(s: &str) -> String {
    match (s.find("<think>"), s.find("</think>")) {
        (Some(a), Some(b)) if b > a => format!("{}{}", &s[..a], &s[b + 8..]),
        (None, Some(b)) => s[b + 8..].to_string(),
        _ => s.to_string(),
    }
}

fn limit_words(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for line in s.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        if count + words.len() > n {
            let take = n.saturating_sub(count);
            if take > 0 {
                out.push_str(&words[..take].join(" "));
                out.push('…');
            }
            break;
        }
        count += words.len();
        out.push_str(line.trim());
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub const PATH_HEADER: &str = "# Path\n";

/// Append a handoff block to PATH.md content and fold old blocks to stay under `max_lines`.
pub fn append_path(existing: &str, header: &str, bullets: &[String], cfg: &Compaction) -> String {
    let mut blocks = split_blocks(existing);
    let mut b = vec![format!("## {header}")];
    b.extend(bullets.iter().map(|x| format!("- {x}")));
    blocks.push(b);
    if cfg.fold_old_entries {
        fold(&mut blocks, cfg.path_max_lines.max(10));
    }
    let mut out = String::from(PATH_HEADER);
    for b in blocks {
        out.push('\n');
        for l in b {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out
}

fn split_blocks(s: &str) -> Vec<Vec<String>> {
    let mut blocks: Vec<Vec<String>> = vec![];
    for line in s.lines() {
        let t = line.trim_end();
        if t.is_empty() || t == PATH_HEADER.trim() {
            continue;
        }
        if t.starts_with("## ") || blocks.is_empty() {
            blocks.push(vec![t.to_string()]);
        } else if let Some(b) = blocks.last_mut() {
            b.push(t.to_string());
        }
    }
    blocks
}

fn line_count(blocks: &[Vec<String>]) -> usize {
    blocks.iter().map(|b| b.len() + 1).sum::<usize>() + 1
}

/// Fold oldest blocks into one "## earlier" block keeping only their first bullet (clipped).
fn fold(blocks: &mut Vec<Vec<String>>, max_lines: usize) {
    while line_count(blocks) > max_lines && blocks.len() > 2 {
        let has_earlier = blocks[0].first().map(|h| h.starts_with("## earlier")).unwrap_or(false);
        let victim_idx = if has_earlier { 1 } else { 0 };
        if victim_idx >= blocks.len() - 1 {
            break;
        }
        let victim = blocks.remove(victim_idx);
        let date = victim[0].trim_start_matches("## ").split(' ').next().unwrap_or("").to_string();
        let first = victim.iter().skip(1).next().map(|l| l.trim_start_matches("- ").to_string()).unwrap_or_default();
        let summary = format!("- {date}: {}", crate::context::clip(&first, 100));
        if has_earlier {
            blocks[0].push(summary);
        } else {
            blocks.insert(0, vec!["## earlier".into(), summary]);
        }
        // Earlier block itself too long: drop its oldest lines.
        let cap = (max_lines / 3).max(4);
        if blocks[0].len() > cap {
            let extra = blocks[0].len() - cap;
            blocks[0].drain(1..1 + extra);
        }
    }
}

pub fn read_path_md(root: &Path, cfg: &Compaction) -> String {
    std::fs::read_to_string(root.join(&cfg.path_file)).unwrap_or_default()
}

pub fn write_path_md(root: &Path, cfg: &Compaction, content: &str) -> std::io::Result<()> {
    let p = root.join(&cfg.path_file);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, content)
}

/// Files the agent worked with, most recent first, with outlines (no contents).
pub fn working_set(
    touched: &[FileTouch],
    root: &Path,
    outliner: Option<&dyn Outliner>,
    max_files: usize,
    budget_tokens: u64,
) -> String {
    let mut files: Vec<&FileTouch> = touched.iter().collect();
    files.sort_by(|a, b| b.step.cmp(&a.step).then_with(|| rank(&b.how).cmp(&rank(&a.how))));
    let mut out = String::from("## Working set (files you already know; outlines only)\n");
    let mut used = crate::tokens::count(&out);
    for f in files.into_iter().take(max_files) {
        let abs = root.join(&f.path);
        let cur_hash = std::fs::read(&abs).ok().map(|b| crate::tool::hash_bytes(&b));
        let changed = match &cur_hash {
            Some(h) if *h != f.hash => " (changed on disk since)",
            None => " (deleted)",
            _ => "",
        };
        let ranges = if f.ranges.is_empty() || f.how != "read" { String::new() } else { format!(" read {}", f.ranges.join(",")) };
        let mut entry = format!("- {} [{}{}, {} lines]{}\n", f.path, f.how, ranges, f.lines, changed);
        if let Some(o) = outliner.and_then(|o| o.outline(&abs)) {
            let lines: Vec<&str> = o.lines().take(14).collect();
            let more = o.lines().count().saturating_sub(lines.len());
            for l in lines {
                entry.push_str("    ");
                entry.push_str(l);
                entry.push('\n');
            }
            if more > 0 {
                entry.push_str(&format!("    …+{more}\n"));
            }
        }
        let t = crate::tokens::count(&entry);
        if used + t > budget_tokens {
            // Try without outline.
            let short = format!("- {} [{}, {} lines]{}\n", f.path, f.how, f.lines, changed);
            let ts = crate::tokens::count(&short);
            if used + ts > budget_tokens {
                break;
            }
            used += ts;
            out.push_str(&short);
            continue;
        }
        used += t;
        out.push_str(&entry);
    }
    out
}

fn rank(how: &str) -> u8 {
    match how {
        "edit" | "write" => 2,
        _ => 1,
    }
}

pub fn render_seed(template: &str, request: &str, path: &str, state: &str, goal: Option<&str>, working_set: &str, recent: &str) -> String {
    let goal = goal.map(|g| format!("## Goal\n{g}")).unwrap_or_default();
    let request = if request.trim().is_empty() { String::new() } else { format!("## Original request\n{}", request.trim()) };
    let mut s = template
        .replace("{request}", &request)
        .replace("{path}", if path.trim().is_empty() { "(empty)" } else { path.trim() })
        .replace("{state}", state.trim())
        .replace("{goal}", &goal)
        .replace("{working_set}", working_set.trim());
    if !recent.trim().is_empty() {
        s.push_str("\n\n## Last messages before compaction\n");
        s.push_str(recent.trim());
    }
    // Collapse 3+ blank lines left by empty placeholders.
    while s.contains("\n\n\n") {
        s = s.replace("\n\n\n", "\n\n");
    }
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags() {
        let cfg = Compaction::default();
        let h = parse_reply(
            "<think>hmm</think><path>\n- added parser in src/a.rs\n- tests pass\n</path>\n<state>\ngoal: x\nnext: y\n</state>",
            &cfg,
        )
        .unwrap();
        assert_eq!(h.path, vec!["added parser in src/a.rs", "tests pass"]);
        assert!(h.state.contains("goal: x"));
    }

    #[test]
    fn parses_unclosed() {
        let cfg = Compaction::default();
        let h = parse_reply("<path>\n- a\n<state>\ngoal: g", &cfg).unwrap();
        assert_eq!(h.path, vec!["a"]);
        assert_eq!(h.state, "goal: g");
    }

    #[test]
    fn path_md_stays_short() {
        let cfg = Compaction { path_max_lines: 20, ..Default::default() };
        let mut s = String::new();
        for i in 0..30 {
            s = append_path(
                &s,
                &format!("2026-09-{:02} chat #{i}", i % 28 + 1),
                &[format!("did thing {i}"), "more".into(), "and more".into()],
                &cfg,
            );
        }
        assert!(s.lines().count() <= 22, "{}", s);
        assert!(s.contains("## earlier"));
        assert!(s.contains("did thing 29"));
    }

    #[test]
    fn seed_renders() {
        let s = render_seed(crate::config::DEFAULT_SEED_TEMPLATE, "fix bug", "# Path\n- a", "goal: g", None, "## Working set\n- a.rs", "");
        assert!(s.contains("fix bug"));
        assert!(!s.contains("{goal}"));
        assert!(!s.contains("\n\n\n"));
    }
}

//! Note parsing: front matter, title, tags, wiki links, headings and heading-aware chunking.

use once_cell::sync::Lazy;
use regex::Regex;

/// Rough token estimate (no tokenizer: ingest must handle gigabytes).
pub fn est_tokens(s: &str) -> u32 {
    let mut ascii = 0u32;
    let mut other = 0u32;
    for c in s.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    ascii / 4 + other / 2 + 1
}

#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    /// 1-based line.
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Heading path, e.g. "Setup › Windows".
    pub heading: String,
    pub line_start: u32,
    pub line_end: u32,
    pub tokens: u32,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub title: String,
    pub tags: Vec<String>,
    /// Link targets, normalized with [`link_key`].
    pub links: Vec<String>,
    pub summary: String,
    pub headings: Vec<Heading>,
    pub chunks: Vec<Chunk>,
    pub tokens: u32,
}

/// What kind of text a file holds (decides chunking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Text,
}

pub fn kind_of(path: &str) -> Option<Kind> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    Some(match ext.as_str() {
        "md" | "markdown" | "mdx" => Kind::Markdown,
        "txt" | "rst" | "org" | "adoc" | "text" | "csv" | "tsv" | "json" | "jsonl" | "yaml" | "yml" | "toml" | "ini"
        | "xml" | "html" | "htm" | "tex" | "log" | "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "c" | "h"
        | "cpp" | "hpp" | "cs" | "java" | "kt" | "swift" | "rb" | "php" | "lua" | "sh" | "ps1" | "sql" | "css"
        | "scss" | "vue" | "svelte" => Kind::Text,
        _ => return None,
    })
}

/// Normalized key a `[[link]]` or file name resolves by: lowercase stem, no extension/folders.
pub fn link_key(s: &str) -> String {
    let s = s.trim().replace('\\', "/");
    let base = s.rsplit('/').next().unwrap_or(&s);
    let base = base.strip_suffix(".md").or_else(|| base.strip_suffix(".markdown")).unwrap_or(base);
    base.trim().to_lowercase()
}

static WIKI: Lazy<Regex> = Lazy::new(|| Regex::new(r"!?\[\[([^\]\|#\n]+)(?:#[^\]\|\n]*)?(?:\|[^\]\n]*)?\]\]").unwrap());
static MDLINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\]\(([^)\s]+\.md)(?:#[^)]*)?\)").unwrap());
static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|[\s(])#([\p{L}\p{N}_][\p{L}\p{N}_/-]*)").unwrap());
static HEAD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*#*\s*$").unwrap());

const MIN_CHUNK: u32 = 60;

/// Split front matter (`---` … `---`) off the top. Returns (front matter lines, body start line index).
fn front_matter(lines: &[&str]) -> (Vec<String>, usize) {
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return (vec![], 0);
    }
    for (i, l) in lines.iter().enumerate().skip(1).take(200) {
        if matches!(l.trim_end(), "---" | "...") {
            return (lines[1..i].iter().map(|s| s.to_string()).collect(), i + 1);
        }
    }
    (vec![], 0)
}

fn fm_list(v: &str) -> Vec<String> {
    v.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|t| t.trim().trim_matches(|c| c == '"' || c == '\'' || c == '#').to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

pub fn parse(path: &str, src: &str, kind: Kind, chunk_tokens: u32) -> Parsed {
    let lines: Vec<&str> = src.lines().collect();
    let target = chunk_tokens.max(100);
    let hard = target * 2;
    let stem = path.rsplit('/').next().unwrap_or(path);
    let stem = stem.rsplit_once('.').map(|(a, _)| a).unwrap_or(stem).to_string();
    let mut p = Parsed { tokens: est_tokens(src), ..Default::default() };

    let (fm, body_start) = if kind == Kind::Markdown { front_matter(&lines) } else { (vec![], 0) };
    let mut fm_title = None;
    let mut in_tags = false;
    for l in &fm {
        if in_tags {
            if let Some(t) = l.trim_start().strip_prefix("- ") {
                p.tags.push(t.trim().trim_matches(|c| c == '"' || c == '\'' || c == '#').to_string());
                continue;
            }
            in_tags = false;
        }
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "title" => fm_title = Some(v.trim().trim_matches(|c| c == '"' || c == '\'').to_string()),
                "tags" | "tag" => {
                    if v.trim().is_empty() {
                        in_tags = true;
                    } else {
                        p.tags.extend(fm_list(v));
                    }
                }
                _ => {}
            }
        }
    }

    // Headings, tags and links outside code fences.
    let mut fence = false;
    let mut is_heading = vec![false; lines.len()];
    for (i, l) in lines.iter().enumerate().skip(body_start) {
        let t = l.trim_start();
        if kind == Kind::Markdown && (t.starts_with("```") || t.starts_with("~~~")) {
            fence = !fence;
            continue;
        }
        if fence || kind != Kind::Markdown {
            continue;
        }
        if let Some(c) = HEAD.captures(l) {
            is_heading[i] = true;
            p.headings.push(Heading { level: c[1].len() as u8, text: c[2].trim().to_string(), line: i as u32 + 1 });
            continue;
        }
        for c in TAG.captures_iter(l) {
            p.tags.push(c[1].to_string());
        }
        for c in WIKI.captures_iter(l) {
            p.links.push(link_key(&c[1]));
        }
        for c in MDLINK.captures_iter(l) {
            if !c[1].contains("://") {
                p.links.push(link_key(&c[1]));
            }
        }
    }
    p.tags.iter_mut().for_each(|t| *t = t.to_lowercase());
    dedup(&mut p.tags);
    dedup(&mut p.links);
    p.links.retain(|l| !l.is_empty() && *l != stem.to_lowercase());

    p.title = fm_title
        .filter(|t| !t.is_empty())
        .or_else(|| p.headings.iter().find(|h| h.level == 1).map(|h| h.text.clone()))
        .unwrap_or(stem);

    // Summary: first paragraph of prose.
    let mut para = String::new();
    let mut fence = false;
    for (i, l) in lines.iter().enumerate().skip(body_start) {
        let t = l.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || is_heading[i] || t.starts_with('|') || t.starts_with("<!--") {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if t.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(t.trim_start_matches(['-', '*', '>', ' ']));
        if para.len() > 600 {
            break;
        }
    }
    p.summary = clip_words(&para, 40);

    // Chunks: split at headings (once the current chunk has some substance), at blank lines past
    // the target size, and hard at twice the target.
    let mut path: Vec<(u8, String)> = vec![];
    let mut cur: Vec<&str> = vec![];
    let mut cur_start = body_start as u32 + 1;
    let mut cur_tok = 0u32;
    let mut cur_head = String::new();
    let mut fence = false;
    let flush = |p: &mut Parsed, cur: &mut Vec<&str>, start: u32, end: u32, head: &str| {
        let text = cur.join("\n");
        if !text.trim().is_empty() {
            let tokens = est_tokens(&text);
            p.chunks.push(Chunk { heading: head.to_string(), line_start: start, line_end: end, tokens, text });
        }
        cur.clear();
    };
    // Heading path without the H1 that is the note's title.
    let title = p.title.clone();
    let head_of = |path: &[(u8, String)]| {
        path.iter()
            .filter(|(lv, t)| !(*lv == 1 && *t == title))
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" › ")
    };
    for (i, l) in lines.iter().enumerate().skip(body_start) {
        let ln = i as u32 + 1;
        let t = l.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        if is_heading[i] {
            let h = p.headings.iter().find(|h| h.line == ln).cloned();
            if cur_tok >= MIN_CHUNK || cur.iter().all(|x| x.trim().is_empty()) {
                if cur_tok > 0 {
                    flush(&mut p, &mut cur, cur_start, ln - 1, &cur_head);
                }
                cur.clear();
                cur_tok = 0;
                cur_start = ln;
            }
            if let Some(h) = h {
                path.retain(|(lv, _)| *lv < h.level);
                path.push((h.level, h.text));
                // A tiny preamble merged into this section takes the section's name.
                if cur_tok < MIN_CHUNK / 2 {
                    cur_head = head_of(&path);
                }
            }
        } else if !fence
            && ((cur_tok >= target && l.trim().is_empty()) || cur_tok >= hard)
            && cur.iter().any(|x| !x.trim().is_empty())
        {
            flush(&mut p, &mut cur, cur_start, ln - 1, &cur_head);
            cur_tok = 0;
            cur_start = ln;
            cur_head = head_of(&path);
            if l.trim().is_empty() {
                cur_start = ln + 1;
                continue;
            }
        }
        cur.push(l);
        cur_tok += est_tokens(l);
    }
    let end = lines.len() as u32;
    flush(&mut p, &mut cur, cur_start, end, &cur_head);
    p
}

fn dedup(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|x| seen.insert(x.clone()));
}

pub fn clip_words(s: &str, n: usize) -> String {
    let w: Vec<&str> = s.split_whitespace().collect();
    if w.len() <= n {
        w.join(" ")
    } else {
        format!("{}…", w[..n].join(" "))
    }
}

/// File-name slug for a note title (keeps Unicode letters).
pub fn slug(title: &str) -> String {
    let mut s = String::new();
    for c in title.trim().chars() {
        if c.is_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() > 80 {
            break;
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "note".into()
    } else {
        s
    }
}

/// Markdown file content for a note written by the agent or the app.
pub fn render_note(title: &str, tags: &[String], body: &str) -> String {
    let mut s = String::new();
    if !tags.is_empty() {
        s.push_str(&format!("---\ntags: [{}]\n---\n", tags.join(", ")));
    }
    let body = body.trim();
    if !body.starts_with("# ") {
        s.push_str(&format!("# {}\n\n", title.trim()));
    }
    s.push_str(body);
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_front_matter_links_tags_and_chunks() {
        let src = "---\ntitle: Build Guide\ntags:\n  - windows\n  - build\n---\nHow we build Xode on #ci machines. See [[Release Notes|notes]] and [setup](docs/Setup.md).\n\n## Windows\nUse MSVC.\n```\n# not a heading #nottag\n```\n## Linux\nUse gcc [[Toolchains#gcc]].\n";
        let p = parse("guides/build.md", src, Kind::Markdown, 400);
        assert_eq!(p.title, "Build Guide");
        assert_eq!(p.tags, vec!["windows", "build", "ci"]);
        assert_eq!(p.links, vec!["release notes", "setup", "toolchains"]);
        assert!(p.summary.starts_with("How we build Xode"));
        assert_eq!(p.headings.len(), 2);
        assert!(!p.tags.contains(&"nottag".to_string()));
        assert!(!p.chunks.is_empty());
        assert!(p.chunks.iter().all(|c| c.line_start <= c.line_end));
    }

    #[test]
    fn splits_long_sections_and_tracks_heading_path() {
        let para = "word ".repeat(120);
        let mut src = String::from("# Top\n\n");
        for i in 0..6 {
            src.push_str(&format!("## Part {i}\n\n{para}\n\n{para}\n\n"));
        }
        let p = parse("x.md", &src, Kind::Markdown, 100);
        assert!(p.chunks.len() >= 6, "{}", p.chunks.len());
        assert!(p.chunks.iter().any(|c| c.heading == "Part 3"), "{:?}", p.chunks.iter().map(|c| &c.heading).collect::<Vec<_>>());
        assert!(p.chunks.iter().all(|c| c.tokens <= 260), "{:?}", p.chunks.iter().map(|c| c.tokens).collect::<Vec<_>>());
    }

    #[test]
    fn title_falls_back_to_file_name_and_slug() {
        let p = parse("a/My Note.md", "just text", Kind::Markdown, 400);
        assert_eq!(p.title, "My Note");
        assert_eq!(slug("Как собрать: Windows!"), "как-собрать-windows");
        assert_eq!(link_key("Folder/My Note.md"), "my note");
    }
}

//! Light, streaming-friendly markdown → styled lines renderer.
//! Supports headings, bold, italic, inline code, links, fenced code blocks, lists, quotes, rules.
use super::{spans_width, wrap};
use crate::theme::{self, t};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render(src: &str, width: usize) -> Vec<Line<'static>> {
    render_with(src, width, theme::text())
}

/// Render markdown with `base` as the paragraph style.
pub fn render_with(src: &str, width: usize, base: Style) -> Vec<Line<'static>> {
    let th = t();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    let code_style = Style::default().fg(th.text).bg(th.code_bg);
    let bar = Span::styled("│ ", Style::default().fg(th.border).bg(th.code_bg));
    let mut last_blank = true;

    for raw in src.split('\n') {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if !in_code {
                in_code = true;
                let lang = trimmed.trim_start_matches(['`', '~']).trim();
                let label = if lang.is_empty() { String::new() } else { lang.to_string() };
                out.extend(wrap(
                    &[Span::styled(label, Style::default().fg(th.muted).bg(th.code_bg))],
                    width,
                    &[bar.clone()],
                    &[bar.clone()],
                    Some(code_style),
                ));
            } else {
                in_code = false;
            }
            last_blank = false;
            continue;
        }
        if in_code {
            out.extend(wrap(&[Span::styled(line.to_string(), code_style)], width, &[bar.clone()], &[bar.clone()], Some(code_style)));
            last_blank = false;
            continue;
        }

        if trimmed.is_empty() {
            if !last_blank {
                out.push(Line::default());
            }
            last_blank = true;
            continue;
        }
        last_blank = false;

        // Headings
        if let Some((level, rest)) = heading(trimmed) {
            let st = if level <= 2 {
                theme::bold(theme::accent())
            } else {
                theme::bold(base)
            };
            out.extend(wrap(&inline(rest, st), width, &[], &[], None));
            continue;
        }
        // Horizontal rule
        if is_rule(trimmed) {
            out.push(Line::from(Span::styled("─".repeat(width.min(60)), theme::muted())));
            continue;
        }
        // Block quote
        if let Some(rest) = trimmed.strip_prefix('>') {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            let q = Span::styled("│ ", theme::muted());
            let st = theme::muted().add_modifier(Modifier::ITALIC);
            out.extend(wrap(&inline(rest, st), width, &[q.clone()], &[q], None));
            continue;
        }
        // List items
        let indent = line.len() - trimmed.len();
        if let Some((marker, rest)) = list_item(trimmed) {
            let pad = " ".repeat((indent / 2) * 2);
            let first = vec![Span::raw(pad.clone()), Span::styled(format!("{marker} "), theme::accent())];
            let w = spans_width(&first);
            let cont = vec![Span::raw(" ".repeat(w))];
            out.extend(wrap(&inline(rest, base), width, &first, &cont, None));
            continue;
        }
        out.extend(wrap(&inline(trimmed, base), width, &[], &[], None));
    }
    while out.last().map(|l| l.spans.is_empty()).unwrap_or(false) {
        out.pop();
    }
    out
}

fn heading(s: &str) -> Option<(usize, &str)> {
    let n = s.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&n) && s[n..].starts_with(' ') {
        Some((n, s[n..].trim()))
    } else {
        None
    }
}

fn is_rule(s: &str) -> bool {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    s.len() >= 3 && (s.chars().all(|c| c == '-') || s.chars().all(|c| c == '*') || s.chars().all(|c| c == '_'))
}

fn list_item(s: &str) -> Option<(String, &str)> {
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(m) {
            return Some(("•".into(), rest));
        }
    }
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits < 4 {
        let rest = &s[digits..];
        if rest.starts_with(". ") || rest.starts_with(") ") {
            return Some((s[..digits + 1].to_string(), &rest[2..]));
        }
    }
    None
}

/// Parse inline markdown: **bold**, *italic*, `code`, [text](url).
pub fn inline(s: &str, base: Style) -> Vec<Span<'static>> {
    let th = t();
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut bold = false;
    let mut italic = false;
    let style = |bold: bool, italic: bool| {
        let mut st = base;
        if bold {
            st = st.add_modifier(Modifier::BOLD);
        }
        if italic {
            st = st.add_modifier(Modifier::ITALIC);
        }
        st
    };
    let flush = |out: &mut Vec<Span<'static>>, buf: &mut String, st: Style| {
        if !buf.is_empty() {
            out.push(Span::styled(std::mem::take(buf), st));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(j) = (i + 1..chars.len()).find(|&j| chars[j] == '`') {
                flush(&mut out, &mut buf, style(bold, italic));
                let code: String = chars[i + 1..j].iter().collect();
                out.push(Span::styled(code, Style::default().fg(th.text).bg(th.code_bg)));
                i = j + 1;
                continue;
            }
        }
        if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            let closing = bold || find_seq(&chars, i + 2, "**").is_some();
            if closing {
                flush(&mut out, &mut buf, style(bold, italic));
                bold = !bold;
            } else {
                buf.push_str("**");
            }
            i += 2;
            continue;
        }
        if c == '*' {
            let opens = !italic
                && i + 1 < chars.len()
                && !chars[i + 1].is_whitespace()
                && (i + 1..chars.len()).any(|j| chars[j] == '*' && !chars[j - 1].is_whitespace());
            let closes = italic && i > 0 && !chars[i - 1].is_whitespace();
            if opens || closes {
                flush(&mut out, &mut buf, style(bold, italic));
                italic = !italic;
                i += 1;
                continue;
            }
        }
        if c == '[' {
            if let Some(close) = (i + 1..chars.len()).find(|&j| chars[j] == ']') {
                if close + 1 < chars.len() && chars[close + 1] == '(' {
                    if let Some(end) = (close + 2..chars.len()).find(|&j| chars[j] == ')') {
                        flush(&mut out, &mut buf, style(bold, italic));
                        let text: String = chars[i + 1..close].iter().collect();
                        out.push(Span::styled(text, Style::default().fg(th.accent).add_modifier(Modifier::UNDERLINED)));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut out, &mut buf, style(bold, italic));
    out
}

fn find_seq(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    (from..chars.len().saturating_sub(p.len() - 1)).find(|&i| chars[i..i + p.len()] == p[..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    fn texts(v: &[Line]) -> Vec<String> {
        v.iter().map(plain).collect()
    }

    #[test]
    fn bold_and_inline_code() {
        let spans = inline("use **cargo** and `fmt` now", theme::text());
        let parts: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(parts, vec!["use ", "cargo", " and ", "fmt", " now"]);
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[3].style.bg, Some(t().code_bg));
        assert!(!spans[4].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn unmatched_markers_are_literal() {
        let spans = inline("a * b and **x", theme::text());
        let s: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(s, "a * b and **x");
    }

    #[test]
    fn italic_and_link() {
        let spans = inline("*hi* see [docs](http://x)", theme::text());
        assert_eq!(spans[0].content, "hi");
        assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(spans.last().unwrap().content, "docs");
    }

    #[test]
    fn headings_lists_and_code() {
        let md = "# Title\n\nSome text\n\n- one\n- two\n1. first\n\n```rust\nfn main() {}\n```\nafter";
        let lines = render(md, 40);
        let t = texts(&lines);
        assert_eq!(t[0], "Title");
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(t[1], "");
        assert_eq!(t[2], "Some text");
        assert_eq!(t[4], "• one");
        assert_eq!(t[5], "• two");
        assert_eq!(t[6], "1. first");
        assert!(t[8].starts_with("│ rust"));
        assert!(t[9].starts_with("│ fn main() {}"));
        // code lines padded to full width with background
        assert_eq!(unicode_width::UnicodeWidthStr::width(t[9].as_str()), 40);
        assert_eq!(t[10], "after");
    }

    #[test]
    fn list_items_hang_indent() {
        let lines = render("- alpha beta gamma delta", 12);
        let t = texts(&lines);
        assert_eq!(t, vec!["• alpha beta", "  gamma", "  delta"]);
    }

    #[test]
    fn unclosed_fence_streams_as_code() {
        let lines = render("```\nlet x = 1;", 20);
        assert!(texts(&lines)[1].starts_with("│ let x = 1;"));
    }

    #[test]
    fn collapses_blank_lines() {
        let lines = render("a\n\n\n\nb\n\n", 20);
        assert_eq!(texts(&lines), vec!["a", "", "b"]);
    }
}

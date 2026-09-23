//! Text layout helpers: word wrapping of styled spans with hanging prefixes.
pub mod markdown;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn cw(c: char) -> usize {
    if c == '\t' {
        4
    } else {
        c.width().unwrap_or(0)
    }
}

pub fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Word-wrap `spans` into lines of at most `width` columns. The first line is prefixed by `first`,
/// continuation lines by `rest`. With `fill`, each line is padded to full width using that style
/// (used for code-block backgrounds).
pub fn wrap(
    spans: &[Span<'static>],
    width: usize,
    first: &[Span<'static>],
    rest: &[Span<'static>],
    fill: Option<Style>,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut prefix = first;
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut cur_w = 0usize;
    let mut last_space: Option<usize> = None;

    let emit = |out: &mut Vec<Line<'static>>, prefix: &[Span<'static>], chars: &[(char, Style)]| {
        let avail = width.saturating_sub(spans_width(prefix)).max(1);
        let mut v: Vec<Span<'static>> = prefix.to_vec();
        let mut buf = String::new();
        let mut st: Option<Style> = None;
        let mut w = 0;
        for &(c, s) in chars {
            if st != Some(s) {
                if let Some(ps) = st {
                    if !buf.is_empty() {
                        v.push(Span::styled(std::mem::take(&mut buf), ps));
                    }
                }
                st = Some(s);
            }
            buf.push(if c == '\t' { ' ' } else { c });
            if c == '\t' {
                buf.push_str("   ");
            }
            w += cw(c);
        }
        if let Some(ps) = st {
            if !buf.is_empty() {
                v.push(Span::styled(buf, ps));
            }
        }
        if let Some(f) = fill {
            if w < avail {
                v.push(Span::styled(" ".repeat(avail - w), f));
            }
        }
        out.push(Line::from(v));
    };

    let mut chars: Vec<(char, Style)> = Vec::new();
    for s in spans {
        for c in s.content.chars() {
            if c == '\n' || c == '\r' {
                continue;
            }
            chars.push((c, s.style));
        }
    }

    for (c, s) in chars {
        let avail = width.saturating_sub(spans_width(prefix)).max(1);
        let w = cw(c);
        if cur_w + w > avail && !cur.is_empty() {
            let rest_chars = match last_space {
                _ if c == ' ' => Vec::new(),
                Some(i) => cur.split_off(i + 1),
                None => Vec::new(),
            };
            while cur.last().map(|x| x.0 == ' ').unwrap_or(false) {
                cur.pop();
            }
            emit(&mut out, prefix, &cur);
            prefix = rest;
            cur = rest_chars;
            cur_w = cur.iter().map(|x| cw(x.0)).sum();
            last_space = None;
            if c == ' ' && cur.is_empty() {
                continue;
            }
        }
        if c == ' ' {
            last_space = Some(cur.len());
        }
        cur.push((c, s));
        cur_w += w;
    }
    if !cur.is_empty() || out.is_empty() {
        emit(&mut out, prefix, &cur);
    }
    out
}

/// Truncate a plain string to `max` display columns, adding an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cwid = cw(c);
        if w + cwid + 1 > max {
            break;
        }
        out.push(c);
        w += cwid;
    }
    out.push('…');
    out
}

/// "1.2k", "81.2k", "950", "1.3M"
pub fn fmt_tok(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// "0.8s", "12.3s", "4m 03s", "1h 02m"
pub fn fmt_ms(ms: u64) -> String {
    let s = ms / 1000;
    if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// "00:42", "12:03", "1:02:03"
pub fn fmt_clock(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn wraps_on_words() {
        let lines = wrap(&[Span::raw("hello world foo bar")], 11, &[], &[], None);
        let v: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(v, vec!["hello world", "foo bar"]);
    }

    #[test]
    fn hard_breaks_long_words_and_prefixes() {
        let lines = wrap(&[Span::raw("abcdefghij")], 6, &[Span::raw("- ")], &[Span::raw("  ")], None);
        let v: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(v, vec!["- abcd", "  efgh", "  ij"]);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_tok(41234), "41.2k");
        assert_eq!(fmt_tok(950), "950");
        assert_eq!(fmt_ms(12300), "12.3s");
        assert_eq!(fmt_clock(42), "00:42");
    }
}

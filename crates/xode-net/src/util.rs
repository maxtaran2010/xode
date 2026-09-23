//! Small shared helpers: token capping, trimming.

use xode_core::tokens;

/// Trim to at most `n` chars, appending `…` when cut. Whitespace is collapsed.
pub fn clip(s: &str, n: usize) -> String {
    let s = collapse_ws(s);
    if s.chars().count() <= n {
        return s;
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cut `s` to roughly `max_tokens` keeping head (~60%) and tail (~40%) lines,
/// inserting a `[N tokens omitted]` marker.
pub fn cap_tokens(s: &str, max_tokens: u64) -> String {
    let max_tokens = max_tokens.max(50);
    let total = tokens::count(s);
    if total <= max_tokens {
        return s.to_string();
    }
    // bytes per token for this text, used to translate budgets to byte lengths
    let bpt = (s.len() as f64 / total.max(1) as f64).max(1.0);
    let budget_bytes = ((max_tokens.saturating_sub(12)) as f64 * bpt) as usize;
    let head_b = budget_bytes * 6 / 10;
    let tail_b = budget_bytes - head_b;
    let head = take_head(s, head_b);
    let tail = take_tail(&s[head.len()..], tail_b);
    let mid = &s[head.len()..s.len() - tail.len()];
    let omitted = tokens::count(mid);
    format!(
        "{}\n[{} tokens omitted]\n{}",
        head.trim_end(),
        omitted,
        tail.trim_start()
    )
}

/// Longest prefix within `max` bytes, preferring to end at a line boundary.
fn take_head(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let cut = floor_char(s, max);
    let pre = &s[..cut];
    match pre.rfind('\n') {
        Some(i) if i > cut / 2 => &s[..i + 1],
        _ => pre,
    }
}

/// Longest suffix within `max` bytes, preferring to start at a line boundary.
fn take_tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let start = ceil_char(s, s.len() - max);
    let suf = &s[start..];
    match suf.find('\n') {
        Some(i) if i < suf.len() / 2 => &suf[i + 1..],
        _ => suf,
    }
}

pub fn floor_char(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_keeps_small() {
        assert_eq!(cap_tokens("hello world", 100), "hello world");
    }

    #[test]
    fn cap_head_tail() {
        let s: String = (0..2000).map(|i| format!("line number {i} with some words\n")).collect();
        let c = cap_tokens(&s, 300);
        assert!(c.contains("tokens omitted]"));
        assert!(c.starts_with("line number 0 "));
        assert!(c.trim_end().ends_with("line number 1999 with some words"));
        assert!(tokens::count(&c) < 400, "{}", tokens::count(&c));
    }

    #[test]
    fn clip_works() {
        assert_eq!(clip("a  b\n c", 10), "a b c");
        assert_eq!(clip("abcdef", 4), "abc…");
    }
}

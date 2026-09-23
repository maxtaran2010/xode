//! Chat transcript rendering with a per-item line cache and scroll/hit-testing.
use crate::app::App;
use crate::render::{fmt_ms, fmt_tok, markdown, truncate, wrap};
use crate::theme::{self, t};
use crate::view::{est_tokens, tool_summary, Item, Level, ToolState};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::Instant;

const RESULT_LINES: usize = 20;

#[derive(Default)]
pub struct ChatCache {
    width: usize,
    entries: Vec<Option<CacheEntry>>,
}

struct CacheEntry {
    rev: u64,
    focused: bool,
    lines: Vec<Line<'static>>,
}

impl ChatCache {
    pub fn reset(&mut self) {
        self.entries.clear();
    }
}

/// Is this item animated (must be re-rendered every frame)?
fn live(item: &Item) -> bool {
    matches!(item, Item::Tool { state: ToolState::Running, .. } | Item::Thinking { dur_ms: None, .. } | Item::Compaction { done: false, .. })
}

/// Blank lines inserted before `cur`.
fn gap(prev: Option<&Item>, cur: &Item) -> usize {
    match (prev, cur) {
        (None, _) => 0,
        (Some(Item::Tool { .. }), Item::Tool { .. }) => 0,
        (Some(Item::Thinking { .. }), Item::Tool { .. }) => 0,
        (Some(Item::Thinking { .. }), Item::Thinking { .. }) => 0,
        (Some(Item::Tool { .. }), Item::Thinking { .. }) => 0,
        (Some(_), Item::Meta(_)) => 0,
        _ => 1,
    }
}

fn divider(label: &str, width: usize, style: Style) -> Line<'static> {
    let label = format!(" {} ", truncate(label, width.saturating_sub(8)));
    let lw = unicode_width::UnicodeWidthStr::width(label.as_str());
    let side = width.saturating_sub(lw) / 2;
    let left = side.min(20).max(2);
    Line::from(vec![Span::styled("─".repeat(left), style), Span::styled(label, style), Span::styled("─".repeat(left), style)])
}

pub fn meta_line(m: &xode_engine::xode_core::TurnMeta) -> String {
    let mut parts = vec![fmt_ms(m.duration_ms), format!("{} in", fmt_tok(m.prompt_tokens)), format!("{} out", fmt_tok(m.completion_tokens))];
    if m.decode_tps > 0.0 {
        parts.push(format!("{:.0} tok/s", m.decode_tps));
    }
    if m.ttft_ms > 0 {
        parts.push(format!("ttft {}", fmt_ms(m.ttft_ms)));
    }
    parts.join(" · ")
}

/// opencode-style layout: user messages as tinted blocks with an accent bar, everything else indented.
pub fn render_item(item: &Item, width: usize, focused: bool, now: Instant, spin: &str) -> Vec<Line<'static>> {
    let th = t();
    match item {
        Item::User { text, pending } => {
            let bg = th.block_bg;
            let st = if *pending { theme::muted() } else { theme::text() }.bg(bg);
            let bar = Span::styled("┃  ", Style::default().fg(th.accent).bg(bg));
            let inner = width.saturating_sub(4);
            let pad_line = |spans: Vec<Span<'static>>| -> Line<'static> {
                let used: usize = spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
                let mut spans = spans;
                spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), Style::default().bg(bg)));
                Line::from(spans)
            };
            let mut out = vec![pad_line(vec![Span::styled("┃", Style::default().fg(th.accent).bg(bg))])];
            for l in text.split('\n') {
                for wl in wrap(&[Span::styled(l.to_string(), st)], inner, &[], &[], None) {
                    let mut spans = vec![bar.clone()];
                    spans.extend(wl.spans);
                    out.push(pad_line(spans));
                }
            }
            out.push(pad_line(vec![Span::styled("┃", Style::default().fg(th.accent).bg(bg))]));
            out
        }
        Item::Meta(m) => {
            let model = if m.model.is_empty() { "model".to_string() } else { m.model.clone() };
            vec![Line::from(vec![
                Span::raw("  "),
                Span::styled("▣ ", theme::accent()),
                Span::styled(model, theme::text()),
                Span::styled(format!(" · {}", meta_line(m)), theme::muted()),
            ])]
        }
        _ => render_inner(item, width.saturating_sub(2), focused, now, spin)
            .into_iter()
            .map(|mut l| {
                l.spans.insert(0, Span::raw("  "));
                l
            })
            .collect(),
    }
}

fn render_inner(item: &Item, width: usize, focused: bool, now: Instant, spin: &str) -> Vec<Line<'static>> {
    let th = t();
    let focus_bg = |mut l: Line<'static>| {
        if focused {
            for s in l.spans.iter_mut() {
                s.style = s.style.bg(th.sel_bg);
            }
        }
        l
    };
    match item {
        Item::User { text, pending } => {
            let st = if *pending { theme::muted() } else { theme::text() };
            let bar = Span::styled("› ", theme::accent());
            let cont = Span::raw("  ");
            let mut out = Vec::new();
            for (i, l) in text.split('\n').enumerate() {
                let first = if i == 0 { bar.clone() } else { cont.clone() };
                out.extend(wrap(&[Span::styled(l.to_string(), st)], width, &[first], &[cont.clone()], None));
            }
            out
        }
        Item::Text { text, .. } => markdown::render(text, width),
        Item::Thinking { text, started, dur_ms, expanded, .. } => {
            let st = Style::default().fg(th.thinking);
            let dur = match (dur_ms, started) {
                (Some(d), _) => *d,
                (None, Some(s)) => now.duration_since(*s).as_millis() as u64,
                _ => 0,
            };
            let mut meta = format!("{} tok", fmt_tok(est_tokens(text)));
            if dur > 0 {
                meta.push_str(&format!(", {}", fmt_ms(dur).replace(".0s", "s")));
            }
            let arrow = if *expanded { "▾ " } else { "▸ " };
            let mut out = vec![focus_bg(Line::from(vec![
                Span::styled(arrow, st),
                Span::styled("Thinking", st.add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({meta})"), st),
            ]))];
            if *expanded {
                let bar = Span::styled("  │ ", Style::default().fg(th.border));
                let body = st.add_modifier(Modifier::ITALIC);
                for l in text.trim().split('\n') {
                    out.extend(wrap(&[Span::styled(l.to_string(), body)], width, &[bar.clone()], &[bar.clone()], None));
                }
            }
            out
        }
        Item::Tool { name, args, state, ms, result, expanded, .. } => {
            let (sym, sym_st) = match state {
                ToolState::Running => (spin.to_string(), theme::accent()),
                ToolState::Ok => ("✓".to_string(), Style::default().fg(th.ok)),
                ToolState::Err => ("✗".to_string(), Style::default().fg(th.err)),
            };
            let summary = tool_summary(name, args);
            let detail = summary.strip_prefix(name.as_str()).unwrap_or("").trim().to_string();
            let time = if *ms > 0 { format!("  {}", fmt_dur_short(*ms)) } else { String::new() };
            let avail = width.saturating_sub(4 + name.chars().count() + time.chars().count());
            let mut spans = vec![
                Span::styled("● ", Style::default().fg(th.border)),
                Span::styled(name.clone(), theme::bold(theme::text())),
            ];
            if !detail.is_empty() {
                spans.push(Span::styled(format!(" {}", truncate(&detail, avail.max(4))), theme::muted()));
            }
            spans.push(Span::styled(time, theme::muted()));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(sym, sym_st));
            let mut out = vec![focus_bg(Line::from(spans))];
            if *expanded {
                let bar = Span::styled("  │ ", Style::default().fg(th.border));
                let body = if *state == ToolState::Err { Style::default().fg(th.err) } else { theme::muted() };
                let lines: Vec<&str> = result.lines().collect();
                if lines.is_empty() {
                    out.push(Line::from(vec![bar.clone(), Span::styled("(no output)", theme::muted())]));
                }
                for l in lines.iter().take(RESULT_LINES) {
                    out.push(Line::from(vec![bar.clone(), Span::styled(truncate(&l.replace('\t', "    "), width.saturating_sub(4)), body)]));
                }
                if lines.len() > RESULT_LINES {
                    out.push(Line::from(vec![bar, Span::styled(format!("… {} more lines", lines.len() - RESULT_LINES), theme::muted())]));
                }
            }
            out
        }
        Item::Compaction { before, after, done } => {
            if !done {
                vec![divider(&format!("{spin} compacting context…"), width, Style::default().fg(th.warn))]
            } else {
                let label = match (before, after) {
                    (Some(b), Some(a)) => format!("context compacted {} → {}", fmt_tok(*b), fmt_tok(*a)),
                    _ => "context compacted".into(),
                };
                vec![divider(&label, width, theme::muted())]
            }
        }
        Item::Goal { text } => {
            let first = text.lines().next().unwrap_or("").trim();
            let label = if first.is_empty() { "goal · continuing".to_string() } else { format!("goal · {first}") };
            vec![divider(&label, width, theme::muted().add_modifier(Modifier::DIM))]
        }
        Item::Meta(m) => vec![Line::from(Span::styled(meta_line(m), theme::muted().add_modifier(Modifier::DIM)))],
        Item::Notice { text, level } => {
            let st = match level {
                Level::Info => theme::muted(),
                Level::Warn => Style::default().fg(th.warn),
                Level::Error => Style::default().fg(th.err),
            };
            let mut out = Vec::new();
            for l in text.split('\n') {
                out.extend(wrap(&[Span::styled(l.to_string(), st)], width, &[], &[], None));
            }
            out
        }
    }
}

fn fmt_dur_short(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        fmt_ms(ms)
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    app.chat_area = area;
    let width = area.width as usize;
    let h = area.height as usize;
    let now = Instant::now();
    let spin = super::spinner(app.started);

    let cache = &mut app.chat_cache;
    if cache.width != width {
        cache.width = width;
        cache.entries.clear();
    }
    let items = &app.view.items;
    cache.entries.truncate(items.len());
    cache.entries.resize_with(items.len(), || None);

    // Heights (gap + lines) per item, refreshing stale cache entries.
    let mut heights = Vec::with_capacity(items.len());
    let mut live_lines: Vec<Option<Vec<Line<'static>>>> = vec![None; items.len()];
    for (i, e) in items.iter().enumerate() {
        let focused = app.focus == Some(i) && app.focus_in_chat;
        let g = gap(i.checked_sub(1).map(|p| &items[p].item), &e.item);
        let n = if live(&e.item) {
            let l = render_item(&e.item, width, focused, now, spin);
            let n = l.len();
            live_lines[i] = Some(l);
            n
        } else {
            let stale = match &cache.entries[i] {
                Some(c) => c.rev != e.rev || c.focused != focused,
                None => true,
            };
            if stale {
                cache.entries[i] = Some(CacheEntry { rev: e.rev, focused, lines: render_item(&e.item, width, focused, now, spin) });
            }
            cache.entries[i].as_ref().map(|c| c.lines.len()).unwrap_or(0)
        };
        heights.push(g + n);
    }
    let total: usize = heights.iter().sum();

    // Keep the viewport stable when scrolled up and new content arrives.
    if app.scroll > 0 && total > app.last_total {
        app.scroll += total - app.last_total;
    }
    app.last_total = total;
    let max_scroll = total.saturating_sub(h);
    // Reveal the focused item.
    if let Some(idx) = app.reveal.take() {
        let start: usize = heights[..idx.min(heights.len())].iter().sum();
        let end = start + heights.get(idx).copied().unwrap_or(0);
        let top = total.saturating_sub(h + app.scroll.min(max_scroll));
        if start < top {
            app.scroll = total.saturating_sub(h + start);
        } else if end > top + h {
            app.scroll = total.saturating_sub(end.max(h));
        }
    }
    app.scroll = app.scroll.min(max_scroll);
    let top = total.saturating_sub(h + app.scroll);

    // Collect visible lines + row → item map.
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(h);
    let mut rows: Vec<Option<usize>> = Vec::with_capacity(h);
    let mut y = 0usize;
    for (i, e) in items.iter().enumerate() {
        let ih = heights[i];
        if y + ih <= top {
            y += ih;
            continue;
        }
        if y >= top + h {
            break;
        }
        let g = gap(i.checked_sub(1).map(|p| &items[p].item), &e.item);
        let item_lines: &[Line<'static>] = match &live_lines[i] {
            Some(l) => l,
            None => cache.entries[i].as_ref().map(|c| c.lines.as_slice()).unwrap_or(&[]),
        };
        for k in 0..ih {
            let row = y + k;
            if row < top || row >= top + h {
                continue;
            }
            if k < g {
                lines.push(Line::default());
                rows.push(None);
            } else {
                lines.push(item_lines[k - g].clone());
                rows.push(Some(i));
            }
        }
        y += ih;
    }
    app.chat_rows = rows;
    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plain(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn tool_and_thinking_lines() {
        let now = Instant::now();
        let tool = Item::Tool {
            call_id: "c".into(),
            name: "read".into(),
            args: json!({"path": "src/main.rs"}),
            state: ToolState::Ok,
            ms: 12,
            result: (0..30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"),
            expanded: false,
            started: None,
        };
        let l = render_item(&tool, 60, false, now, "⠋");
        assert_eq!(l.len(), 1);
        assert_eq!(plain(&l[0]), "  ● read src/main.rs  12ms ✓");
        let mut open = tool.clone();
        if let Item::Tool { expanded, .. } = &mut open {
            *expanded = true;
        }
        let l = render_item(&open, 60, false, now, "⠋");
        assert_eq!(l.len(), 1 + RESULT_LINES + 1);
        assert_eq!(plain(l.last().unwrap()), "    │ … 10 more lines");

        let th = Item::Thinking { turn: "t".into(), text: "x".repeat(4800), started: None, dur_ms: Some(14_000), expanded: false };
        assert_eq!(plain(&render_item(&th, 60, false, now, "")[0]), "  ▸ Thinking (1.2k tok, 14s)");

        let c = Item::Compaction { before: Some(81_200), after: Some(6_400), done: true };
        assert!(plain(&render_item(&c, 60, false, now, "")[0]).contains("context compacted 81.2k → 6.4k"));
    }

    #[test]
    fn meta_format() {
        let m = xode_engine::xode_core::TurnMeta {
            duration_ms: 12_300,
            prompt_tokens: 1200,
            completion_tokens: 480,
            decode_tps: 42.0,
            ttft_ms: 800,
            ..Default::default()
        };
        assert_eq!(meta_line(&m), "12.3s · 1.2k in · 480 out · 42 tok/s · ttft 0.8s");
    }
}

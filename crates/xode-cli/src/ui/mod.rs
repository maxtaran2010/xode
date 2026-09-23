//! Top-level layout: chat | panel, working/permission line, input, status line, overlays.
pub mod chat;
pub mod context;
pub mod input;
pub mod panel;
pub mod popup;

use crate::app::{App, Overlay};
use crate::view::Activity;
use crate::render::{fmt_clock, fmt_tok, truncate};
use crate::theme::{self, t};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::time::Instant;
use xode_engine::xode_core::Mode;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(since: Instant) -> &'static str {
    SPINNER[(since.elapsed().as_millis() / 80) as usize % SPINNER.len()]
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let th = t();
    f.render_widget(Block::default().style(Style::default().bg(th.bg)), area);

    if let Overlay::Context(st) = &mut app.overlay {
        context::draw(f, st, area);
        return;
    }

    // Sidebar spans the full height, like opencode.
    let show_panel = app.show_panel && area.width >= panel::WIDTH + 50;
    let (left, panel_area) = if show_panel {
        let [l, p] = Layout::horizontal([Constraint::Min(30), Constraint::Length(panel::WIDTH)]).areas(area);
        (l, Some(p))
    } else {
        (area, None)
    };
    let left = Rect { x: left.x + 1, width: left.width.saturating_sub(2), ..left };

    let input_h = app.input.height(left.width);
    let extra = if !app.perms.is_empty() || app.view.running { 1 } else { 0 };
    let queue_h = app.view.queue.len().min(4) as u16;
    let [_, main, _, queue_area, work, input_area, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(queue_h),
        Constraint::Length(extra),
        Constraint::Length(input_h),
        Constraint::Length(1),
    ])
    .areas(left);

    if app.view.items.is_empty() {
        draw_empty(f, app, main);
        app.chat_area = main;
        app.chat_rows.clear();
    } else {
        chat::draw(f, app, main);
    }
    if let Some(p) = panel_area {
        panel::draw(f, app, p);
    }
    if queue_h > 0 {
        draw_queue(f, app, queue_area);
    }
    if extra > 0 {
        draw_work_line(f, app, work);
    }
    let input_active = matches!(app.overlay, Overlay::None) && app.perms.is_empty() && !app.focus_in_chat;
    let footer = input_footer(app);
    input::draw(f, &app.input, input_area, input_active, footer);
    draw_status(f, app, status);

    if let Some(p) = &app.popup {
        popup::draw(f, p, input_area, main);
    }
    if let Overlay::Sessions { list, sel } = &app.overlay {
        draw_sessions(f, list, *sel, area);
    }
}

fn draw_empty(f: &mut Frame, app: &App, area: Rect) {
    let th = t();
    let mut lines = panel::logo_lines(th.bg);
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("What should we work on in ", theme::muted()),
        Span::styled(app.project.name.clone(), theme::text()),
        Span::styled("?", theme::muted()),
    ]));
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let r = Rect { y, height: h.min(area.height), ..area };
    f.render_widget(Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center), r);
}

fn input_footer(app: &App) -> Line<'static> {
    let th = t();
    let bg = th.block_bg;
    let (mode, mode_st) = match app.session.mode {
        Mode::Normal => ("Normal", Style::default().fg(th.accent).bg(bg)),
        Mode::Plan => ("Plan", Style::default().fg(th.warn).bg(bg)),
    };
    let model = if app.view.stats.model.is_empty() { app.session.model.clone() } else { app.view.stats.model.clone() };
    let gw_id = if app.view.stats.gateway.is_empty() { app.session.gateway.clone() } else { app.view.stats.gateway.clone() };
    let gw = app.gateway_name(&gw_id).to_string();
    let mut spans = vec![
        Span::styled(mode, mode_st),
        Span::styled(" · ", Style::default().fg(th.muted).bg(bg)),
        Span::styled(if model.is_empty() { "no model".to_string() } else { model }, theme::text().bg(bg)),
        Span::styled(format!("  {gw}"), theme::muted().bg(bg)),
    ];
    if !app.effort.is_empty() {
        spans.push(Span::styled(" · ", Style::default().fg(th.muted).bg(bg)));
        spans.push(Span::styled(format!("{} effort", app.effort), Style::default().fg(th.thinking).bg(bg)));
    }
    Line::from(spans)
}

/// Queued (steer) messages: delivered after the next tool call, or now with ctrl+s.
fn draw_queue(f: &mut Frame, app: &App, area: Rect) {
    let th = t();
    let w = area.width as usize;
    let lines: Vec<Line> = app
        .view
        .queue
        .iter()
        .take(area.height as usize)
        .enumerate()
        .map(|(i, q)| {
            let right = if i == 0 { "steer ctrl+s" } else { "" };
            let text = truncate(&q.text.replace('\n', " "), w.saturating_sub(right.len() + 6));
            let pad = w.saturating_sub(3 + unicode_width::UnicodeWidthStr::width(text.as_str()) + right.len() + 1);
            Line::from(vec![
                Span::styled(" ↳ ", theme::muted()),
                Span::styled(text, Style::default().fg(if q.command { th.accent } else { th.thinking })),
                Span::raw(" ".repeat(pad)),
                Span::styled(right, theme::accent()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_work_line(f: &mut Frame, app: &App, area: Rect) {
    let th = t();
    if let Some(p) = app.perms.front() {
        let key = |k: &str| Span::styled(format!("[{k}]"), theme::bold(theme::accent()));
        let line = Line::from(vec![
            Span::styled(" Allow ", Style::default().fg(th.warn)),
            Span::styled(format!("{}: ", p.tool), theme::bold(Style::default().fg(th.warn))),
            Span::styled(truncate(&p.summary, (area.width as usize).saturating_sub(48)), theme::text()),
            Span::styled("?  ", Style::default().fg(th.warn)),
            key("y"),
            Span::styled(" once  ", theme::muted()),
            key("a"),
            Span::styled(" always  ", theme::muted()),
            key("n"),
            Span::styled(" deny", theme::muted()),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    let v = &app.view;
    let secs = v.run_start.map(|s| s.elapsed().as_secs()).unwrap_or(0);
    let step = if v.stats.steps > 0 { v.stats.steps } else { v.turns_this_run };
    let label = match v.activity.label() {
        s if s.is_empty() => "Working…".to_string(),
        s => s,
    };
    let mut spans = vec![
        Span::styled(format!(" {} ", spinner(app.started)), theme::accent()),
        Span::styled(label, Style::default().fg(th.thinking)),
    ];
    if let (Activity::Compacting, Some((stage, tokens, expected))) = (&v.activity, &v.compact) {
        spans.extend(compact_progress(stage, *tokens, *expected));
    }
    spans.push(Span::styled(format!(" {}", fmt_clock(secs)), theme::muted()));
    spans.push(Span::styled(if step > 0 { format!(" · step {step}") } else { String::new() }, theme::muted()));
    let line = Line::from(spans);
    f.render_widget(Paragraph::new(line), area);
}

/// `reading context ━━━━──── 120/400` — overall compaction progress by stage.
fn compact_progress(stage: &str, tokens: u64, expected: u64) -> Vec<Span<'static>> {
    let (name, r) = match stage {
        "prefill" => ("reading context", 0.05),
        "handoff" => ("writing handoff", 0.1 + 0.75 * (tokens as f64 / expected.max(1) as f64).min(1.0)),
        "path" => ("updating PATH.md", 0.9),
        _ => ("fresh context", 0.96),
    };
    let w = 12usize;
    let filled = ((r * w as f64).round() as usize).min(w);
    let th = t();
    let mut v = vec![
        Span::styled(format!(" {name} "), theme::muted()),
        Span::styled("━".repeat(filled), Style::default().fg(th.warn)),
        Span::styled("─".repeat(w - filled), Style::default().fg(th.border)),
    ];
    if stage == "handoff" {
        v.push(Span::styled(format!(" {tokens} tok"), theme::muted()));
    }
    v
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let th = t();
    let path = panel::truncate_left(&app.project.root.replace('\\', "/"), (area.width as usize / 2).max(10));
    let mut left = vec![Span::styled(path, theme::muted())];
    if let Some(goal) = app.session.goal.as_deref().filter(|g| !g.trim().is_empty()) {
        left.push(Span::styled("  goal ", theme::accent()));
        left.push(Span::styled(truncate(goal, 40), theme::muted()));
    }
    let s = &app.view.stats;
    let mut right: Vec<Span> = vec![];
    if let Some((msg, at)) = &app.flash {
        if at.elapsed().as_secs() < 3 {
            right.push(Span::styled(msg.clone(), Style::default().fg(th.warn)));
            right.push(Span::raw("  "));
        }
    }
    if s.context_limit > 0 {
        let r = s.context_used as f64 / s.context_limit as f64;
        right.push(Span::styled(fmt_tok(s.context_used), Style::default().fg(panel::ratio_color(r))));
        right.push(Span::styled(format!(" / {}", fmt_tok(s.context_limit)), theme::muted()));
    }
    let w = |v: &[Span]| -> usize { v.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum() };
    let pad = (area.width as usize).saturating_sub(w(&left) + w(&right));
    left.push(Span::raw(" ".repeat(pad)));
    left.extend(right);
    f.render_widget(Paragraph::new(Line::from(left)), area);
}

fn draw_sessions(f: &mut Frame, list: &[xode_engine::xode_core::store::SessionInfo], sel: usize, area: Rect) {
    let th = t();
    let w = area.width.saturating_sub(8).min(90);
    let h = area.height.saturating_sub(4).min(list.len().max(1) as u16 + 2);
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height.saturating_sub(h)) / 3, width: w, height: h };
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .title(Span::styled(" Sessions ", theme::bold(theme::text())));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let iw = inner.width as usize;
    let rows = inner.height as usize;
    if list.is_empty() {
        f.render_widget(Paragraph::new(Span::styled(" no sessions", theme::muted())), inner);
        return;
    }
    let top = if sel >= rows { sel + 1 - rows } else { 0 };
    let now = chrono::Utc::now().timestamp_millis();
    let lines: Vec<Line> = list
        .iter()
        .enumerate()
        .skip(top)
        .take(rows)
        .map(|(i, s)| {
            let bg = if i == sel { Style::default().bg(th.sel_bg) } else { Style::default() };
            let when = ago(now - s.updated_at);
            let title = if s.title.trim().is_empty() { "untitled" } else { s.title.trim() };
            let tw = iw.saturating_sub(when.len() + 3);
            let title = truncate(title, tw);
            let pad = tw.saturating_sub(unicode_width::UnicodeWidthStr::width(title.as_str()));
            Line::from(vec![
                Span::styled(" ", bg),
                Span::styled(title, theme::text().patch(bg)),
                Span::styled(" ".repeat(pad + 1), bg),
                Span::styled(when, theme::muted().patch(bg)),
                Span::styled(" ", bg),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn ago(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

//! Right sidebar (opencode-style): logo, session title, context, speed, session stats, MCP.
use crate::app::App;
use crate::render::{fmt_clock, fmt_ms, fmt_tok, truncate};
use crate::theme::{self, t};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Sparkline, Wrap};
use ratatui::Frame;

pub const WIDTH: u16 = 38;

/// Block-letter logo. First 6 columns (the X) are drawn in the accent color.
pub const LOGO: [&str; 3] = ["▀▄ ▄▀ ▄▀▀▄ █▀▀▄ █▀▀▀", "  █   █  █ █  █ █▀▀ ", "▄▀ ▀▄ ▀▄▄▀ █▄▄▀ █▄▄▄"];

pub fn logo_lines(bg: Color) -> Vec<Line<'static>> {
    let th = t();
    LOGO.iter()
        .map(|l| {
            let (x, rest) = l.split_at(l.char_indices().nth(6).map(|(i, _)| i).unwrap_or(l.len()));
            Line::from(vec![
                Span::styled(x.to_string(), Style::default().fg(th.accent).bg(bg)),
                Span::styled(rest.to_string(), Style::default().fg(th.text).bg(bg)),
            ])
        })
        .collect()
}

pub fn ratio_color(r: f64) -> Color {
    let th = t();
    if r >= 0.85 {
        th.err
    } else if r >= 0.65 {
        th.warn
    } else {
        th.accent
    }
}

/// Text gauge like `━━━━━━────`.
pub fn gauge(used: u64, limit: u64, width: usize) -> Vec<Span<'static>> {
    let th = t();
    let r = if limit > 0 { (used as f64 / limit as f64).min(1.0) } else { 0.0 };
    let filled = ((r * width as f64).round() as usize).min(width);
    vec![
        Span::styled("━".repeat(filled), Style::default().fg(ratio_color(r))),
        Span::styled("─".repeat(width - filled), Style::default().fg(th.border)),
    ]
}

fn heading(s: &str) -> Line<'static> {
    Line::from(Span::styled(s.to_string(), theme::bold(theme::text())))
}

fn row(label: &str, value: String, w: usize) -> Line<'static> {
    let value = truncate(&value, w.saturating_sub(label.len() + 1));
    let pad = w.saturating_sub(label.len() + unicode_width::UnicodeWidthStr::width(value.as_str()));
    Line::from(vec![
        Span::styled(label.to_string(), theme::muted()),
        Span::raw(" ".repeat(pad)),
        Span::styled(value, theme::text()),
    ])
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let th = t();
    let block = Block::default().style(Style::default().bg(th.panel_bg)).padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let s = &app.view.stats;

    let title = if app.session.title.trim().is_empty() { "New session".to_string() } else { app.session.title.trim().to_string() };
    let title_h = ((unicode_width::UnicodeWidthStr::width(title.as_str()) / w.max(1)) + 1).min(3) as u16;

    let [logo_a, _, title_a, _, ctx_a, _, speed_head, spark_a, speed_rest, _, rest, foot] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(title_h),
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(inner);

    f.render_widget(Paragraph::new(logo_lines(th.panel_bg)), logo_a);
    f.render_widget(Paragraph::new(Span::styled(title, theme::bold(theme::text()))).wrap(Wrap { trim: true }), title_a);

    // Context
    let limit = s.context_limit;
    let pct = if limit > 0 { (s.context_used as f64 / limit as f64 * 100.0).round() as u64 } else { 0 };
    f.render_widget(
        Paragraph::new(vec![
            heading("Context"),
            Line::from(Span::styled(
                format!("{} / {} tokens", thousands(s.context_used), if limit > 0 { fmt_tok(limit) } else { "?".into() }),
                theme::muted(),
            )),
            Line::from(Span::styled(format!("{pct}% used"), theme::muted())),
            Line::from(gauge(s.context_used, limit, w)),
        ]),
        ctx_a,
    );

    // Speed
    let tps = if s.tps > 0.0 { format!("{:.1}", s.tps) } else { "0.0".into() };
    f.render_widget(
        Paragraph::new(vec![
            heading("Speed"),
            Line::from(vec![Span::styled(tps, theme::bold(theme::accent())), Span::styled(" tok/s", theme::muted())]),
        ]),
        speed_head,
    );
    let data: Vec<u64> = app.view.tps_samples.iter().copied().collect();
    let data = if data.len() > w { data[data.len() - w..].to_vec() } else { data };
    f.render_widget(Sparkline::default().data(&data).style(Style::default().fg(th.accent).bg(th.panel_bg)), spark_a);
    f.render_widget(
        Paragraph::new(vec![
            row("TTFT", if s.ttft_ms > 0 { fmt_ms(s.ttft_ms) } else { "—".into() }, w),
            row("Prefill", if s.prefill_tps > 0.0 { format!("{:.0} tok/s", s.prefill_tps) } else { "—".into() }, w),
        ]),
        speed_rest,
    );

    // Session + MCP
    let elapsed = match app.view.run_start {
        Some(st) => (st.elapsed().as_millis() as u64).max(s.elapsed_ms),
        None => s.elapsed_ms,
    };
    let mut lines = vec![
        heading("Session"),
        row("In", fmt_tok(s.tokens_in), w),
        row("Out", fmt_tok(s.tokens_out), w),
        row("Steps", s.steps.to_string(), w),
        row("Tool calls", s.tool_calls.to_string(), w),
        row("Compactions", s.compactions.to_string(), w),
        row("Elapsed", fmt_clock(elapsed / 1000), w),
        row("Total", fmt_ms(s.total_work_ms.max(app.session.total_work_ms)), w),
        row("RTK saved", fmt_tok(s.rtk_saved), w),
    ];
    if let Some(goal) = app.session.goal.as_deref().filter(|g| !g.trim().is_empty()) {
        lines.push(Line::default());
        lines.push(heading("Goal"));
        lines.push(Line::from(Span::styled(truncate(goal, w * 2), theme::muted())));
    }
    let mcp = app.engine.mcp_status();
    if let Some(arr) = mcp.as_array().filter(|a| !a.is_empty()) {
        lines.push(Line::default());
        lines.push(heading("MCP"));
        for m in arr {
            let name = m["name"].as_str().unwrap_or("?").to_string();
            let ok = m["connected"].as_bool().unwrap_or(false);
            lines.push(Line::from(vec![
                Span::styled("• ", Style::default().fg(if ok { th.ok } else { th.err })),
                Span::styled(truncate(&name, w.saturating_sub(14)), theme::text()),
                Span::styled(if ok { " Connected" } else { " Failed" }, theme::muted()),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), rest);

    // Footer: project path + version
    let path = app.project.root.replace('\\', "/");
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(truncate_left(&path, w), theme::muted())),
            Line::from(vec![
                Span::styled("• ", Style::default().fg(if app.view.running { th.accent } else { th.ok })),
                Span::styled("Xode", theme::bold(theme::text())),
                Span::styled(format!(" {}", env!("CARGO_PKG_VERSION")), theme::muted()),
            ]),
        ]),
        foot,
    );
}

/// Keep the end of a long path: `…/src/project`.
pub fn truncate_left(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - w.saturating_sub(1)).collect();
    format!("…{tail}")
}

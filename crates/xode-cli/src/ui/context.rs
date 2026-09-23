//! Full-screen context inspector built from `Engine::context_view`.
use crate::render::{fmt_tok, truncate, wrap};
use crate::theme::{self, t};
use crate::ui::panel::gauge;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use xode_engine::ContextView;

pub struct ContextState {
    pub view: ContextView,
    pub sel: usize,
    /// (title, body, scroll)
    pub detail: Option<(String, String, usize)>,
    page: usize,
}

impl ContextState {
    pub fn new(view: ContextView) -> Self {
        Self { view, sel: 0, detail: None, page: 10 }
    }

    fn len(&self) -> usize {
        self.view.sections.len() + self.view.segments.len()
    }

    /// Returns false when the view should close.
    pub fn key(&mut self, k: KeyEvent) -> bool {
        if let Some((_, _, scroll)) = &mut self.detail {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace | KeyCode::Left => self.detail = None,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(self.page),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll += self.page,
                KeyCode::Home => *scroll = 0,
                _ => {}
            }
            return true;
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return false,
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(self.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => self.open(),
            _ => {}
        }
        true
    }

    pub fn scroll(&mut self, delta: i32) {
        if let Some((_, _, s)) = &mut self.detail {
            *s = if delta < 0 { s.saturating_sub((-delta) as usize) } else { *s + delta as usize };
        } else if delta < 0 {
            self.sel = self.sel.saturating_sub(1);
        } else {
            self.sel = (self.sel + 1).min(self.len().saturating_sub(1));
        }
    }

    fn open(&mut self) {
        let n = self.view.sections.len();
        if self.sel < n {
            let s = &self.view.sections[self.sel];
            self.detail = Some((format!("{} · {} tok", s.label, fmt_tok(s.tokens)), s.content.clone(), 0));
        } else if let Some(seg) = self.view.segments.get(self.sel - n) {
            let body = format!(
                "{} → {} tokens\n\n## Path\n{}\n\n## State\n{}",
                fmt_tok(seg.tokens_before),
                fmt_tok(seg.tokens_after),
                seg.path,
                seg.state
            );
            self.detail = Some((format!("Segment {}", seg.index), body, 0));
        }
    }
}

pub fn draw(f: &mut Frame, st: &mut ContextState, area: Rect) {
    let th = t();
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.border))
        .title(Span::styled(" Context ", theme::bold(theme::text())));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let w = inner.width as usize;
    st.page = inner.height.saturating_sub(3).max(1) as usize;

    let v = &st.view;
    let [head, body] = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(inner);
    let mut head_lines = vec![Line::from(vec![
        Span::styled(format!("{} / {}", fmt_tok(v.used), fmt_tok(v.limit)), theme::bold(theme::text())),
        Span::styled(format!("   compacts at {}", fmt_tok(v.threshold)), theme::muted()),
    ])];
    head_lines.push(Line::from(gauge(v.used, v.limit, w)));
    f.render_widget(Paragraph::new(head_lines), head);

    if let Some((title, content, scroll)) = &mut st.detail {
        let mut lines = vec![Line::from(Span::styled(title.clone(), theme::bold(theme::accent()))), Line::default()];
        let mut body_lines = Vec::new();
        for l in content.split('\n') {
            body_lines.extend(wrap(&[Span::styled(l.to_string(), theme::text())], w, &[], &[], None));
        }
        let h = body.height.saturating_sub(2) as usize;
        *scroll = (*scroll).min(body_lines.len().saturating_sub(h));
        lines.extend(body_lines.into_iter().skip(*scroll).take(h));
        f.render_widget(Paragraph::new(lines), body);
        return;
    }

    let label_w = 22.min(w / 2);
    let bar_w = w.saturating_sub(label_w + 10).min(40);
    let max_tok = v.sections.iter().map(|s| s.tokens).max().unwrap_or(1).max(1);
    let mut rows: Vec<Line> = Vec::new();
    for (i, s) in v.sections.iter().enumerate() {
        let sel = i == st.sel;
        let bg = if sel { Style::default().bg(th.sel_bg) } else { Style::default() };
        let label = truncate(&s.label, label_w);
        let pad = label_w.saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()));
        let filled = ((s.tokens as f64 / max_tok as f64) * bar_w as f64).round() as usize;
        rows.push(Line::from(vec![
            Span::styled(format!("{label}{}", " ".repeat(pad)), theme::text().patch(bg)),
            Span::styled(format!("{:>8}  ", fmt_tok(s.tokens)), theme::muted().patch(bg)),
            Span::styled("■".repeat(filled.min(bar_w)), Style::default().fg(th.accent)),
        ]));
    }
    if !v.segments.is_empty() {
        rows.push(Line::default());
        rows.push(Line::from(Span::styled("Segments", theme::muted())));
        let n = v.sections.len();
        for (j, seg) in v.segments.iter().enumerate() {
            let sel = n + j == st.sel;
            let bg = if sel { Style::default().bg(th.sel_bg) } else { Style::default() };
            let path_first = seg.path.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            rows.push(Line::from(vec![
                Span::styled(format!("#{:<3}", seg.index), theme::text().patch(bg)),
                Span::styled(format!("{} → {}  ", fmt_tok(seg.tokens_before), fmt_tok(seg.tokens_after)), theme::muted().patch(bg)),
                Span::styled(truncate(path_first, w.saturating_sub(24)), theme::muted().patch(bg)),
            ]));
        }
    }
    // Keep the selection visible.
    let h = body.height as usize;
    let sel_row = if st.sel < v.sections.len() { st.sel } else { st.sel + 2 };
    let top = if sel_row >= h { sel_row + 1 - h } else { 0 };
    let rows: Vec<Line> = rows.into_iter().skip(top).take(h).collect();
    f.render_widget(Paragraph::new(rows), body);
}

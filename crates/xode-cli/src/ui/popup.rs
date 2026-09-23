//! Completion popup for `/commands` and `@paths`.
use crate::render::truncate;
use crate::theme::{self, t};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use xode_engine::CommandInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupKind {
    Slash,
    Path,
}

#[derive(Debug, Clone)]
pub struct PopupItem {
    pub label: String,
    pub detail: String,
    /// Replacement for the current token.
    pub insert: String,
    /// Slash command without arguments: run immediately on Enter.
    pub runnable: bool,
}

#[derive(Debug, Clone)]
pub struct Popup {
    pub kind: PopupKind,
    pub items: Vec<PopupItem>,
    pub sel: usize,
    /// Byte offset in the input where the completed token starts.
    pub token_start: usize,
}

impl Popup {
    pub fn up(&mut self) {
        if !self.items.is_empty() {
            self.sel = (self.sel + self.items.len() - 1) % self.items.len();
        }
    }
    pub fn down(&mut self) {
        if !self.items.is_empty() {
            self.sel = (self.sel + 1) % self.items.len();
        }
    }
    pub fn selected(&self) -> Option<&PopupItem> {
        self.items.get(self.sel)
    }
}

/// Filter commands by the typed prefix (without the leading `/`). Prefix matches first, then substring.
pub fn filter_commands(cmds: &[CommandInfo], typed: &str) -> Vec<PopupItem> {
    let q = typed.to_lowercase();
    let mut pre = Vec::new();
    let mut sub = Vec::new();
    for c in cmds {
        let n = c.name.trim_start_matches('/').to_lowercase();
        let item = PopupItem {
            label: format!("/{}{}", c.name.trim_start_matches('/'), if c.args.is_empty() { String::new() } else { format!(" {}", c.args) }),
            detail: c.description.clone(),
            insert: format!("/{}", c.name.trim_start_matches('/')),
            runnable: c.args.is_empty(),
        };
        if n.starts_with(&q) {
            pre.push(item);
        } else if n.contains(&q) {
            sub.push(item);
        }
    }
    pre.extend(sub);
    pre
}

pub fn path_items(paths: Vec<String>) -> Vec<PopupItem> {
    paths
        .into_iter()
        .take(50)
        .map(|p| PopupItem { label: p.clone(), detail: String::new(), insert: format!("@{p}"), runnable: false })
        .collect()
}

/// Draw anchored above `anchor` (the input box), within `bounds`.
pub fn draw(f: &mut Frame, p: &Popup, anchor: Rect, bounds: Rect) {
    if p.items.is_empty() {
        return;
    }
    let th = t();
    let max_rows = 8usize;
    let n = p.items.len().min(max_rows);
    let h = (n + 2) as u16;
    let w = bounds.width.min(72).max(20);
    let y = anchor.y.saturating_sub(h).max(bounds.y);
    let area = Rect { x: anchor.x, y, width: w.min(bounds.x + bounds.width - anchor.x), height: h.min(anchor.y - bounds.y) };
    if area.height < 3 {
        return;
    }
    let top = if p.sel >= n { p.sel + 1 - n } else { 0 };
    let inner_w = area.width.saturating_sub(2) as usize;
    let label_w = p.items.iter().map(|i| i.label.chars().count()).max().unwrap_or(0).min(inner_w * 2 / 3);
    let lines: Vec<Line> = p
        .items
        .iter()
        .enumerate()
        .skip(top)
        .take(n)
        .map(|(i, it)| {
            let sel = i == p.sel;
            let bg = if sel { Style::default().bg(th.sel_bg) } else { Style::default() };
            let label = truncate(&it.label, label_w);
            let pad = label_w.saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()));
            let detail = truncate(&it.detail, inner_w.saturating_sub(label_w + 3));
            let used = 1 + label_w + 2 + unicode_width::UnicodeWidthStr::width(detail.as_str());
            Line::from(vec![
                Span::styled(" ", bg),
                Span::styled(label, theme::text().patch(bg)),
                Span::styled(" ".repeat(pad + 2), bg),
                Span::styled(detail, theme::muted().patch(bg)),
                Span::styled(" ".repeat(inner_w.saturating_sub(used)), bg),
            ])
        })
        .collect();
    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(th.border));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(n: &str, args: &str) -> CommandInfo {
        CommandInfo { name: n.into(), args: args.into(), description: String::new(), source: "builtin".into() }
    }

    #[test]
    fn filters_prefix_first() {
        let cmds = vec![cmd("compact", ""), cmd("context", ""), cmd("resume", "[id]"), cmd("goal", "<text>")];
        let v = filter_commands(&cmds, "co");
        assert_eq!(v.iter().map(|i| i.insert.as_str()).collect::<Vec<_>>(), vec!["/compact", "/context"]);
        let v = filter_commands(&cmds, "su");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].insert, "/resume");
        assert!(!v[0].runnable);
        assert_eq!(filter_commands(&cmds, "").len(), 4);
    }
}

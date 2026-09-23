//! Multi-line input editor (state + rendering).
use crate::render::cw;
use crate::theme::{self, t};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

#[derive(Debug, Default)]
pub struct Input {
    pub text: String,
    /// Byte offset of the cursor in `text`.
    pub cursor: usize,
    history: Vec<String>,
    hist_idx: Option<usize>,
    draft: String,
}

impl Input {
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.hist_idx = None;
    }
    pub fn set(&mut self, s: &str) {
        self.text = s.to_string();
        self.cursor = self.text.len();
    }
    /// Take the text for submission and remember it in history.
    pub fn take(&mut self) -> String {
        let s = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.hist_idx = None;
        if !s.trim().is_empty() && self.history.last() != Some(&s) {
            self.history.push(s.clone());
        }
        s
    }
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }
    pub fn insert_str(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }
    fn prev_boundary(&self) -> Option<usize> {
        self.text[..self.cursor].char_indices().next_back().map(|(i, _)| i)
    }
    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..].chars().next().map(|c| self.cursor + c.len_utf8())
    }
    pub fn backspace(&mut self) {
        if let Some(p) = self.prev_boundary() {
            self.text.replace_range(p..self.cursor, "");
            self.cursor = p;
        }
    }
    pub fn delete(&mut self) {
        if let Some(n) = self.next_boundary() {
            self.text.replace_range(self.cursor..n, "");
        }
    }
    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let start = trimmed.rfind(|c: char| c.is_whitespace() || c == '/').map(|i| i + 1).unwrap_or(0);
        let start = if start == self.cursor && self.cursor > 0 { self.prev_boundary().unwrap_or(0) } else { start };
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }
    pub fn left(&mut self) {
        if let Some(p) = self.prev_boundary() {
            self.cursor = p;
        }
    }
    pub fn right(&mut self) {
        if let Some(n) = self.next_boundary() {
            self.cursor = n;
        }
    }
    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }
    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map(|i| self.cursor + i).unwrap_or(self.text.len())
    }
    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }
    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }
    fn col(&self) -> usize {
        self.text[self.line_start()..self.cursor].chars().count()
    }
    fn move_to_col(&mut self, line_start: usize, col: usize) {
        let line_end = self.text[line_start..].find('\n').map(|i| line_start + i).unwrap_or(self.text.len());
        let mut pos = line_start;
        for (n, (i, c)) in self.text[line_start..line_end].char_indices().enumerate() {
            if n == col {
                pos = line_start + i;
                break;
            }
            pos = line_start + i + c.len_utf8();
        }
        self.cursor = pos;
    }
    /// Move up a line; returns false when already on the first line (then history is used).
    pub fn up(&mut self) -> bool {
        let ls = self.line_start();
        if ls == 0 {
            return self.history_prev();
        }
        let col = self.col();
        let prev_start = self.text[..ls - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.move_to_col(prev_start, col);
        true
    }
    pub fn down(&mut self) -> bool {
        let le = self.line_end();
        if le >= self.text.len() {
            return self.history_next();
        }
        let col = self.col();
        self.move_to_col(le + 1, col);
        true
    }
    fn history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let idx = match self.hist_idx {
            None => {
                self.draft = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => return false,
            Some(i) => i - 1,
        };
        self.hist_idx = Some(idx);
        let s = self.history[idx].clone();
        self.set(&s);
        true
    }
    fn history_next(&mut self) -> bool {
        match self.hist_idx {
            None => false,
            Some(i) if i + 1 >= self.history.len() => {
                self.hist_idx = None;
                let d = std::mem::take(&mut self.draft);
                self.set(&d);
                true
            }
            Some(i) => {
                self.hist_idx = Some(i + 1);
                let s = self.history[i + 1].clone();
                self.set(&s);
                true
            }
        }
    }

    /// The whitespace-delimited token ending at the cursor: (start byte, token).
    pub fn current_token(&self) -> (usize, &str) {
        let before = &self.text[..self.cursor];
        let start = before.rfind(char::is_whitespace).map(|i| i + before[i..].chars().next().unwrap().len_utf8()).unwrap_or(0);
        (start, &self.text[start..self.cursor])
    }

    /// Replace the token starting at `start` (up to cursor) with `with`.
    pub fn replace_token(&mut self, start: usize, with: &str) {
        self.text.replace_range(start..self.cursor, with);
        self.cursor = start + with.len();
    }

    /// Visual rows for a given inner width: (byte start, byte end) of each row.
    pub fn rows(&self, width: usize) -> Vec<(usize, usize)> {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut start = 0;
        for line in self.text.split('\n') {
            let mut row_start = start;
            let mut w = 0;
            for (i, c) in line.char_indices() {
                let cwid = cw(c);
                if w + cwid > width {
                    rows.push((row_start, start + i));
                    row_start = start + i;
                    w = 0;
                }
                w += cwid;
            }
            rows.push((row_start, start + line.len()));
            start += line.len() + 1;
        }
        rows
    }

    /// Cursor (row, col) in visual rows.
    pub fn cursor_pos(&self, width: usize) -> (usize, usize) {
        let rows = self.rows(width);
        for (r, &(s, e)) in rows.iter().enumerate() {
            let next_start = rows.get(r + 1).map(|x| x.0);
            let is_last_on_line = next_start.map(|n| n > e).unwrap_or(true);
            if self.cursor >= s && (self.cursor < e || (self.cursor == e && is_last_on_line)) {
                let col = self.text[s..self.cursor].chars().map(cw).sum();
                return (r, col);
            }
        }
        (rows.len().saturating_sub(1), 0)
    }

    /// Block height: top pad, text rows, spacer, footer, bottom pad.
    pub fn height(&self, width: u16) -> u16 {
        let inner = width.saturating_sub(5) as usize;
        (self.rows(inner).len().clamp(1, 10) + 4) as u16
    }
}

/// opencode-style input: tinted block with an accent bar on the left and a footer line.
pub fn draw(f: &mut Frame, input: &Input, area: Rect, active: bool, footer: Line<'static>) {
    let th = t();
    let bg = Style::default().bg(th.block_bg);
    f.render_widget(Block::default().style(bg), area);
    let bar_color = if active { th.accent } else { th.border };
    let bar: Vec<Line> = (0..area.height).map(|_| Line::from(Span::styled("┃", Style::default().fg(bar_color).bg(th.block_bg)))).collect();
    f.render_widget(Paragraph::new(bar), Rect { width: 1, ..area });

    let text_h = area.height.saturating_sub(4);
    let inner = Rect { x: area.x + 3, y: area.y + 1, width: area.width.saturating_sub(5), height: text_h };
    let w = inner.width as usize;
    let rows = input.rows(w);
    let (crow, ccol) = input.cursor_pos(w);
    let h = inner.height as usize;
    let top = if crow >= h { crow + 1 - h } else { 0 };
    let lines: Vec<Line> = rows
        .iter()
        .skip(top)
        .take(h)
        .map(|&(s, e)| Line::from(Span::styled(input.text[s..e].replace('\t', "    "), theme::text().bg(th.block_bg))))
        .collect();
    f.render_widget(Paragraph::new(lines).style(bg), inner);
    let foot = Rect { x: inner.x, y: area.y + area.height.saturating_sub(2), width: inner.width, height: 1 };
    f.render_widget(Paragraph::new(footer).style(bg), foot);
    if active {
        f.set_cursor_position((inner.x + ccol as u16, inner.y + (crow - top) as u16));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_cursor() {
        let mut i = Input::default();
        for c in "hello".chars() {
            i.insert(c);
        }
        i.insert('\n');
        i.insert_str("wörld");
        assert_eq!(i.cursor_pos(20), (1, 5));
        assert!(i.up());
        assert_eq!(i.cursor_pos(20), (0, 5));
        i.backspace();
        assert_eq!(i.text, "hell\nwörld");
        i.end();
        i.down();
        assert_eq!(i.cursor_pos(20), (1, 4));
        assert_eq!(i.rows(3).len(), 4);
    }

    #[test]
    fn tokens_and_history() {
        let mut i = Input::default();
        i.set("look at @src/ma");
        let (s, tok) = i.current_token();
        assert_eq!(tok, "@src/ma");
        i.replace_token(s, "@src/main.rs ");
        assert_eq!(i.text, "look at @src/main.rs ");
        assert_eq!(i.take(), "look at @src/main.rs ");
        i.set("draft");
        assert!(i.up());
        assert_eq!(i.text, "look at @src/main.rs ");
        assert!(i.down());
        assert_eq!(i.text, "draft");
    }
}

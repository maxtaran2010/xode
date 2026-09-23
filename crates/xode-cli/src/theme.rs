//! Fluent-ish blue palette shared with the desktop app.
use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub text: Color,
    pub muted: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub thinking: Color,
    pub code_bg: Color,
    pub border: Color,
    pub sel_bg: Color,
    /// Main background.
    pub bg: Color,
    /// Sidebar background.
    pub panel_bg: Color,
    /// User message / input block background.
    pub block_bg: Color,
}

const TRUE: Theme = Theme {
    accent: Color::Rgb(0x4a, 0x8d, 0xff),
    text: Color::Rgb(0xe6, 0xe9, 0xef),
    muted: Color::Rgb(0x7d, 0x85, 0x95),
    ok: Color::Rgb(0x3f, 0xb6, 0x8b),
    warn: Color::Rgb(0xe0, 0xa8, 0x4a),
    err: Color::Rgb(0xe5, 0x60, 0x4f),
    thinking: Color::Rgb(0x9a, 0xa6, 0xc7),
    code_bg: Color::Rgb(0x1c, 0x21, 0x2b),
    border: Color::Rgb(0x34, 0x3b, 0x4a),
    sel_bg: Color::Rgb(0x22, 0x33, 0x55),
    bg: Color::Rgb(0x0d, 0x0f, 0x13),
    panel_bg: Color::Rgb(0x14, 0x17, 0x1d),
    block_bg: Color::Rgb(0x1b, 0x1f, 0x27),
};

const BASIC: Theme = Theme {
    accent: Color::LightBlue,
    text: Color::White,
    muted: Color::DarkGray,
    ok: Color::Green,
    warn: Color::Yellow,
    err: Color::LightRed,
    thinking: Color::Gray,
    code_bg: Color::Reset,
    border: Color::DarkGray,
    sel_bg: Color::Blue,
    bg: Color::Reset,
    panel_bg: Color::Reset,
    block_bg: Color::Reset,
};

static THEME: OnceLock<Theme> = OnceLock::new();

fn detect() -> Theme {
    if let Ok(v) = std::env::var("XODE_COLORS") {
        return if v == "16" { BASIC } else { TRUE };
    }
    let ct = std::env::var("COLORTERM").unwrap_or_default();
    if ct.contains("truecolor") || ct.contains("24bit") {
        return TRUE;
    }
    // Apple Terminal (older) lacks truecolor; everything else we care about has it
    // (Windows Terminal, modern conhost with VT, iTerm, VS Code, kitty ...).
    if std::env::var("TERM_PROGRAM").map(|t| t == "Apple_Terminal").unwrap_or(false) {
        return BASIC;
    }
    TRUE
}

pub fn t() -> &'static Theme {
    THEME.get_or_init(detect)
}

pub fn fg(c: Color) -> Style {
    Style::default().fg(c)
}
pub fn text() -> Style {
    fg(t().text)
}
pub fn muted() -> Style {
    fg(t().muted)
}
pub fn accent() -> Style {
    fg(t().accent)
}
pub fn bold(s: Style) -> Style {
    s.add_modifier(Modifier::BOLD)
}

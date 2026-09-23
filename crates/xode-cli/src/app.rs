//! TUI application state and input/event handling.
use crate::ui::chat::ChatCache;
use crate::ui::context::ContextState;
use crate::ui::input::Input;
use crate::ui::popup::{self, Popup, PopupKind};
use crate::view::{ChatView, Level};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use xode_engine::xode_core::store::{Project, SessionInfo};
use xode_engine::xode_core::{AgentEvent, Mode};
use xode_engine::{CommandInfo, CommandResult, Engine, PermDecision};

pub struct PermAsk {
    pub req_id: String,
    pub tool: String,
    pub summary: String,
}

pub enum Overlay {
    None,
    Sessions { list: Vec<SessionInfo>, sel: usize },
    Context(ContextState),
}

/// Results of background engine calls.
pub enum AppMsg {
    Command(anyhow::Result<CommandResult>),
    Error(String),
}

pub struct App {
    pub engine: Arc<Engine>,
    pub project: Project,
    pub session: SessionInfo,
    pub view: ChatView,
    pub input: Input,
    pub popup: Option<Popup>,
    pub overlay: Overlay,
    pub perms: VecDeque<PermAsk>,
    pub commands: Vec<CommandInfo>,
    pub show_panel: bool,
    /// Lines scrolled up from the bottom (0 = follow).
    pub scroll: usize,
    pub last_total: usize,
    /// Focused collapsible item (index into view.items).
    pub focus: Option<usize>,
    pub focus_in_chat: bool,
    pub reveal: Option<usize>,
    pub chat_cache: ChatCache,
    pub chat_area: Rect,
    pub chat_rows: Vec<Option<usize>>,
    pub quit_armed: Option<Instant>,
    pub esc_armed: Option<Instant>,
    pub should_quit: bool,
    pub flash: Option<(String, Instant)>,
    pub started: Instant,
    pub tx: UnboundedSender<AppMsg>,
    /// Gateway id → display name.
    pub gateway_names: std::collections::HashMap<String, String>,
    /// Reasoning effort shown in the input footer ("" = model default).
    pub effort: String,
    /// Terminal focus (for notifications).
    pub focused: bool,
}

impl App {
    pub fn new(engine: Arc<Engine>, project: Project, session: SessionInfo, tx: UnboundedSender<AppMsg>) -> Self {
        let commands = engine.commands(Some(&project.id));
        let gateway_names = engine.config().gateways.into_iter().map(|g| (g.id.clone(), if g.name.is_empty() { g.url } else { g.name })).collect();
        let mut app = Self {
            engine,
            project,
            view: ChatView::new(session.id.clone()),
            session,
            input: Input::default(),
            popup: None,
            overlay: Overlay::None,
            perms: VecDeque::new(),
            commands,
            show_panel: true,
            scroll: 0,
            last_total: 0,
            focus: None,
            focus_in_chat: false,
            reveal: None,
            chat_cache: ChatCache::default(),
            chat_area: Rect::default(),
            chat_rows: Vec::new(),
            quit_armed: None,
            esc_armed: None,
            should_quit: false,
            flash: None,
            started: Instant::now(),
            tx,
            gateway_names,
            effort: String::new(),
            focused: true,
        };
        let id = app.session.id.clone();
        app.load_session(&id);
        app
    }

    pub fn load_session(&mut self, id: &str) {
        match self.engine.session(id) {
            Ok(s) => self.session = s,
            Err(e) => {
                self.view.notice(format!("cannot open session: {e}"), Level::Error);
                return;
            }
        }
        let mut view = ChatView::new(id);
        match self.engine.messages(id) {
            Ok(m) => view.load(&m),
            Err(e) => view.notice(format!("cannot load messages: {e}"), Level::Error),
        }
        view.stats = self.engine.stats(id);
        view.running = self.engine.is_running(id);
        view.queue = self.engine.queued(id);
        if view.running {
            view.run_start = Some(Instant::now());
            view.activity = crate::view::Activity::Waiting;
        }
        self.view = view;
        self.chat_cache.reset();
        self.scroll = 0;
        self.last_total = 0;
        self.focus = None;
        self.focus_in_chat = false;
        self.perms.clear();
    }

    fn refresh_session(&mut self) {
        if let Ok(s) = self.engine.session(&self.session.id) {
            self.session = s;
        }
        // Gateways may have been added by /connect.
        let cfg = self.engine.config();
        self.effort = cfg.generation.effort().to_string();
        self.gateway_names = cfg
            .gateways
            .into_iter()
            .map(|g| (g.id.clone(), if g.name.is_empty() { g.url } else { g.name }))
            .collect();
    }

    /// Reload after a lagged event stream.
    pub fn resync(&mut self) {
        let id = self.session.id.clone();
        let (scroll, panel) = (self.scroll, self.show_panel);
        self.load_session(&id);
        self.scroll = scroll;
        self.show_panel = panel;
    }

    /// Whether something animates (spinner) and needs periodic redraws.
    pub fn animating(&self) -> bool {
        self.view.running || self.flash.as_ref().map(|f| f.1.elapsed().as_secs() < 4).unwrap_or(false)
    }

    pub fn gateway_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.gateway_names.get(id).map(|s| s.as_str()).unwrap_or(id)
    }

    /// Terminal notification: bell (taskbar flash / dock bounce) plus OSC 9/777 desktop
    /// notifications on terminals that support them.
    fn notify(&self, text: &str, n: &xode_engine::xode_core::config::Notifications) {
        if n.only_unfocused && self.focused {
            return;
        }
        use std::io::Write;
        let mut out = std::io::stdout();
        let text: String = text.chars().filter(|c| !c.is_control() && *c != ';').collect();
        if !cfg!(windows) {
            let _ = write!(out, "\x1b]9;{text}\x07\x1b]777;notify;Xode;{text}\x07");
        }
        if n.sound || cfg!(windows) {
            let _ = write!(out, "\x07");
        }
        let _ = out.flush();
    }

    fn flash(&mut self, s: &str) {
        self.flash = Some((s.to_string(), Instant::now()));
    }

    // ------------------------------------------------------------ engine events

    pub fn on_agent_event(&mut self, ev: AgentEvent) {
        if ev.session() != self.session.id {
            return;
        }
        match &ev {
            AgentEvent::PermissionAsk { req_id, tool, summary, .. } => {
                self.perms.push_back(PermAsk { req_id: req_id.clone(), tool: tool.clone(), summary: summary.clone() });
                let n = self.engine.config().notifications;
                if n.permission {
                    self.notify(&format!("Permission needed: {tool}"), &n);
                }
            }
            AgentEvent::Finished { stopped: false, error, .. } => {
                let n = self.engine.config().notifications;
                if (*error && n.errors) || (!*error && n.finished) {
                    let title = if self.session.title.trim().is_empty() { "Xode" } else { self.session.title.trim() };
                    self.notify(&format!("{} · {title}", if *error { "Failed" } else { "Done" }), &n);
                }
            }
            AgentEvent::CommandDone { result, .. } => {
                if let Ok(r) = serde_json::from_value::<CommandResult>(result.clone()) {
                    self.on_command_result(r);
                }
            }
            AgentEvent::State { running: false, .. } => {
                self.perms.clear();
                self.view.apply(&ev, Instant::now());
                self.refresh_session();
            }
            _ => self.view.apply(&ev, Instant::now()),
        }
    }

    pub fn on_msg(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::Error(e) => self.view.notice(e, Level::Error),
            AppMsg::Command(Err(e)) => self.view.notice(format!("{e}"), Level::Error),
            AppMsg::Command(Ok(r)) => self.on_command_result(r),
        }
    }

    fn on_command_result(&mut self, r: CommandResult) {
        match r {
            CommandResult::Done => {}
            CommandResult::Notice { text } => self.view.notice(text, Level::Info),
            CommandResult::SwitchSession { session_id } => self.load_session(&session_id),
            CommandResult::Open { panel } => self.open_panel(&panel),
            CommandResult::Export { markdown, path } => {
                let path = if path.trim().is_empty() {
                    let name = format!("xode-{}.md", chrono::Local::now().format("%Y%m%d-%H%M%S"));
                    std::path::Path::new(&self.project.root).join(name).to_string_lossy().to_string()
                } else {
                    path
                };
                match std::fs::write(&path, markdown) {
                    Ok(_) => self.view.notice(format!("exported to {path}"), Level::Info),
                    Err(e) => self.view.notice(format!("export failed: {e}"), Level::Error),
                }
            }
            CommandResult::Exit => self.should_quit = true,
        }
        self.refresh_session();
        self.commands = self.engine.commands(Some(&self.project.id));
    }

    fn open_panel(&mut self, panel: &str) {
        let name = panel.split(':').next().unwrap_or(panel);
        match name {
            "sessions" => match self.engine.sessions(Some(&self.project.id)) {
                Ok(mut list) => {
                    list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                    let sel = list.iter().position(|s| s.id == self.session.id).unwrap_or(0);
                    self.overlay = Overlay::Sessions { list, sel };
                }
                Err(e) => self.view.notice(format!("{e}"), Level::Error),
            },
            "context" => match self.engine.context_view(&self.session.id) {
                Ok(mut v) => {
                    // Per-tool rows go right under "Tool results".
                    let mut flat = vec![];
                    for mut sec in std::mem::take(&mut v.sections) {
                        let kids = std::mem::take(&mut sec.children);
                        flat.push(sec);
                        for mut k in kids {
                            k.label = format!("  {}", k.label);
                            flat.push(k);
                        }
                    }
                    v.sections = flat;
                    self.overlay = Overlay::Context(ContextState::new(v))
                }
                Err(e) => self.view.notice(format!("{e}"), Level::Error),
            },
            "settings" => {
                let p = xode_engine::xode_core::config::config_path();
                self.view.notice(format!("use the desktop app or edit {}", p.display()), Level::Info);
            }
            "project" => self.view.notice(format!("project {} · {}", self.project.name, self.project.root), Level::Info),
            "knowledge" => match self.engine.kb_overview(&self.project.id) {
                Ok(o) => {
                    let off = self.engine.session(&self.session.id).map(|s| s.kb_off).unwrap_or_default();
                    let lines: Vec<String> = o
                        .sources
                        .iter()
                        .map(|s| {
                            let on = !off.contains(&s.key) && !off.iter().any(|k| k == s.layer.key());
                            format!("{} {} · {} · {} notes", if on { "on " } else { "off" }, s.layer.label(), s.name, s.notes)
                        })
                        .collect();
                    let text = if lines.is_empty() { "knowledge base is empty: /kb add library <folder>".into() } else { lines.join("\n") };
                    self.view.notice(text, Level::Info)
                }
                Err(e) => self.view.notice(format!("{e}"), Level::Error),
            },
            other => self.view.notice(format!("panel '{other}' is not available in the terminal"), Level::Info),
        }
    }

    // ------------------------------------------------------------ terminal events

    pub fn on_term_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Paste(s) => {
                if matches!(self.overlay, Overlay::None) && self.perms.is_empty() {
                    self.focus_in_chat = false;
                    self.input.insert_str(&s);
                    self.update_popup();
                }
            }
            Event::Mouse(m) => self.on_mouse(m),
            Event::Resize(..) => self.chat_cache.reset(),
            Event::FocusGained => self.focused = true,
            Event::FocusLost => self.focused = false,
            _ => {}
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let delta: i32 = match m.kind {
            MouseEventKind::ScrollUp => -3,
            MouseEventKind::ScrollDown => 3,
            MouseEventKind::Down(MouseButton::Left) => {
                if matches!(self.overlay, Overlay::None) {
                    self.click(m.column, m.row);
                }
                return;
            }
            _ => return,
        };
        match &mut self.overlay {
            Overlay::Context(st) => st.scroll(delta),
            Overlay::Sessions { list, sel } => {
                *sel = if delta < 0 { sel.saturating_sub(1) } else { (*sel + 1).min(list.len().saturating_sub(1)) };
            }
            Overlay::None => self.scroll_by(-delta),
        }
    }

    fn click(&mut self, col: u16, row: u16) {
        let a = self.chat_area;
        if col < a.x || col >= a.x + a.width || row < a.y || row >= a.y + a.height {
            return;
        }
        if let Some(Some(idx)) = self.chat_rows.get((row - a.y) as usize).copied() {
            if self.view.items[idx].item.collapsible() {
                self.view.toggle(idx);
                self.focus = Some(idx);
            }
        }
    }

    /// Positive = scroll up (older content).
    fn scroll_by(&mut self, lines: i32) {
        if lines > 0 {
            self.scroll += lines as usize;
        } else {
            self.scroll = self.scroll.saturating_sub((-lines) as usize);
        }
    }

    fn page(&self) -> i32 {
        (self.chat_area.height as i32 - 2).max(1)
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);

        // Ctrl+C only clears the input; it never stops the model or quits.
        if ctrl && k.code == KeyCode::Char('c') {
            self.input.clear();
            self.popup = None;
            return;
        }
        // Ctrl+S: steer — deliver queued messages now.
        if ctrl && k.code == KeyCode::Char('s') {
            if !self.view.queue.is_empty() {
                self.engine.steer(&self.session.id);
            }
            return;
        }
        // Ctrl+D quits (twice while the model is running).
        if ctrl && k.code == KeyCode::Char('d') {
            if !self.view.running || self.quit_armed.map(|t| t.elapsed() < Duration::from_secs(2)).unwrap_or(false) {
                self.should_quit = true;
            } else {
                self.quit_armed = Some(Instant::now());
                self.flash("ctrl+d again to quit");
            }
            return;
        }

        // Overlays capture keys.
        match &mut self.overlay {
            Overlay::Context(st) => {
                if !st.key(k) {
                    self.overlay = Overlay::None;
                }
                return;
            }
            Overlay::Sessions { list, sel } => {
                match k.code {
                    KeyCode::Esc => self.overlay = Overlay::None,
                    KeyCode::Up => *sel = sel.saturating_sub(1),
                    KeyCode::Down => *sel = (*sel + 1).min(list.len().saturating_sub(1)),
                    KeyCode::PageUp => *sel = sel.saturating_sub(10),
                    KeyCode::PageDown => *sel = (*sel + 10).min(list.len().saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some(s) = list.get(*sel) {
                            let id = s.id.clone();
                            self.overlay = Overlay::None;
                            self.load_session(&id);
                        }
                    }
                    _ => {}
                }
                return;
            }
            Overlay::None => {}
        }

        // Permission prompt.
        if let Some(p) = self.perms.front() {
            let d = match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => Some(PermDecision::Once),
                KeyCode::Char('a') | KeyCode::Char('A') => Some(PermDecision::Always),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(PermDecision::Deny),
                _ => None,
            };
            if let Some(d) = d {
                self.engine.permission_reply(&p.req_id, d);
                self.perms.pop_front();
            }
            return;
        }

        // Global keys.
        match k.code {
            KeyCode::Char('b') if ctrl => {
                self.show_panel = !self.show_panel;
                return;
            }
            KeyCode::BackTab => {
                self.toggle_mode();
                return;
            }
            KeyCode::PageUp => {
                self.scroll_by(self.page());
                return;
            }
            KeyCode::PageDown => {
                self.scroll_by(-self.page());
                return;
            }
            KeyCode::Up if ctrl => {
                self.move_focus(-1);
                return;
            }
            KeyCode::Down if ctrl => {
                self.move_focus(1);
                return;
            }
            _ => {}
        }

        // Completion popup.
        if let Some(p) = &mut self.popup {
            match k.code {
                KeyCode::Up => return p.up(),
                KeyCode::Down => return p.down(),
                KeyCode::Esc => {
                    self.popup = None;
                    return;
                }
                KeyCode::Tab => {
                    self.accept_popup(false);
                    return;
                }
                KeyCode::Enter if !alt && !shift => {
                    self.accept_popup(true);
                    return;
                }
                _ => {}
            }
        }

        // Chat focus mode.
        if self.focus_in_chat {
            match k.code {
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Tab => {
                    if let Some(i) = self.focus {
                        self.view.toggle(i);
                        self.reveal = Some(i);
                    }
                    return;
                }
                KeyCode::Up => return self.move_focus(-1),
                KeyCode::Down => return self.move_focus(1),
                KeyCode::Esc => {
                    self.focus_in_chat = false;
                    return;
                }
                _ => self.focus_in_chat = false,
            }
        }

        match k.code {
            KeyCode::Tab => {
                let target = self.focus.filter(|&i| i < self.view.items.len()).or_else(|| self.view.collapsibles().last().copied());
                if let Some(i) = target {
                    self.view.toggle(i);
                    self.focus = Some(i);
                    self.reveal = Some(i);
                }
            }
            KeyCode::Esc => {
                if self.view.running {
                    // Esc twice within 1.5s stops the model.
                    if self.esc_armed.map(|t| t.elapsed() < Duration::from_millis(1500)).unwrap_or(false) {
                        self.esc_armed = None;
                        self.engine.cancel(&self.session.id);
                        self.flash("stopping");
                    } else {
                        self.esc_armed = Some(Instant::now());
                        self.flash("esc again to stop");
                    }
                } else {
                    self.scroll = 0;
                }
            }
            KeyCode::Enter if alt || shift => self.input.insert('\n'),
            KeyCode::Char('j') if ctrl => self.input.insert('\n'),
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace if ctrl || alt => self.input.delete_word(),
            KeyCode::Char('w') if ctrl => self.input.delete_word(),
            KeyCode::Char('h') if ctrl => self.input.backspace(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Char('a') if ctrl => self.input.home(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Up => {
                self.input.up();
            }
            KeyCode::Down => {
                self.input.down();
            }
            KeyCode::Char(c) if !ctrl || alt => self.input.insert(c),
            _ => return,
        }
        self.update_popup();
    }

    fn toggle_mode(&mut self) {
        let m = match self.session.mode {
            Mode::Normal => Mode::Plan,
            Mode::Plan => Mode::Normal,
        };
        match self.engine.set_mode(&self.session.id, m) {
            Ok(_) => {
                self.session.mode = m;
                self.refresh_session();
            }
            Err(e) => self.view.notice(format!("{e}"), Level::Error),
        }
    }

    fn move_focus(&mut self, dir: i32) {
        let c = self.view.collapsibles();
        if c.is_empty() {
            return;
        }
        let pos = self.focus.and_then(|f| c.iter().position(|&i| i == f));
        let next = match (pos, self.focus_in_chat) {
            (Some(p), true) => {
                if dir < 0 {
                    p.saturating_sub(1)
                } else {
                    (p + 1).min(c.len() - 1)
                }
            }
            _ => c.len() - 1,
        };
        self.focus = Some(c[next]);
        self.focus_in_chat = true;
        self.reveal = Some(c[next]);
    }

    fn update_popup(&mut self) {
        let text = &self.input.text;
        // Slash commands: the whole input is a single `/word` at the cursor.
        if text.starts_with('/') && !text[..self.input.cursor].contains(char::is_whitespace) {
            let typed = &text[1..self.input.cursor];
            let items = popup::filter_commands(&self.commands, typed);
            let sel = self.popup.as_ref().filter(|p| p.kind == PopupKind::Slash).map(|p| p.sel).unwrap_or(0);
            self.popup = if items.is_empty() {
                None
            } else {
                Some(Popup { kind: PopupKind::Slash, sel: sel.min(items.len() - 1), items, token_start: 0 })
            };
            return;
        }
        let (start, tok) = self.input.current_token();
        if let Some(prefix) = tok.strip_prefix('@') {
            let items = popup::path_items(self.engine.complete_path(&self.project.id, prefix));
            self.popup = if items.is_empty() { None } else { Some(Popup { kind: PopupKind::Path, sel: 0, items, token_start: start }) };
            return;
        }
        self.popup = None;
    }

    fn accept_popup(&mut self, enter: bool) {
        let Some(p) = self.popup.take() else { return };
        let Some(item) = p.selected().cloned() else { return };
        match p.kind {
            PopupKind::Slash => {
                let already = self.input.text.trim() == item.insert;
                if enter && (already || item.runnable) {
                    self.input.set(&item.insert);
                    self.submit();
                    return;
                }
                self.input.set(&format!("{} ", item.insert));
            }
            PopupKind::Path => {
                let dir = item.insert.ends_with('/');
                let ins = if dir { item.insert.clone() } else { format!("{} ", item.insert) };
                self.input.replace_token(p.token_start, &ins);
                if dir {
                    self.update_popup();
                }
            }
        }
    }

    fn submit(&mut self) {
        let text = self.input.text.trim_end().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.input.take();
        self.popup = None;
        self.scroll = 0;
        let engine = self.engine.clone();
        let sid = self.session.id.clone();
        let tx = self.tx.clone();
        if text.starts_with('/') {
            let line = text.trim().to_string();
            if line == "/quit" || line == "/exit" {
                self.should_quit = true;
                return;
            }
            tokio::spawn(async move {
                let r = engine.command(&sid, &line).await;
                let _ = tx.send(AppMsg::Command(r));
            });
        } else {
            // While running, the engine queues it (steer); it shows up in the queue list instead.
            if !self.view.running {
                self.view.push_user_pending(&text);
            }
            tokio::spawn(async move {
                if let Err(e) = engine.send(&sid, text, Vec::new()).await {
                    let _ = tx.send(AppMsg::Error(format!("{e}")));
                }
            });
        }
    }
}

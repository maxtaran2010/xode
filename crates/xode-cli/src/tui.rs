//! Terminal setup/teardown and the event-driven redraw loop.
use crate::app::{App, AppMsg};
use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture,
    KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{execute, queue};
use ratatui::prelude::CrosstermBackend;
use ratatui::Terminal;
use std::io::{stdout, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use xode_engine::xode_core::store::{Project, SessionInfo};
use xode_engine::Engine;

static ENHANCED: AtomicBool = AtomicBool::new(false);
const FRAME: Duration = Duration::from_millis(16);
const SPIN: Duration = Duration::from_millis(80);

fn setup() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    terminal::enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste, EnableFocusChange)?;
    // Lets Shift+Enter be distinguished on terminals supporting the kitty protocol.
    if matches!(terminal::supports_keyboard_enhancement(), Ok(true)) {
        let _ = queue!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
        let _ = out.flush();
        ENHANCED.store(true, Ordering::Relaxed);
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        prev(info);
    }));
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

pub fn restore() {
    let mut out = stdout();
    if ENHANCED.swap(false, Ordering::Relaxed) {
        let _ = queue!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableFocusChange, DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen, ratatui::crossterm::cursor::Show);
    let _ = terminal::disable_raw_mode();
}

pub async fn run(engine: Arc<Engine>, project: Project, session: SessionInfo) -> Result<()> {
    // Subscribe before loading history so no event is lost in between.
    let mut events = engine.subscribe();
    let (tx, mut app_rx) = mpsc::unbounded_channel::<AppMsg>();
    let mut app = App::new(engine, project, session, tx);

    let mut terminal = setup()?;
    let (term_tx, mut term_rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(ev) => {
                if term_tx.send(ev).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    });

    let result = async {
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut dirty = true;
        let mut last_draw = Instant::now() - Duration::from_secs(1);
        loop {
            if dirty && last_draw.elapsed() >= FRAME {
                terminal.draw(|f| crate::ui::draw(f, &mut app))?;
                last_draw = Instant::now();
                dirty = false;
            }
            if app.should_quit {
                break;
            }
            // When a frame is pending, wake up exactly when it may be drawn.
            let wait = if dirty { FRAME.saturating_sub(last_draw.elapsed()) } else { Duration::from_secs(3600) };
            tokio::select! {
                _ = tokio::time::sleep(wait), if dirty => {}
                Some(ev) = term_rx.recv() => {
                    app.on_term_event(ev);
                    while let Ok(ev) = term_rx.try_recv() {
                        app.on_term_event(ev);
                    }
                    dirty = true;
                }
                r = events.recv() => {
                    match r {
                        Ok(ev) => {
                            app.on_agent_event(ev);
                            // Batch whatever else is queued (streaming deltas).
                            loop {
                                match events.try_recv() {
                                    Ok(ev) => app.on_agent_event(ev),
                                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => app.resync(),
                                    Err(_) => break,
                                }
                            }
                        }
                        Err(RecvError::Lagged(_)) => app.resync(),
                        Err(RecvError::Closed) => break,
                    }
                    dirty = true;
                }
                Some(msg) = app_rx.recv() => {
                    app.on_msg(msg);
                    dirty = true;
                }
                _ = tick.tick() => {
                    if app.animating() && last_draw.elapsed() >= SPIN {
                        dirty = true;
                    }
                }
            }
        }
        anyhow::Ok(())
    }
    .await;

    restore();
    result
}

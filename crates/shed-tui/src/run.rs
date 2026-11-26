//! The interface in the real terminal: raw mode, the alternate screen,
//! mouse and bracketed paste; threads for the keeper's frames, the
//! terminal's events and a clock; drawing when something changed.

use std::io::{Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Context;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use shed_emu::CursorShape;
use shed_proto::{ClientKind, ShowTarget};

use crate::app::{App, Event, Options};

/// The terminal was asked to tell keys apart, and must be told to stop.
static ENHANCED: AtomicBool = AtomicBool::new(false);

/// At most this often a frame is drawn while output streams in.
const FRAME: Duration = Duration::from_millis(16);

/// Runs the interface until it is left (`C-b d`) or the terminal goes away.
/// Starts the keeper if it is not running; shows `target` first.
pub fn run(target: ShowTarget) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("shed tui needs a terminal (over ssh: ssh -t host shed tui)");
    }
    let (conn, frames) = shed_client::keeper_conn::connect(ClientKind::Tui, true)?;
    let (tx, events) = mpsc::channel();
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("keeper-events".into()).spawn(move || {
            while let Ok(frame) = frames.recv_blocking() {
                if tx.send(Event::Keeper(Box::new(frame))).is_err() {
                    return;
                }
            }
            let _ = tx.send(Event::KeeperGone);
        })?;
    }
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("clock".into()).spawn(move || {
            while tx.send(Event::Tick).is_ok() {
                std::thread::sleep(Duration::from_secs(1));
            }
        })?;
    }

    let mut app = App::new(conn, tx.clone(), Options::default());
    enter()?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave();
        previous(info);
    }));
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("terminal-events".into()).spawn(move || {
            loop {
                match event::read() {
                    Ok(ev) => {
                        if tx.send(Event::Term(ev)).is_err() {
                            return;
                        }
                    }
                    // The terminal went away (a dropped ssh): leave.
                    Err(_) => {
                        let _ = tx.send(Event::Apply(Box::new(|app: &mut App| app.quit = true)));
                        return;
                    }
                }
            }
        })?;
    }
    let result = main_loop(&mut app, &events, target);
    leave();
    result?;
    if let Some(fatal) = app.fatal {
        anyhow::bail!("{fatal}");
    }
    Ok(())
}

fn main_loop(app: &mut App, events: &mpsc::Receiver<Event>, target: ShowTarget) -> anyhow::Result<()> {
    let mut term = Terminal::new(CrosstermBackend::new(stdout())).context("opening the terminal")?;
    // Not `term.clear()`: it asks the terminal where the cursor is, and the
    // answer never comes back while the terminal-events thread holds
    // crossterm's reader (start-up failed with "The cursor position could
    // not be read"). A fresh alternate screen needs only clearing.
    execute!(term.backend_mut(), terminal::Clear(terminal::ClearType::All))?;
    app.start(target);
    let mut last_draw = Instant::now() - FRAME;
    let mut shape: Option<CursorShape> = None;
    loop {
        // Wait for something; if a frame is due, only until it is.
        let first = if app.dirty {
            events.recv_timeout(FRAME.saturating_sub(last_draw.elapsed())).ok()
        } else {
            Some(events.recv().context("lost the event threads")?)
        };
        if let Some(ev) = first {
            app.handle(ev);
            let started = Instant::now();
            while started.elapsed() < FRAME {
                match events.try_recv() {
                    Ok(ev) => app.handle(ev),
                    Err(_) => break,
                }
            }
        }
        if app.quit {
            return Ok(());
        }
        if app.dirty && last_draw.elapsed() >= FRAME {
            app.cursor_shape = None;
            term.draw(|f| app.render(f))?;
            last_draw = Instant::now();
            if app.cursor_shape != shape {
                shape = app.cursor_shape;
                let style = match shape {
                    Some(CursorShape::Beam) => SetCursorStyle::SteadyBar,
                    Some(CursorShape::Underline) => SetCursorStyle::SteadyUnderScore,
                    Some(CursorShape::Block | CursorShape::HollowBlock) => SetCursorStyle::SteadyBlock,
                    _ => SetCursorStyle::DefaultUserShape,
                };
                execute!(term.backend_mut(), style)?;
            }
        }
    }
}

fn enter() -> anyhow::Result<()> {
    terminal::enable_raw_mode().context("raw mode")?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    // Keys told apart (Ctrl+I from Tab, Esc at once) where the terminal can.
    if terminal::supports_keyboard_enhancement().unwrap_or(false) {
        execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        ENHANCED.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Puts the terminal back as it was; safe to call twice.
fn leave() {
    let mut out = stdout();
    if ENHANCED.swap(false, Ordering::Relaxed) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, SetCursorStyle::DefaultUserShape, DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen, ratatui::crossterm::cursor::Show);
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
}

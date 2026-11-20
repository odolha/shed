//! Terminal panes: an emulator per visible pane, fed by the keeper's replay
//! and then live output, drawn cell by cell; keys, pastes and the mouse
//! going to the pane's program; scrolling back through a pane.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use shed_emu::mouse::{self, Button, MouseAction};
use shed_emu::{AlacrittyEmulator, CursorShape, Emulator, GridSize, Modes, MouseMode, Palette, Snapshot};
use shed_model::{PaneId, Rgb, Tab};
use shed_proto::{ClientMsg, Frame as Wire};

use crate::app::{App, Screen};
use crate::layout;
use crate::text::Input;

/// A pane's screen as this client keeps it.
pub struct PaneEmu {
    pub emu: AlacrittyEmulator,
    palette: Palette,
    snap: Snapshot,
}

impl PaneEmu {
    pub fn new(cols: u16, rows: u16, palette: Palette) -> Self {
        PaneEmu { emu: AlacrittyEmulator::new(GridSize::new(cols, rows), palette.clone()), palette, snap: Snapshot::default() }
    }

    /// Starts again from the keeper's replay of the pane.
    pub fn restore(&mut self, cols: u16, rows: u16, bytes: &[u8]) {
        self.emu = AlacrittyEmulator::new(GridSize::new(cols, rows), self.palette.clone());
        self.feed(bytes);
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.emu.advance(bytes);
        // The keeper answers the program's queries; answering here too would answer twice.
        self.emu.take_replies();
        self.emu.take_events();
    }

    /// Ends a synchronized update that ran out of time; whether it did.
    pub fn tick(&mut self) -> bool {
        self.emu.tick().is_some()
    }

    pub fn modes(&self) -> Modes {
        self.emu.modes()
    }

    pub fn scroll_to_end(&mut self) {
        if self.emu.display_offset() > 0 {
            self.emu.scroll_to_bottom();
        }
    }
}

/// Scrolling back through a pane (`C-b [`), with a search.
pub struct ScrollMode {
    pub pane: PaneId,
    /// Typing a search (`/`).
    pub search: Option<Input>,
    pub query: Option<String>,
}

impl ScrollMode {
    pub fn describe(&self, pane: Option<&PaneEmu>) -> String {
        let at = pane.map_or(0, |p| p.emu.display_offset());
        let total = pane.map_or(0, |p| p.emu.history_size());
        match (&self.search, &self.query) {
            (Some(s), _) => format!("search: {}▏", s.text),
            (None, Some(q)) => format!("{at}/{total} · “{q}” n/N · q leaves"),
            (None, None) => format!("{at}/{total} · ↑↓ PgUp PgDn g G · / search · q leaves"),
        }
    }
}

fn rgb(c: Rgb, no_color: bool) -> Color {
    if no_color { Color::Reset } else { Color::Rgb(c.r, c.g, c.b) }
}

impl App {
    pub(crate) fn render_panes(&mut self, f: &mut Frame, area: Rect, tab: &Tab) {
        let (rects, dividers) = if self.zoomed {
            (vec![(tab.active_pane, area)], Vec::new())
        } else {
            layout::cells(&tab.layout, area)
        };
        let accent = self.current_bench().map(|b| b.color.accent()).unwrap_or(self.colors.accent);
        let active_rect = rects.iter().find(|(p, _)| *p == tab.active_pane).map(|(_, r)| *r);
        let c = self.colors.clone();
        let buf = f.buffer_mut();
        for d in &dividers {
            for y in d.area.y..d.area.y + d.area.height {
                for x in d.area.x..d.area.x + d.area.width {
                    let near = active_rect.is_some_and(|r| layout::touches(r, x, y));
                    let style = if near { c.fg(c.ink(accent)) } else { c.fg(c.divider) };
                    buf[(x, y)].set_symbol(if d.vertical { "│" } else { "─" }).set_style(style.bg(c.c(c.background)));
                }
            }
        }
        let mut cursor = None;
        for (pane, r) in &rects {
            self.hits.panes.push((*pane, *r));
            if r.width == 0 || r.height == 0 {
                continue;
            }
            let Some(p) = self.panes.get_mut(pane) else { continue };
            // The pane takes the size it is drawn at.
            let size = p.emu.size();
            if (size.cols, size.rows) != (r.width.max(2), r.height) {
                p.emu.resize(GridSize::new(r.width, r.height));
                let size = p.emu.size();
                self.conn.send_msg(ClientMsg::Resize { pane: *pane, cols: size.cols, rows: size.rows, cell_width: 0, cell_height: 0 });
            }
            p.emu.snapshot(&p.palette, &mut p.snap);
            let snap = &p.snap;
            let buf = f.buffer_mut();
            for row in 0..snap.rows.min(r.height as usize) {
                for (col, cell) in snap.row(row).iter().enumerate().take(r.width as usize) {
                    let target = &mut buf[(r.x + col as u16, r.y + row as u16)];
                    if cell.flags.spacer {
                        target.reset();
                        target.set_bg(rgb(cell.bg, c.no_color));
                        continue;
                    }
                    let mut style = Style::new().fg(rgb(cell.fg, c.no_color)).bg(rgb(cell.bg, c.no_color));
                    let fl = cell.flags;
                    let mut m = Modifier::empty();
                    if fl.bold {
                        m |= Modifier::BOLD;
                    }
                    if fl.italic {
                        m |= Modifier::ITALIC;
                    }
                    if fl.underline || fl.double_underline || fl.undercurl {
                        m |= Modifier::UNDERLINED;
                    }
                    if fl.strikeout {
                        m |= Modifier::CROSSED_OUT;
                    }
                    if fl.selected || (c.no_color && fl.has_bg) {
                        m |= Modifier::REVERSED;
                    }
                    style = style.add_modifier(m);
                    let mut sym = String::new();
                    sym.push(if cell.ch == '\0' { ' ' } else { cell.ch });
                    if let Some(z) = &cell.zerowidth {
                        sym.push_str(z);
                    }
                    target.set_symbol(&sym).set_style(style);
                }
            }
            if *pane == tab.active_pane && snap.display_offset == 0 && snap.cursor.shape != CursorShape::Hidden && self.overlay.is_none() {
                let (cx, cy) = (snap.cursor.col as u16, snap.cursor.row as u16);
                if cx < r.width && cy < r.height {
                    cursor = Some((Position::new(r.x + cx, r.y + cy), snap.cursor.shape));
                }
            }
        }
        if let Some((at, shape)) = cursor {
            f.set_cursor_position(at);
            self.cursor_shape = Some(shape);
        }
    }

    /// A key for the focused pane's program.
    pub(crate) fn pane_key(&mut self, ev: KeyEvent) {
        let Some(pane) = self.focused_pane() else { return };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if let Some(bytes) = crate::keys::encode(&ev, p.modes()) {
            p.scroll_to_end();
            self.conn.send(Wire::Input { pane, bytes });
        }
    }

    pub(crate) fn enter_scroll(&mut self) {
        if let Some(pane) = self.focused_pane().filter(|p| self.panes.contains_key(p)) {
            self.scroll = Some(ScrollMode { pane, search: None, query: None });
        }
    }

    pub(crate) fn scroll_key(&mut self, ev: KeyEvent) {
        let Some(mut s) = self.scroll.take() else { return };
        let Some(p) = self.panes.get_mut(&s.pane) else { return };
        let page = p.emu.size().rows as i32 - 1;
        if let Some(mut input) = s.search.take() {
            match ev.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let q = input.text.trim().to_string();
                    if !q.is_empty() && !p.emu.set_search(Some(&q)) {
                        self.note(format!("“{q}” is not there"));
                    }
                    s.query = Some(q).filter(|q| !q.is_empty());
                }
                _ => {
                    input.key(&ev);
                    s.search = Some(input);
                }
            }
            self.scroll = Some(s);
            return;
        }
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        match ev.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                p.emu.set_search(None);
                p.emu.scroll_to_bottom();
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => p.emu.scroll_display(1),
            KeyCode::Down | KeyCode::Char('j') => p.emu.scroll_display(-1),
            KeyCode::PageUp | KeyCode::Char('b') => p.emu.scroll_display(page),
            KeyCode::Char('u') if ctrl => p.emu.scroll_display(page / 2),
            KeyCode::Char('d') if ctrl => p.emu.scroll_display(-page / 2),
            KeyCode::PageDown | KeyCode::Char(' ') | KeyCode::Char('f') => p.emu.scroll_display(-page),
            KeyCode::Char('g') | KeyCode::Home => p.emu.scroll_display(i32::MAX / 2),
            KeyCode::Char('G') | KeyCode::End => p.emu.scroll_to_bottom(),
            KeyCode::Char('[') => {
                p.emu.jump_to_prompt(true);
            }
            KeyCode::Char(']') => {
                p.emu.jump_to_prompt(false);
            }
            KeyCode::Char('/') | KeyCode::Char('?') => s.search = Some(Input::default()),
            KeyCode::Char('n') => {
                p.emu.search_step(true);
            }
            KeyCode::Char('N') => {
                p.emu.search_step(false);
            }
            _ => {}
        }
        self.scroll = Some(s);
    }

    pub(crate) fn on_mouse(&mut self, m: MouseEvent) {
        if self.overlay.is_some() {
            if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                let code = if m.kind == MouseEventKind::ScrollUp { KeyCode::Up } else { KeyCode::Down };
                self.overlay_key(KeyEvent::new(code, KeyModifiers::NONE));
            }
            return;
        }
        if self.screen == Screen::Benches {
            self.benches_mouse(m);
            return;
        }
        let at = (m.column, m.row);
        let inside = |r: &Rect| at.0 >= r.x && at.0 < r.x + r.width && at.1 >= r.y && at.1 < r.y + r.height;
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && let Some((_, tab)) = self.hits.tabs.iter().find(|(r, _)| inside(r))
        {
            let tab = *tab;
            self.send(ClientMsg::FocusTab { tab });
            return;
        }
        let Some(tab) = self.active_tab() else { return };
        if !tab.has_panes() {
            let id = tab.id;
            match tab.kind {
                shed_model::TabKind::Ai(_) => self.ai_mouse(id, m),
                shed_model::TabKind::Project(_) => self.project_mouse(id, m),
                _ => {}
            }
            return;
        }
        let active = tab.active_pane;
        let held = self.mouse_down.map(|(p, _)| p);
        // While a button is held, its pane gets the motion, wherever it goes.
        let target = held.or_else(|| self.hits.panes.iter().find(|(_, r)| inside(r)).map(|(p, _)| *p));
        let Some(pane) = target else { return };
        let Some(rect) = self.hits.panes.iter().find(|(p, _)| *p == pane).map(|(_, r)| *r) else { return };
        let col = m.column.saturating_sub(rect.x).min(rect.width.saturating_sub(1)) as usize;
        let row = m.row.saturating_sub(rect.y).min(rect.height.saturating_sub(1)) as usize;
        let mods = crate::keys::mods(m.modifiers);
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let modes = p.modes();
        // Shift keeps the mouse for Shed (and the terminal's own selection).
        let wants = modes.mouse != MouseMode::Off && !mods.shift;
        let button = |b: MouseButton| match b {
            MouseButton::Left => Button::Left,
            MouseButton::Right => Button::Right,
            MouseButton::Middle => Button::Middle,
        };
        let report = |action| mouse::encode(action, col, row, mods, modes);
        let bytes = match m.kind {
            MouseEventKind::Down(b) => {
                if pane != active {
                    self.send(ClientMsg::FocusPane { pane });
                    return;
                }
                if !wants {
                    return;
                }
                self.mouse_down = Some((pane, button(b)));
                report(MouseAction::Press(button(b)))
            }
            MouseEventKind::Up(b) => {
                self.mouse_down = None;
                if !wants {
                    return;
                }
                report(MouseAction::Release(button(b)))
            }
            MouseEventKind::Drag(b) if wants => report(MouseAction::Move(Some(button(b)))),
            MouseEventKind::Moved if wants => report(MouseAction::Move(None)),
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if wants {
                    report(MouseAction::Press(if up { Button::WheelUp } else { Button::WheelDown }))
                } else if modes.alt_screen && modes.alternate_scroll {
                    // Full-screen programs without the mouse: arrows, as terminals send.
                    let key = KeyEvent::new(if up { KeyCode::Up } else { KeyCode::Down }, KeyModifiers::NONE);
                    crate::keys::encode(&key, modes).map(|k| k.repeat(3))
                } else {
                    p.emu.scroll_display(if up { 3 } else { -3 });
                    self.dirty = true;
                    None
                }
            }
            _ => None,
        };
        if let Some(bytes) = bytes {
            self.conn.send(Wire::Input { pane, bytes });
        }
    }
}

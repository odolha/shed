//! One pane: a client-side emulator fed by the keeper, drawn as a cell grid.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Render and paint counters, logged once a second with `SHED_PERF=1`.
pub mod perf {
    use super::*;
    pub static RENDERS: AtomicU64 = AtomicU64::new(0);
    pub static RENDER_US: AtomicU64 = AtomicU64::new(0);
    pub static PAINTS: AtomicU64 = AtomicU64::new(0);
    pub static PAINT_US: AtomicU64 = AtomicU64::new(0);
    pub static WINDOW_RENDERS: AtomicU64 = AtomicU64::new(0);
    pub static BOARD_RENDERS: AtomicU64 = AtomicU64::new(0);
    pub static BOARD_US: AtomicU64 = AtomicU64::new(0);

    pub fn enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("SHED_PERF").is_some())
    }

    pub fn report() -> String {
        let take = |a: &AtomicU64| a.swap(0, Ordering::Relaxed);
        let (r, rus, p, pus) = (take(&RENDERS), take(&RENDER_US), take(&PAINTS), take(&PAINT_US));
        let (b, bus) = (take(&BOARD_RENDERS), take(&BOARD_US));
        format!(
            "window renders {}/s, terminal renders {r}/s ({} µs avg), paints {p}/s ({} µs avg), Shed window renders {b}/s ({} µs avg)",
            take(&WINDOW_RENDERS),
            rus.checked_div(r).unwrap_or(0),
            pus.checked_div(p).unwrap_or(0),
            bus.checked_div(b).unwrap_or(0)
        )
    }
}

use gpui::{
    App, Bounds, ClipboardItem, ElementInputHandler, EntityInputHandler, UTF16Selection, Context, Entity, FocusHandle, Focusable, Subscription, Font, FontStyle, FontWeight, Hsla,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels,
    Point, ScrollWheelEvent, SharedString, StrikethroughStyle, Task, TextAlign, TextRun,
    UnderlineStyle, Window, canvas, div, point, prelude::*, px, size,
};
use shed_emu::keys::{self, KeyEvent, KeyInput, Modifier, Mods};
use shed_emu::links::{self, Link};
use shed_emu::marks::{Mark, MarkScanner, WalkState};
use shed_emu::mouse::{self, Button, MouseAction};
use shed_emu::{
    AlacrittyEmulator, CursorShape, EmuEvent, Emulator, GridPoint, GridSize, MouseMode, Palette, Rgb,
    SelectionKind, Snapshot,
};
use shed_model::{AiState, PaneId};
use shed_proto::{ClientMsg, Frame};

use crate::actions::{Copy, OpenLogbook, Paste, ScrollPageDown, ScrollPageUp, SearchScrollback, ZoomIn, ZoomOut, ZoomReset};
use crate::keeper_conn::KeeperConn;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::logbook_view::{LogbookSearch, SearchEvent};
use crate::settings::Settings;
use crate::theme::{FontSettings, UiColors, hsla, hsla_alpha};


#[derive(Clone, Copy, Debug, PartialEq)]
struct CellMetrics {
    width: Pixels,
    height: Pixels,
    font_size: Pixels,
}

pub struct TerminalView {
    pane: PaneId,
    conn: KeeperConn,
    emu: AlacrittyEmulator,
    palette: Palette,
    /// The font in use: the configured one, at its size plus `zoom`.
    font: FontSettings,
    /// The configured font size, before zoom.
    base_size: f32,
    /// The tab's zoom, in points; kept by the keeper and saved with the bench.
    pub zoom: f32,
    /// Space between the pane's edge and the grid.
    padding: f32,
    metrics: Option<(FontSettings, CellMetrics)>,
    snapshot: Snapshot,
    dirty: bool,
    focus: FocusHandle,
    /// The grid's top-left corner in window coordinates, written during prepaint.
    grid_origin: Rc<Cell<Point<Pixels>>>,
    /// Fractional lines of scroll not yet applied.
    scroll_acc: f32,
    held_button: Option<Button>,
    /// Where a plain left press landed; a selection starts only once the mouse moves.
    select_anchor: Option<(GridPoint, SelectionKind)>,
    selecting: bool,
    /// The pane's working directory, for opening relative paths.
    pub cwd: std::path::PathBuf,
    /// The pane's tab and bench, the context for logbook scopes.
    pub here: Option<shed_logbook::Context>,
    pub accent: Rgb,
    pub ui: UiColors,
    sync_timer: Option<Task<()>>,
    marks: MarkScanner,
    /// The shell is at its prompt (between OSC 133 B and C): Ctrl+R is Shed's.
    at_prompt: bool,
    /// The history walk in progress, shown as a chip by the prompt.
    walk: Option<WalkState>,
    /// The pane's AI request, shown as a chip by the prompt.
    pub ai: Option<AiState>,
    search: Option<(Entity<LogbookSearch>, Subscription)>,
    /// The scrollback search bar, while open.
    find: Option<Find>,
    /// Text an input method is composing, shown at the cursor until it commits.
    preedit: Option<String>,
    /// Keys whose press went to the app: only their releases are reported
    /// (kitty protocol), not those of keys Shed kept, like its shortcuts.
    pressed: std::collections::HashSet<String>,
    /// Modifier keys held, to tell which one a change pressed or released.
    held: gpui::Modifiers,
    /// macOS: Option sends Alt to the shell instead of typing characters.
    option_as_alt: bool,
}

struct Find {
    editor: Entity<LineEditor>,
    query: String,
    found: bool,
    _subscriptions: [Subscription; 2],
}

impl TerminalView {
    pub fn new(pane: PaneId, conn: KeeperConn, settings: &Settings, cx: &mut Context<Self>) -> Self {
        let (palette, font) = (settings.palette.clone(), settings.font.clone());
        Self {
            pane,
            conn,
            emu: AlacrittyEmulator::new(GridSize::new(80, 24), palette.clone()),
            palette,
            base_size: font.size,
            zoom: 0.0,
            font,
            padding: settings.padding,
            metrics: None,
            snapshot: Snapshot::default(),
            dirty: true,
            focus: cx.focus_handle(),
            grid_origin: Rc::new(Cell::new(Point::default())),
            scroll_acc: 0.0,
            held_button: None,
            select_anchor: None,
            selecting: false,
            cwd: std::path::PathBuf::from("/"),
            here: None,
            accent: Rgb::hex(0x5CB2FF),
            ui: settings.ui.clone(),
            marks: MarkScanner::new(),
            at_prompt: false,
            walk: None,
            ai: None,
            search: None,
            find: None,
            preedit: None,
            pressed: Default::default(),
            held: Default::default(),
            option_as_alt: settings.config.macos.option_as_alt,
            sync_timer: None,
        }
    }

    fn open_find(&mut self, _: &SearchScrollback, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(find) = &self.find {
            window.focus(&find.editor.read(cx).focus_handle(cx), cx);
            return;
        }
        // A one-line selection is the likeliest thing to look for.
        let initial = self.emu.selection_text().filter(|t| !t.contains('\n')).unwrap_or_default();
        let editor = cx.new(|cx| LineEditor::new(initial.clone(), cx));
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let typed = cx.observe(&editor, |this, editor, cx| {
            let text = editor.read(cx).text().to_string();
            let Some(find) = &mut this.find else { return };
            if text != find.query {
                find.found = this.emu.set_search(Some(&text));
                find.query = text;
                this.dirty = true;
                cx.notify();
            }
        });
        // Enter never reaches the field: the bar takes it to step (see `find_key`).
        let cancelled = cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, window, cx| {
            if let EditorEvent::Cancel = event {
                this.close_find(window, cx);
            }
        });
        let found = self.emu.set_search(Some(&initial));
        self.find = Some(Find { editor, query: initial, found, _subscriptions: [typed, cancelled] });
        self.dirty = true;
        cx.notify();
    }

    /// Ends the search where it is; typing in the pane goes back to the bottom.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        self.emu.set_search(None);
        self.dirty = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn find_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let older = match event.keystroke.key.as_str() {
            "enter" => !event.keystroke.modifiers.shift,
            "up" => true,
            "down" => false,
            _ => return,
        };
        if let Some(find) = &mut self.find {
            find.found = self.emu.search_step(older);
        }
        self.dirty = true;
        cx.stop_propagation();
        cx.notify();
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Takes new colours, font and padding from a reloaded config.
    pub fn apply(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.palette = settings.palette.clone();
        self.font = settings.font.clone();
        self.base_size = self.font.size;
        self.font.size = zoomed(self.base_size, self.zoom);
        self.padding = settings.padding;
        self.ui = settings.ui.clone();
        self.option_as_alt = settings.config.macos.option_as_alt;
        self.dirty = true;
        cx.notify();
    }

    /// Rebuilds the screen from the keeper's replay of this pane. The grid
    /// takes the keeper's size; the next layout resizes it to fit the window.
    pub fn restore(&mut self, cols: u16, rows: u16, bytes: &[u8], cx: &mut Context<Self>) {
        let size = self.emu.size();
        let size = GridSize::new(cols, rows).with_cell(size.cell_width, size.cell_height);
        self.emu = AlacrittyEmulator::new(size, self.palette.clone());
        self.marks = MarkScanner::new();
        self.feed(bytes, cx);
    }

    /// Output from the keeper.
    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let mut marks = Vec::new();
        self.marks.scan(bytes, &mut marks);
        for mark in marks {
            match mark {
                Mark::PromptStart | Mark::InputStart => self.at_prompt = true,
                // Readline redraws the prompt (and its marks) after every
                // binding, so only a running command ends the walk here.
                Mark::Executed => {
                    self.at_prompt = false;
                    self.walk = None;
                }
                Mark::Walk(w) => self.walk = Some(w),
                Mark::WalkEnd => self.walk = None,
                Mark::Finished(_) | Mark::Cwd(_) | Mark::Command(_) | Mark::ShellAliases(_) | Mark::Ask(_) => {}
            }
        }
        self.emu.advance(bytes);
        // The keeper answers queries; answering here too would answer twice.
        self.emu.take_replies();
        for event in self.emu.take_events() {
            match event {
                EmuEvent::ClipboardStore { primary: false, text } => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                EmuEvent::ClipboardStore { primary: true, .. } | EmuEvent::Title(_) | EmuEvent::Bell => {}
            }
        }
        self.schedule_sync_tick(cx);
        self.dirty = true;
        cx.notify();
    }

    /// A synchronized update holds back drawing until it ends or times out;
    /// make sure the timeout is noticed even if no more output arrives.
    fn schedule_sync_tick(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.emu.tick() else {
            self.sync_timer = None;
            return;
        };
        if self.sync_timer.is_some() {
            return;
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        self.sync_timer = Some(cx.spawn(async move |view, cx| {
            cx.background_executor().timer(wait).await;
            view.update(cx, |view, cx| {
                view.sync_timer = None;
                view.schedule_sync_tick(cx);
                view.dirty = true;
                cx.notify();
            })
            .ok();
        }));
    }

    fn send(&self, bytes: Vec<u8>) {
        self.conn.send(Frame::Input { pane: self.pane, bytes });
    }

    fn metrics(&mut self, window: &mut Window) -> CellMetrics {
        if let Some((font, m)) = &self.metrics
            && *font == self.font
        {
            return *m;
        }
        let font_size = px(self.font.size);
        let ts = window.text_system();
        let id = ts.resolve_font(&self.font.font());
        let width = ts.advance(id, font_size, 'm').map(|s| s.width).unwrap_or(font_size * 0.6);
        let height = px((self.font.size * self.font.line_height).round());
        let m = CellMetrics { width, height, font_size };
        self.metrics = Some((self.font.clone(), m));
        m
    }

    fn set_grid_size(&mut self, cols: u16, rows: u16, m: CellMetrics) {
        let size = GridSize::new(cols, rows)
            .with_cell(f32::from(m.width).round() as u16, f32::from(m.height).round() as u16);
        if size == self.emu.size() {
            return;
        }
        self.emu.resize(size);
        self.dirty = true;
        self.conn.send_msg(ClientMsg::Resize {
            pane: self.pane,
            cols: size.cols,
            rows: size.rows,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        });
    }

    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let Some((_, m)) = self.metrics else { return (0, 0) };
        let rel = position - self.grid_origin.get();
        let size = self.emu.size();
        let col = (f32::from(rel.x) / f32::from(m.width)).floor().max(0.0) as usize;
        let row = (f32::from(rel.y) / f32::from(m.height)).floor().max(0.0) as usize;
        (col.min(size.cols as usize - 1), row.min(size.rows as usize - 1))
    }

    fn grid_point(&self, position: Point<Pixels>) -> GridPoint {
        let (col, row) = self.cell_at(position);
        let right_half = match self.metrics {
            Some((_, m)) => {
                let x = f32::from(position.x - self.grid_origin.get().x) / f32::from(m.width);
                x - x.floor() >= 0.5
            }
            None => false,
        };
        GridPoint { col, row, right_half }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys an overlay's field leaves alone bubble up here; they are not the shell's.
        if !self.focus.is_focused(window) {
            return;
        }
        let ks = &event.keystroke;
        let m = ks.modifiers;
        let key = ks.key.as_str();

        // Escape stops an AI request under way.
        if key == "escape" && matches!(self.ai, Some(AiState::Asking(_))) {
            self.conn.send_msg(ClientMsg::CancelAsk { pane: self.pane });
            cx.stop_propagation();
            return;
        }

        // Typing anything but the walk keys ends the walk, as far as the chip goes.
        if self.walk.is_some() && !(m.control && matches!(key, "up" | "down")) {
            self.walk = None;
            cx.notify();
        }

        // On macOS Option types characters (ƒ for Option+F, @ on many layouts),
        // unless it is set to be Alt; then the key's own character goes, Alt'd.
        let mac_option = cfg!(target_os = "macos") && m.alt && key.chars().count() == 1;
        let alt_text = (mac_option && self.option_as_alt).then(|| if m.shift { key.to_uppercase() } else { key.to_string() });
        let option_types = mac_option && !self.option_as_alt && ks.key_char.is_some();
        let mods = Mods { ctrl: m.control, alt: m.alt && !option_types, shift: m.shift, super_: m.platform };
        let text = alt_text.as_deref().or(ks.key_char.as_deref());
        let input = KeyInput { key, text, mods };
        let kind = if event.is_held { KeyEvent::Repeat } else { KeyEvent::Press };
        if let Some(bytes) = keys::encode_event(input, kind, self.emu.modes()) {
            if self.emu.display_offset() != 0 {
                self.emu.scroll_to_bottom();
                self.dirty = true;
                cx.notify();
            }
            self.send(bytes);
            self.pressed.insert(key.to_string());
            cx.stop_propagation();
        }
    }

    /// Releases, for apps that asked the kitty protocol for them.
    fn on_key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        if !self.focus.is_focused(window) || !self.pressed.remove(&ks.key) {
            return;
        }
        let m = ks.modifiers;
        let mods = Mods { ctrl: m.control, alt: m.alt, shift: m.shift, super_: m.platform };
        let input = KeyInput { key: &ks.key, text: None, mods };
        if let Some(bytes) = keys::encode_event(input, KeyEvent::Release, self.emu.modes()) {
            self.send(bytes);
            cx.stop_propagation();
        }
    }

    /// Modifier keys alone, for apps that asked the kitty protocol for every key.
    fn on_modifiers_changed(&mut self, event: &gpui::ModifiersChangedEvent, window: &mut Window, _: &mut Context<Self>) {
        let (before, after) = (std::mem::replace(&mut self.held, event.modifiers), event.modifiers);
        if !self.focus.is_focused(window) {
            return;
        }
        let mods = Mods { ctrl: after.control, alt: after.alt, shift: after.shift, super_: after.platform };
        let changes = [
            (Modifier::Shift, before.shift, after.shift),
            (Modifier::Ctrl, before.control, after.control),
            (Modifier::Alt, before.alt, after.alt),
            (Modifier::Super, before.platform, after.platform),
        ];
        for (key, was, is) in changes {
            let event = match (was, is) {
                (false, true) => KeyEvent::Press,
                (true, false) => KeyEvent::Release,
                _ => continue,
            };
            if let Some(bytes) = keys::encode_modifier(key, event, mods, self.emu.modes()) {
                self.send(bytes);
            }
        }
    }

    fn open_logbook(&mut self, _: &OpenLogbook, window: &mut Window, cx: &mut Context<Self>) {
        let Some(here) = self.here else { return };
        let (ui, accent, mono) = (self.ui.clone(), self.accent, SharedString::from(self.font.family.clone()));
        let search = cx.new(|cx| LogbookSearch::new(here, crate::logbook::path(), ui, accent, mono, cx));
        window.focus(&search.read(cx).focus_handle(cx), cx);
        let sub = cx.subscribe_in(&search, window, |this, _, event: &SearchEvent, window, cx| {
            if let SearchEvent::Insert { command, run } = event {
                // Clear the line (Ctrl+E, Ctrl+U), then paste, so nothing typed runs by accident.
                let mut bytes = b"\x05\x15".to_vec();
                bytes.extend(keys::encode_paste(command, this.emu.modes()));
                if *run {
                    bytes.push(b'\r');
                }
                this.send(bytes);
            }
            this.search = None;
            window.focus(&this.focus, cx);
            cx.notify();
        });
        self.search = Some((search, sub));
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.emu.scroll_to_bottom();
        self.dirty = true;
        self.send(keys::encode_paste(text, self.emu.modes()));
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.emu.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Zooms the whole tab: the keeper keeps the zoom and saves it with the bench.
    fn zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        let zoom = if delta == 0.0 { 0.0 } else { zoomed(self.base_size, self.zoom + delta) - self.base_size };
        self.set_zoom(zoom, cx);
        if let Some(here) = self.here {
            self.conn.send_msg(ClientMsg::SetZoom { tab: here.tab, zoom });
        }
    }

    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.font.size = zoomed(self.base_size, zoom);
        self.dirty = true;
        cx.notify();
    }

    fn scroll_page(&mut self, up: bool, cx: &mut Context<Self>) {
        if self.emu.modes().alt_screen {
            cx.propagate();
            return;
        }
        let page = self.emu.size().rows as i32 - 1;
        self.emu.scroll_display(if up { page } else { -page });
        self.dirty = true;
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, m)) = self.metrics else { return };
        self.scroll_acc += f32::from(event.delta.pixel_delta(m.height).y) / f32::from(m.height);
        let lines = self.scroll_acc.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_acc -= lines as f32;
        let modes = self.emu.modes();
        let mods = mods_of(event.modifiers);

        if modes.mouse != MouseMode::Off && !event.modifiers.shift {
            let (col, row) = self.cell_at(event.position);
            let button = if lines > 0 { Button::WheelUp } else { Button::WheelDown };
            let mut bytes = Vec::new();
            for _ in 0..lines.abs() {
                if let Some(b) = mouse::encode(MouseAction::Press(button), col, row, mods, modes) {
                    bytes.extend(b);
                }
            }
            self.send(bytes);
        } else if modes.alt_screen && modes.alternate_scroll {
            // Apps without mouse support (less, man) get arrow keys instead.
            let key = if lines > 0 { "up" } else { "down" };
            let mut bytes = Vec::new();
            for _ in 0..lines.abs() {
                bytes.extend(keys::encode(KeyInput { key, text: None, mods: Mods::NONE }, modes).unwrap());
            }
            self.send(bytes);
        } else {
            self.emu.scroll_display(lines);
            self.dirty = true;
            cx.notify();
        }
    }

    fn mouse_report(&mut self, action: MouseAction, position: Point<Pixels>, modifiers: gpui::Modifiers) -> bool {
        let modes = self.emu.modes();
        // Shift always belongs to Shed, so text stays selectable inside mouse apps.
        if modes.mouse == MouseMode::Off || modifiers.shift {
            return false;
        }
        let (col, row) = self.cell_at(position);
        if let Some(bytes) = mouse::encode(action, col, row, mods_of(modifiers), modes) {
            self.send(bytes);
        }
        true
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
            self.conn.send_msg(ClientMsg::FocusPane { pane: self.pane });
        }
        let Some(button) = button_of(event.button) else { return };
        self.held_button = Some(button);
        if self.mouse_report(MouseAction::Press(button), event.position, event.modifiers) {
            // The app has the mouse; Shed's menus must not open over it.
            cx.stop_propagation();
            return;
        }
        let at = self.grid_point(event.position);
        match button {
            Button::Left if event.modifiers.control => self.open_link(at, cx),
            Button::Left => {
                let kind = match event.click_count {
                    1 if event.modifiers.alt => SelectionKind::Block,
                    1 => SelectionKind::Simple,
                    2 => SelectionKind::Word,
                    _ => SelectionKind::Line,
                };
                self.emu.clear_selection();
                if kind == SelectionKind::Word || kind == SelectionKind::Line {
                    self.emu.start_selection(kind, at);
                    self.selecting = true;
                } else {
                    self.select_anchor = Some((at, kind));
                }
                self.dirty = true;
                cx.notify();
            }
            // X11 and Wayland convention: middle-click pastes the primary selection.
            #[cfg(target_os = "linux")]
            Button::Middle => {
                if let Some(text) = cx.read_from_primary().and_then(|item| item.text()) {
                    self.emu.scroll_to_bottom();
                    self.send(keys::encode_paste(&text, self.emu.modes()));
                }
            }
            _ => {}
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(button) = button_of(event.button) else { return };
        self.held_button = None;
        self.select_anchor = None;
        if self.mouse_report(MouseAction::Release(button), event.position, event.modifiers) {
            return;
        }
        let selected = std::mem::take(&mut self.selecting);
        // X11 convention: selecting copies to the primary selection.
        #[cfg(target_os = "linux")]
        if selected && let Some(text) = self.emu.selection_text() {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (selected, &cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let held = event.pressed_button.and_then(button_of).or(self.held_button.filter(|_| event.pressed_button.is_some()));
        if self.mouse_report(MouseAction::Move(held), event.position, event.modifiers) {
            return;
        }
        if held != Some(Button::Left) {
            return;
        }
        if let Some((anchor, kind)) = self.select_anchor.take() {
            self.emu.start_selection(kind, anchor);
            self.selecting = true;
        }
        if !self.selecting {
            return;
        }
        // Dragging past the top or bottom edge scrolls.
        if let Some((_, m)) = self.metrics {
            let y = f32::from(event.position.y - self.grid_origin.get().y);
            let height = f32::from(m.height) * self.emu.size().rows as f32;
            if y < 0.0 {
                self.emu.scroll_display(1);
            } else if y > height {
                self.emu.scroll_display(-1);
            }
        }
        self.emu.update_selection(self.grid_point(event.position));
        self.dirty = true;
        cx.notify();
    }

    /// Opens the hyperlink, URL or file path under a Ctrl+click.
    fn open_link(&mut self, at: GridPoint, cx: &mut Context<Self>) {
        let target = self.emu.hyperlink_at(at.col, at.row).or_else(|| {
            let (text, cols) = self.emu.row_text(at.row);
            let index = cols.iter().rposition(|&c| c <= at.col)?;
            match links::link_at(&text, index)? {
                Link::Url(url) => Some(url),
                Link::Path(path) => {
                    let expanded = match path.strip_prefix("~/") {
                        Some(rest) => std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(rest))?,
                        None => self.cwd.join(&path),
                    };
                    expanded.exists().then(|| format!("file://{}", expanded.display()))
                }
            }
        });
        if let Some(url) = target {
            cx.open_url(&url);
        }
    }
}

fn mods_of(m: gpui::Modifiers) -> Mods {
    Mods { ctrl: m.control, alt: m.alt, shift: m.shift, super_: m.platform }
}

fn button_of(b: MouseButton) -> Option<Button> {
    match b {
        MouseButton::Left => Some(Button::Left),
        MouseButton::Middle => Some(Button::Middle),
        MouseButton::Right => Some(Button::Right),
        _ => None,
    }
}

/// What a frame draws, in grid coordinates, prepared during render.
struct GridFrame {
    background: Rgb,
    rects: Vec<Rect>,
    texts: Vec<TextBatch>,
    cursor: Option<CursorPaint>,
}

struct Rect {
    row: usize,
    col: usize,
    cols: usize,
    color: Rgb,
}

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

struct TextBatch {
    row: usize,
    col: usize,
    text: String,
    style: Style,
    underline_color: Option<Rgb>,
    /// Drawn with the emoji font, in colour.
    emoji: bool,
}

struct CursorPaint {
    row: usize,
    col: usize,
    /// Width in cells: 2 over a double-width character.
    cols: usize,
    shape: CursorShape,
    color: Rgb,
    /// The character under a block cursor, redrawn in the cursor-text colour.
    under: Option<TextBatch>,
}

fn build_frame(snap: &Snapshot, palette: &Palette, focused: bool) -> GridFrame {
    let mut rects = Vec::new();
    let mut texts = Vec::new();

    for row in 0..snap.rows {
        let cells = snap.row(row);

        // Backgrounds: merge neighbouring cells of the same non-default colour.
        let mut col = 0;
        while col < cells.len() {
            let cell = &cells[col];
            if !cell.flags.has_bg {
                col += 1;
                continue;
            }
            let start = col;
            while col < cells.len() && cells[col].flags.has_bg && cells[col].bg == cell.bg {
                col += 1;
            }
            rects.push(Rect { row, col: start, cols: col - start, color: cell.bg });
        }

        // Text: ASCII runs of one style are shaped together; anything else is
        // placed cell by cell so fallback fonts cannot push the grid out of line.
        let mut current: Option<TextBatch> = None;
        for (col, cell) in cells.iter().enumerate() {
            if cell.flags.spacer {
                continue;
            }
            let style = Style {
                fg: cell.fg,
                bold: cell.flags.bold,
                italic: cell.flags.italic,
                underline: cell.flags.underline || cell.flags.double_underline || cell.flags.undercurl,
                strike: cell.flags.strikeout,
            };
            let ascii = cell.ch.is_ascii() && cell.zerowidth.is_none() && !cell.flags.wide;
            let blank = cell.ch == ' ' && !style.underline && !style.strike;
            if let Some(batch) = &mut current
                && ascii
                && batch.style == style
                && batch.underline_color == cell.underline_color
                && batch.col + batch.text.len() == col
            {
                batch.text.push(cell.ch);
                continue;
            }
            if let Some(batch) = current.take() {
                texts.push(batch);
            }
            if blank {
                continue;
            }
            let mut text = String::from(cell.ch);
            if let Some(z) = &cell.zerowidth {
                text.push_str(z);
            }
            let emoji = !ascii && crate::emoji::is_emoji(cell.ch, cell.zerowidth.as_deref());
            let batch = TextBatch { row, col, text, style, underline_color: cell.underline_color, emoji };
            if ascii {
                current = Some(batch);
            } else {
                texts.push(batch);
            }
        }
        if let Some(batch) = current.take() {
            texts.push(batch);
        }
    }
    for batch in texts.iter_mut().filter(|b| !b.style.underline && !b.style.strike) {
        let trimmed = batch.text.trim_end_matches(' ').len();
        batch.text.truncate(trimmed.max(1));
    }

    let c = snap.cursor;
    let cursor = (c.shape != CursorShape::Hidden && snap.rows > 0).then(|| {
        let shape = if focused { c.shape } else { CursorShape::HollowBlock };
        let under = (shape == CursorShape::Block).then(|| {
            let cell = &snap.row(c.row)[c.col];
            let mut text = String::from(cell.ch);
            if let Some(z) = &cell.zerowidth {
                text.push_str(z);
            }
            let emoji = crate::emoji::is_emoji(cell.ch, cell.zerowidth.as_deref());
            TextBatch {
                emoji,
                row: c.row,
                col: c.col,
                text,
                style: Style {
                    fg: palette.cursor_text,
                    bold: cell.flags.bold,
                    italic: cell.flags.italic,
                    underline: false,
                    strike: false,
                },
                underline_color: None,
            }
        });
        CursorPaint { row: c.row, col: c.col, cols: if c.wide { 2 } else { 1 }, shape, color: palette.cursor, under }
    });

    GridFrame { background: snap.background, rects, texts, cursor }
}

fn paint_frame(frame: &GridFrame, origin: Point<Pixels>, m: CellMetrics, font: &Font, emoji_font: &Font, window: &mut Window, cx: &mut App) {
    let at = |row: usize, col: usize| point(origin.x + m.width * col as f32, origin.y + m.height * row as f32);

    for r in &frame.rects {
        let bounds = Bounds::new(at(r.row, r.col), size(m.width * r.cols as f32, m.height));
        window.paint_quad(PaintQuad::new(bounds).background(hsla(r.color)));
    }

    let paint_text = |batch: &TextBatch, window: &mut Window, cx: &mut App| {
        let style = batch.style;
        let color = hsla(style.fg);
        let mut font = if batch.emoji { emoji_font.clone() } else { font.clone() };
        if style.bold && !batch.emoji {
            font.weight = FontWeight::BOLD;
        }
        if style.italic && !batch.emoji {
            font.style = FontStyle::Italic;
        }
        let run = TextRun {
            len: batch.text.len(),
            font,
            color,
            background_color: None,
            underline: style.underline.then(|| UnderlineStyle {
                thickness: px(1.0),
                color: Some(batch.underline_color.map(hsla).unwrap_or(color)),
                wavy: false,
            }),
            strikethrough: style.strike.then(|| StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
            letter_spacing: None,
        };
        let text: SharedString = batch.text.clone().into();
        let line = window.text_system().shape_line(text, m.font_size, &[run]);
        let _ = line.paint(at(batch.row, batch.col), m.height, TextAlign::Left, None, window, cx);
    };

    for batch in &frame.texts {
        paint_text(batch, window, cx);
    }

    if let Some(c) = &frame.cursor {
        let origin = at(c.row, c.col);
        let width = m.width * c.cols as f32;
        let color = hsla(c.color);
        match c.shape {
            CursorShape::Block => {
                window.paint_quad(PaintQuad::new(Bounds::new(origin, size(width, m.height))).background(color));
                if let Some(under) = &c.under {
                    paint_text(under, window, cx);
                }
            }
            CursorShape::HollowBlock => {
                window.paint_quad(
                    PaintQuad::new(Bounds::new(origin, size(width, m.height)))
                        .border_widths(px(1.0))
                        .border_color(color),
                );
            }
            CursorShape::Beam => {
                window.paint_quad(PaintQuad::new(Bounds::new(origin, size(px(2.0), m.height))).background(color));
            }
            CursorShape::Underline => {
                let y = origin.y + m.height - px(2.0);
                window.paint_quad(PaintQuad::new(Bounds::new(point(origin.x, y), size(width, px(2.0)))).background(color));
            }
            CursorShape::Hidden => {}
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = Instant::now();
        let m = self.metrics(window);
        if self.dirty {
            self.emu.snapshot(&self.palette, &mut self.snapshot);
            self.dirty = false;
        }
        let focused = self.focus.is_focused(window);
        let frame = build_frame(&self.snapshot, &self.palette, focused);
        let background: Hsla = hsla(frame.background);
        let font = self.font.font();
        let emoji_font = gpui::font(self.font.emoji.clone());
        let view = cx.entity().downgrade();
        let input = (cx.entity(), self.focus.clone());
        let (font_for_overlay, background_overlay, fg_overlay) = (self.font.font(), hsla(self.palette.background), self.palette.foreground);
        let preedit = self.preedit.clone().map(|text| {
            let c = &self.snapshot.cursor;
            (px(self.padding) + m.width * c.col as f32, px(self.padding) + m.height * c.row as f32, text)
        });
        let grid_origin = self.grid_origin.clone();
        let padding = self.padding;
        let accent = self.accent;
        // The walk chip, or the AI's, sits at the right end of the prompt line.
        let top = px(padding) + m.height * self.snapshot.cursor.row as f32;
        let ai_chip = self.ai.as_ref().map(|state| {
            let (label, color) = match state {
                AiState::Asking(request) => (format!("asking the AI: {} · Esc stops", short(request, 40)), accent),
                AiState::Failed(why) => (format!("AI: {}", short(why, 80)), self.ui.danger),
            };
            (top, label, color)
        });
        let chip = ai_chip.or_else(|| {
            self.walk.as_ref().map(|w| {
                let mut label = format!("{} {}/{}", w.scope, w.pos, w.total);
                if !w.source.is_empty() {
                    label.push_str(&format!(" · {}", w.source));
                }
                (top, label, accent)
            })
        });
        if perf::enabled() {
            perf::RENDERS.fetch_add(1, Ordering::Relaxed);
            perf::RENDER_US.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        }

        div()
            .id("terminal")
            .relative()
            .key_context(if self.at_prompt { "Terminal at_prompt" } else { "Terminal" })
            .track_focus(&self.focus)
            .size_full()
            // The window paints the theme background (cleared by the renderer
            // at no cost); a pane paints its own only if the app changed it.
            .when(frame.background != self.palette.background, |d| d.bg(background))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::open_logbook))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(|v, _: &ZoomIn, _, cx| v.zoom(1.0, cx)))
            .on_action(cx.listener(|v, _: &ZoomOut, _, cx| v.zoom(-1.0, cx)))
            .on_action(cx.listener(|v, _: &ZoomReset, _, cx| v.zoom(0.0, cx)))
            .on_action(cx.listener(|v, _: &ScrollPageUp, _, cx| v.scroll_page(true, cx)))
            .on_action(cx.listener(|v, _: &ScrollPageDown, _, cx| v.scroll_page(false, cx)))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(
                canvas(
                    move |bounds, _window, cx| {
                        let origin = point(bounds.origin.x + px(padding), bounds.origin.y + px(padding));
                        grid_origin.set(origin);
                        let cols = ((f32::from(bounds.size.width) - 2.0 * padding) / f32::from(m.width)).floor();
                        let rows = ((f32::from(bounds.size.height) - 2.0 * padding) / f32::from(m.height)).floor();
                        let (cols, rows) = (cols.max(2.0) as u16, rows.max(1.0) as u16);
                        view.update(cx, |view, cx| {
                            let before = view.emu.size();
                            view.set_grid_size(cols, rows, m);
                            if view.emu.size() != before {
                                cx.notify();
                            }
                        })
                        .ok();
                        origin
                    },
                    move |bounds, origin, window, cx| {
                        let t = Instant::now();
                        // Input methods: composed text arrives through this, not as keys.
                        let (entity, focus) = input;
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                        paint_frame(&frame, origin, m, &font, &emoji_font, window, cx);
                        if perf::enabled() {
                            perf::PAINTS.fetch_add(1, Ordering::Relaxed);
                            perf::PAINT_US.fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                        }
                    },
                )
                .size_full(),
            )
            .when_some(preedit, |el, (left, top, text)| {
                el.child(
                    div()
                        .absolute()
                        .left(left)
                        .top(top)
                        .h(m.height)
                        .flex()
                        .items_center()
                        .font(font_for_overlay.clone())
                        .text_size(m.font_size)
                        .bg(background_overlay)
                        .text_color(hsla(fg_overlay))
                        .underline()
                        .child(text),
                )
            })
            .when_some(chip, |el, (top, label, color)| {
                el.child(
                    div()
                        .absolute()
                        .top(top)
                        .right(px(padding + 4.0))
                        .px_1p5()
                        .rounded_sm()
                        .text_size(px(12.0))
                        .bg(hsla_alpha(color, 0.22))
                        .text_color(hsla(self.ui.ink(color)))
                        .child(label),
                )
            })
            .when_some(self.search.as_ref().map(|(s, _)| s.clone()), |el, search| {
                el.child(div().absolute().bottom_0().left_0().w_full().child(search))
            })
            .when_some(self.find.as_ref().map(|f| (f.editor.clone(), f.found, f.query.is_empty())), |el, (editor, found, empty)| {
                let ui = &self.ui;
                let hint = match (empty, found) {
                    (true, _) => "find in scrollback",
                    (false, true) => "Enter older · Shift+Enter newer · Esc",
                    (false, false) => "no matches",
                };
                el.child(
                    div()
                        .absolute()
                        .top(px(4.0))
                        .right(px(padding + 8.0))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(hsla(ui.tab_active))
                        .border_1()
                        .border_color(hsla(ui.divider))
                        .text_size(px(12.0))
                        .capture_key_down(cx.listener(Self::find_key))
                        .child(div().w(px(220.0)).text_color(hsla(ui.text)).child(editor))
                        .child(div().text_color(hsla(if !empty && !found { ui.danger } else { ui.text_dim })).child(hint)),
                )
            })
    }
}

/// Input methods (IBus, Fcitx, dead keys): the composed text goes to the
/// shell like typing; while composing, it shows at the cursor.
impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, _: std::ops::Range<usize>, _: &mut Option<std::ops::Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.preedit.as_ref().map(|p| 0..p.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<std::ops::Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit = None;
        if !text.is_empty() {
            if self.emu.display_offset() != 0 {
                self.emu.scroll_to_bottom();
                self.dirty = true;
            }
            self.send(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn paste(&mut self, item: ClipboardItem, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.paste_text(&text, cx);
        }
    }

    /// The cursor's cell, so the candidate window opens next to it.
    fn bounds_for_range(&mut self, _: std::ops::Range<usize>, _: Bounds<Pixels>, window: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let m = self.metrics(window);
        let origin = self.grid_origin.get();
        let c = &self.snapshot.cursor;
        Some(Bounds::new(point(origin.x + m.width * c.col as f32, origin.y + m.height * c.row as f32), size(m.width, m.height)))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// `text` on one line, cut to `max` characters.
fn short(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    format!("{}…", line.chars().take(max - 1).collect::<String>())
}

/// A font size `zoom` points from `base`, within what stays readable.
fn zoomed(base: f32, zoom: f32) -> f32 {
    (base + zoom).clamp(6.0, 72.0)
}

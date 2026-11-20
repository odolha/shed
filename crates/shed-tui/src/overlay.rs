//! What opens over the screen: a field to type in, a question, a menu,
//! text to read, the logbook search. Each holds what happens when it is
//! answered, as a closure over the app.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use shed_model::Rgb;

use crate::app::App;
use crate::text::{Input, centered, fit, pad, width};
use crate::theme::Colors;

pub type OnText = Box<dyn FnOnce(&mut App, String)>;
pub type OnPick = Box<dyn FnOnce(&mut App, usize)>;
pub type OnYes = Box<dyn FnOnce(&mut App)>;
/// A tab menu's entry: what it says, and what it does to the tab.
pub type TabAction = (Item, fn(&mut App, shed_model::TabId));

pub enum Overlay {
    Input(Ask),
    Confirm(Confirm),
    Menu(Menu),
    Info(Info),
    Logbook(crate::logbook::Search),
}

pub struct Ask {
    pub title: String,
    pub hint: String,
    pub input: Input,
    pub on_submit: OnText,
}

pub struct Confirm {
    pub title: String,
    pub text: Vec<String>,
    /// The button: "Close", "Discard", …
    pub yes: String,
    pub danger: bool,
    pub on_yes: OnYes,
}

pub struct Info {
    pub title: String,
    pub lines: Vec<Line<'static>>,
    pub scroll: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub label: String,
    /// Shown after the label, dimmed: a key, a state.
    pub hint: String,
    pub danger: bool,
    /// A colour swatch before the label.
    pub swatch: Option<Rgb>,
}

impl Item {
    pub fn new(label: impl Into<String>) -> Self {
        Item { label: label.into(), ..Default::default() }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn swatch(mut self, c: Rgb) -> Self {
        self.swatch = Some(c);
        self
    }
}

pub struct Menu {
    pub title: String,
    pub items: Vec<Item>,
    pub sel: usize,
    /// Typing narrows the items down.
    pub filter: Input,
    pub on_pick: OnPick,
}

impl Menu {
    /// The items the filter lets through, by their place in `items`.
    fn shown(&self) -> Vec<usize> {
        let words: Vec<String> = self.filter.text.split_whitespace().map(str::to_lowercase).collect();
        (0..self.items.len()).filter(|i| words.iter().all(|w| self.items[*i].label.to_lowercase().contains(w))).collect()
    }
}

impl App {
    pub fn ask(&mut self, title: impl Into<String>, initial: impl Into<String>, on_submit: impl FnOnce(&mut App, String) + 'static) {
        self.overlay = Some(Overlay::Input(Ask { title: title.into(), hint: String::new(), input: Input::new(initial), on_submit: Box::new(on_submit) }));
    }

    pub fn ask_with_hint(&mut self, title: impl Into<String>, hint: impl Into<String>, initial: impl Into<String>, on_submit: impl FnOnce(&mut App, String) + 'static) {
        self.overlay = Some(Overlay::Input(Ask { title: title.into(), hint: hint.into(), input: Input::new(initial), on_submit: Box::new(on_submit) }));
    }

    pub fn confirm(&mut self, title: impl Into<String>, text: impl Into<String>, yes: impl Into<String>, danger: bool, on_yes: impl FnOnce(&mut App) + 'static) {
        let text: String = text.into();
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: title.into(),
            text: text.lines().map(String::from).collect(),
            yes: yes.into(),
            danger,
            on_yes: Box::new(on_yes),
        }));
    }

    pub fn menu(&mut self, title: impl Into<String>, items: Vec<Item>, on_pick: impl FnOnce(&mut App, usize) + 'static) {
        self.overlay = Some(Overlay::Menu(Menu { title: title.into(), items, sel: 0, filter: Input::default(), on_pick: Box::new(on_pick) }));
    }

    pub fn info(&mut self, title: impl Into<String>, lines: Vec<Line<'static>>) {
        self.overlay = Some(Overlay::Info(Info { title: title.into(), lines, scroll: 0 }));
    }

    /// A key while something is open over the screen.
    pub(crate) fn overlay_key(&mut self, ev: KeyEvent) {
        let Some(overlay) = self.overlay.take() else { return };
        match overlay {
            Overlay::Input(mut a) => match ev.code {
                KeyCode::Esc => {}
                KeyCode::Enter => (a.on_submit)(self, a.input.text),
                _ => {
                    a.input.key(&ev);
                    self.overlay = Some(Overlay::Input(a));
                }
            },
            Overlay::Confirm(c) => match ev.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => (c.on_yes)(self),
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {}
                _ => self.overlay = Some(Overlay::Confirm(c)),
            },
            Overlay::Menu(mut m) => {
                let shown = m.shown();
                match ev.code {
                    KeyCode::Esc => {}
                    KeyCode::Enter => {
                        if let Some(&i) = shown.get(m.sel) {
                            (m.on_pick)(self, i);
                        } else {
                            self.overlay = Some(Overlay::Menu(m));
                        }
                    }
                    KeyCode::Up => {
                        m.sel = m.sel.checked_sub(1).unwrap_or(shown.len().saturating_sub(1));
                        self.overlay = Some(Overlay::Menu(m));
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        m.sel = if m.sel + 1 >= shown.len() { 0 } else { m.sel + 1 };
                        self.overlay = Some(Overlay::Menu(m));
                    }
                    KeyCode::Char('p') if ev.modifiers.contains(KeyModifiers::CONTROL) => {
                        m.sel = m.sel.saturating_sub(1);
                        self.overlay = Some(Overlay::Menu(m));
                    }
                    KeyCode::Char('n') if ev.modifiers.contains(KeyModifiers::CONTROL) => {
                        m.sel = (m.sel + 1).min(shown.len().saturating_sub(1));
                        self.overlay = Some(Overlay::Menu(m));
                    }
                    _ => {
                        m.filter.key(&ev);
                        m.sel = 0;
                        self.overlay = Some(Overlay::Menu(m));
                    }
                }
            }
            Overlay::Info(mut i) => match ev.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {}
                KeyCode::Up | KeyCode::Char('k') => {
                    i.scroll = i.scroll.saturating_sub(1);
                    self.overlay = Some(Overlay::Info(i));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    i.scroll = (i.scroll + 1).min(i.lines.len().saturating_sub(1));
                    self.overlay = Some(Overlay::Info(i));
                }
                KeyCode::PageUp => {
                    i.scroll = i.scroll.saturating_sub(10);
                    self.overlay = Some(Overlay::Info(i));
                }
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    i.scroll = (i.scroll + 10).min(i.lines.len().saturating_sub(1));
                    self.overlay = Some(Overlay::Info(i));
                }
                _ => self.overlay = Some(Overlay::Info(i)),
            },
            Overlay::Logbook(s) => self.logbook_key(s, ev),
        }
    }

    pub(crate) fn render_overlay(&mut self, f: &mut Frame, area: Rect) {
        let colors = self.colors.clone();
        let Some(overlay) = self.overlay.as_mut() else { return };
        match overlay {
            Overlay::Input(a) => {
                let w = (area.width.saturating_sub(4)).clamp(20, 76);
                let inner_w = w.saturating_sub(2) as usize;
                let (rows, (crow, ccol)) = a.input.rows(inner_w.saturating_sub(1));
                let extra = if a.hint.is_empty() { 0 } else { 1 };
                let h = (rows.len() as u16 + 2 + extra).min(area.height);
                let r = centered(area, w, h);
                let block = frame_block(&colors, &a.title, false);
                let inner = block.inner(r);
                f.render_widget(Clear, r);
                f.render_widget(block, r);
                let mut lines: Vec<Line> = rows.iter().map(|row| Line::raw(row.clone())).collect();
                if extra == 1 {
                    lines.push(Line::styled(fit(&a.hint, inner_w), colors.dim()));
                }
                f.render_widget(Paragraph::new(lines).style(colors.base()), inner);
                let top = rows.len().saturating_sub(inner.height as usize - extra as usize);
                if crow >= top {
                    f.set_cursor_position((inner.x + ccol as u16, inner.y + (crow - top) as u16));
                }
            }
            Overlay::Confirm(c) => {
                let longest = c.text.iter().map(|l| width(l)).max().unwrap_or(0).max(width(&c.title) + 4).max(30);
                let w = (longest as u16 + 4).min(area.width.saturating_sub(2)).min(80);
                let inner_w = w.saturating_sub(2) as usize;
                let mut lines: Vec<Line> = Vec::new();
                for l in &c.text {
                    lines.extend(crate::text::wrap(vec![Span::raw(l.clone())], inner_w, vec![], vec![]));
                }
                lines.push(Line::default());
                let yes_style = if c.danger { colors.chip(colors.danger) } else { colors.chip(colors.accent) };
                lines.push(Line::from(vec![
                    Span::styled(format!(" {} ", c.yes), yes_style),
                    Span::raw("  "),
                    Span::styled("Enter / y", colors.dim()),
                    Span::raw("    "),
                    Span::styled(" Cancel ", colors.selected()),
                    Span::raw("  "),
                    Span::styled("Esc / n", colors.dim()),
                ]));
                let h = (lines.len() as u16 + 2).min(area.height);
                let r = centered(area, w, h);
                let block = frame_block(&colors, &c.title, c.danger);
                let inner = block.inner(r);
                f.render_widget(Clear, r);
                f.render_widget(block, r);
                f.render_widget(Paragraph::new(lines).style(colors.base()), inner);
            }
            Overlay::Menu(m) => {
                let shown = m.shown();
                let label_w = m.items.iter().map(|i| width(&i.label) + if i.swatch.is_some() { 3 } else { 0 }).max().unwrap_or(10);
                let hint_w = m.items.iter().map(|i| width(&i.hint)).max().unwrap_or(0);
                let w = ((label_w + hint_w + 6).max(width(&m.title) + 6).max(24) as u16).min(area.width.saturating_sub(2));
                let filter_row = u16::from(!m.filter.text.is_empty());
                let h = ((shown.len() as u16).max(1) + 2 + filter_row).min(area.height);
                let r = centered(area, w, h);
                let block = frame_block(&colors, &m.title, false);
                let inner = block.inner(r);
                f.render_widget(Clear, r);
                f.render_widget(block, r);
                let rows = (inner.height - filter_row) as usize;
                m.sel = m.sel.min(shown.len().saturating_sub(1));
                let top = m.sel.saturating_sub(rows.saturating_sub(1));
                let iw = inner.width as usize;
                let mut lines: Vec<Line> = Vec::new();
                if filter_row == 1 {
                    lines.push(Line::from(vec![Span::styled("/ ", colors.dim()), Span::raw(m.filter.text.clone())]));
                }
                if shown.is_empty() {
                    lines.push(Line::styled("nothing matches", colors.dim()));
                }
                for (n, &i) in shown.iter().enumerate().skip(top).take(rows) {
                    let item = &m.items[i];
                    let mut spans = vec![Span::raw(" ")];
                    let mut used = 1;
                    if let Some(c) = item.swatch {
                        spans.push(Span::styled("██", colors.fg(c)));
                        spans.push(Span::raw(" "));
                        used += 3;
                    }
                    let hint = if item.hint.is_empty() { String::new() } else { format!(" {}", item.hint) };
                    let label = pad(&item.label, iw.saturating_sub(used + width(&hint)));
                    let style = if item.danger { colors.danger() } else { Style::new() };
                    spans.push(Span::styled(label, style));
                    spans.push(Span::styled(hint, colors.dim()));
                    let mut line = Line::from(spans);
                    if n == m.sel {
                        line = line.style(colors.selected());
                    }
                    lines.push(line);
                }
                f.render_widget(Paragraph::new(lines).style(colors.base()), inner);
            }
            Overlay::Info(i) => {
                let w = area.width.saturating_sub(4).min(100);
                let h = (i.lines.len() as u16 + 2).clamp(5, area.height.saturating_sub(2).max(5));
                let r = centered(area, w, h);
                let block = frame_block(&colors, &i.title, false).title_bottom(Line::styled(" ↑↓ scroll · Esc close ", colors.dim()));
                let inner = block.inner(r);
                f.render_widget(Clear, r);
                f.render_widget(block, r);
                i.scroll = i.scroll.min(i.lines.len().saturating_sub(inner.height as usize));
                let lines: Vec<Line> = i.lines.iter().skip(i.scroll).take(inner.height as usize).cloned().collect();
                f.render_widget(Paragraph::new(lines).style(colors.base()), inner);
            }
            Overlay::Logbook(_) => self.render_logbook(f, area),
        }
    }
}

/// A box with a title, in the chrome's colours (danger's for what removes things).
pub fn frame_block<'a>(colors: &Colors, title: &str, danger: bool) -> Block<'a> {
    let edge = if danger { colors.danger() } else { colors.accent() };
    Block::default()
        .borders(Borders::ALL)
        .border_style(edge)
        .style(colors.base())
        .title(Line::from(Span::styled(format!(" {title} "), edge.add_modifier(Modifier::BOLD))))
}

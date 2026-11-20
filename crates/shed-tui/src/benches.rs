//! The benches screen: every bench, running and closed, as the pegboard
//! lists them, with their tabs; opening, loading, making, renaming,
//! colouring, closing and deleting them.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use shed_client::pegboard::{BenchStatus, ListedBench};
use shed_model::BenchId;
use shed_proto::{BenchRef, ClientMsg, ShowTarget};

use crate::app::App;
use crate::text::{Input, fit, pad, width};

#[derive(Default)]
pub struct BenchesState {
    pub sel: usize,
    pub filter: Input,
    /// Typing goes to the filter.
    pub filtering: bool,
    /// Where each bench's row was drawn.
    rows: Vec<(u16, usize)>,
}

impl BenchesState {
    /// Selects the bench shown, when coming back to the list.
    pub fn select_current(&mut self, _bench: Option<BenchId>) {
        self.filtering = false;
    }
}

impl App {
    pub(crate) fn listed(&self) -> Vec<ListedBench> {
        shed_client::pegboard::list(&self.tree, |_| false, &self.pegboard.stored, &self.pegboard.order, &self.benches.filter.text)
    }

    fn selected_bench(&self) -> Option<ListedBench> {
        let list = self.listed();
        let at = self.benches.sel.min(list.len().saturating_sub(1));
        list.into_iter().nth(at)
    }

    pub(crate) fn benches_key(&mut self, ev: KeyEvent) {
        if self.benches.filtering {
            match ev.code {
                KeyCode::Esc => {
                    self.benches.filtering = false;
                    self.benches.filter = Input::default();
                }
                KeyCode::Enter | KeyCode::Down => self.benches.filtering = false,
                _ => {
                    self.benches.filter.key(&ev);
                    self.benches.sel = 0;
                }
            }
            return;
        }
        let count = self.listed().len();
        match ev.code {
            KeyCode::Up | KeyCode::Char('k') => self.benches.sel = self.benches.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.benches.sel = (self.benches.sel + 1).min(count.saturating_sub(1)),
            KeyCode::Home | KeyCode::Char('g') => self.benches.sel = 0,
            KeyCode::End | KeyCode::Char('G') => self.benches.sel = count.saturating_sub(1),
            KeyCode::Char('/') => self.benches.filtering = true,
            KeyCode::Enter | KeyCode::Char('o') => self.open_selected(false),
            KeyCode::Char('L') => self.open_selected(true),
            KeyCode::Char('n') => self.new_bench(),
            KeyCode::Char('r') => {
                if let Some(b) = self.selected_bench() {
                    self.rename_bench_ref(b.bench_ref(), b.bench.name.clone());
                }
            }
            KeyCode::Char('c') => {
                if let Some(b) = self.selected_bench() {
                    self.color_menu(b.bench_ref());
                }
            }
            KeyCode::Char('x') => {
                let Some(b) = self.selected_bench() else { return };
                if b.status == BenchStatus::Closed {
                    self.note(format!("{} is closed already", b.bench.name));
                    return;
                }
                let id = b.bench.id;
                self.confirm(
                    "Close Bench",
                    format!("Close {}? It is saved, then every program in it stops. Load it again any time.", b.bench.name),
                    "Close",
                    true,
                    move |app| app.send(ClientMsg::CloseBench { bench: id }),
                );
            }
            KeyCode::Char('D') => {
                let Some(b) = self.selected_bench() else { return };
                let name = b.bench.name.clone();
                self.confirm(
                    "Delete Bench",
                    format!("Delete {name}? Its programs stop and its saved file is removed. This cannot be undone."),
                    "Delete",
                    true,
                    move |app| app.send(ClientMsg::DeleteBench { name }),
                );
            }
            KeyCode::Esc | KeyCode::Char('q') if self.current_bench().is_some() => self.screen = crate::app::Screen::Bench,
            _ => {}
        }
    }

    /// Opens the selected bench: shows it if it runs, loads it if not
    /// (`start`: with its startup).
    fn open_selected(&mut self, start: bool) {
        let Some(b) = self.selected_bench() else { return };
        match b.bench_ref() {
            BenchRef::Running(id) => self.open_bench(id),
            BenchRef::Saved(name) => {
                let target = if start { ShowTarget::LoadAndStart { name } } else { ShowTarget::Load { name } };
                self.send(ClientMsg::Show(target));
            }
        }
    }

    fn new_bench(&mut self) {
        self.ask_with_hint("New Bench", "its name (empty: the next free one)", "", |app, name| {
            let name = Some(name.trim().to_string()).filter(|n| !n.is_empty());
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| "/".into());
            let start = std::env::current_dir().unwrap_or(home);
            app.ask_folder("New bench in folder", &start, move |app, cwd| {
                app.send(ClientMsg::Show(ShowTarget::NewBench { name, cwd: Some(cwd) }));
            });
        });
    }

    pub(crate) fn benches_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((_, i)) = self.benches.rows.iter().find(|(y, _)| *y == m.row) {
                    if self.benches.sel == *i {
                        self.open_selected(false);
                    } else {
                        self.benches.sel = *i;
                    }
                }
            }
            MouseEventKind::ScrollUp => self.benches.sel = self.benches.sel.saturating_sub(1),
            MouseEventKind::ScrollDown => self.benches.sel = (self.benches.sel + 1).min(self.listed().len().saturating_sub(1)),
            _ => {}
        }
    }

    pub(crate) fn render_benches(&mut self, f: &mut Frame, area: Rect) {
        let c = self.colors.clone();
        let list = self.listed();
        self.benches.sel = self.benches.sel.min(list.len().saturating_sub(1));
        let wide = area.width >= 100;
        let list_area = if wide { Rect { width: area.width / 2, ..area } } else { area };
        let head = Line::from(vec![
            Span::styled(" Benches ", c.chip(c.accent)),
            Span::raw("  "),
            Span::styled("Enter open · n new · r rename · c colour · x close · D delete · / filter · L load and start", c.dim()),
        ]);
        f.render_widget(Paragraph::new(head), Rect { height: 1, ..area });
        let mut y = area.y + 2;
        if self.benches.filtering || !self.benches.filter.text.is_empty() {
            let style = if self.benches.filtering { c.accent() } else { c.dim() };
            f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" / ", style), Span::raw(self.benches.filter.text.clone())])), Rect { y: area.y + 1, height: 1, ..list_area });
            if self.benches.filtering {
                f.set_cursor_position((list_area.x + 3 + width(&self.benches.filter.text) as u16, area.y + 1));
            }
        }
        if list.is_empty() {
            let text = if self.benches.filter.text.is_empty() { "No benches yet: n makes one." } else { "No bench matches." };
            f.render_widget(Paragraph::new(Line::styled(format!("  {text}"), c.dim())), Rect { y, height: 1, ..list_area });
        }
        self.benches.rows.clear();
        // Keep the selection in view: skip benches above it if needed.
        let bottom = area.y + area.height;
        let heights: Vec<u16> = list.iter().map(|b| 1 + b.tabs.len().min(6) as u16).collect();
        let mut first = 0;
        while first < self.benches.sel && heights[first..=self.benches.sel].iter().sum::<u16>() > bottom.saturating_sub(y) {
            first += 1;
        }
        let w = list_area.width as usize;
        for (i, b) in list.iter().enumerate().skip(first) {
            if y >= bottom {
                break;
            }
            let selected = i == self.benches.sel;
            let here = self.bench == Some(b.bench.id);
            let state = match b.status {
                BenchStatus::Closed => format!("closed{}", b.saved.as_deref().map(|s| format!(" · saved {s}")).unwrap_or_default()),
                _ if here => "running · shown here".to_string(),
                _ => "running".to_string(),
            };
            let accent = b.bench.color.accent();
            let marker = if selected { "▶ " } else { "  " };
            let name = format!(" {} ", fit(&b.bench.name, 30));
            let count = format!("  {} tab{}  ", b.bench.tabs.len(), if b.bench.tabs.len() == 1 { "" } else { "s" });
            let mut spans = vec![Span::styled(marker, c.accent()), Span::styled(name.clone(), c.chip(accent)), Span::styled(count.clone(), c.dim())];
            let left = w.saturating_sub(2 + width(&name) + width(&count));
            let state_style = if b.status == BenchStatus::Closed { c.dim() } else { c.added() };
            spans.push(Span::styled(fit(&state, left), state_style));
            let mut line = Line::from(spans);
            if selected {
                line = line.style(Style::new().bg(c.c(c.tab_hover)));
            }
            f.render_widget(Paragraph::new(line), Rect { x: list_area.x, y, width: list_area.width, height: 1 });
            self.benches.rows.push((y, i));
            y += 1;
            for (n, tab) in b.tabs.iter().filter_map(|t| b.bench.tabs.iter().find(|x| x.id == *t)).enumerate() {
                if y >= bottom {
                    break;
                }
                if n == 6 {
                    break;
                }
                let active = tab.id == b.bench.active_tab;
                let title = format!("{}{}", if active { "• " } else { "  " }, tab.display_title());
                let style = if active { Style::new().fg(c.c(c.ink(accent))) } else { c.dim() };
                f.render_widget(Paragraph::new(Line::styled(pad(&format!("      {title}"), w), style)), Rect { x: list_area.x, y, width: list_area.width, height: 1 });
                self.benches.rows.push((y, i));
                y += 1;
            }
        }
        if wide && let Some(b) = list.get(self.benches.sel) {
            let r = Rect { x: area.x + area.width / 2, y: area.y + 2, width: area.width - area.width / 2, height: area.height.saturating_sub(2) };
            let block = Block::default().borders(Borders::LEFT).border_style(c.fg(c.divider)).title(Line::styled(format!(" {} ", b.bench.name), c.fg(c.ink(b.bench.color.accent())).add_modifier(Modifier::BOLD)));
            let inner = block.inner(r);
            f.render_widget(block, r);
            let mut lines = Vec::new();
            if let Some(tab) = b.bench.active_tab() {
                lines.push(Line::styled(format!("{} — {}", tab.display_title(), b.bench.cwd.display()), c.dim()));
                lines.push(Line::default());
                if let Some((_, preview)) = self.pegboard.previews.iter().find(|(p, _)| *p == tab.active_pane) {
                    for l in preview {
                        lines.push(Line::raw(fit(l, inner.width as usize)));
                    }
                } else if let Some(ai) = tab.ai_info() {
                    lines.push(Line::raw(format!("Claude Code in {}", ai.folder.display())));
                } else if let Some(p) = tab.project_info() {
                    lines.push(Line::raw(format!("Project {}", p.root.display())));
                }
            }
            f.render_widget(Paragraph::new(lines), inner);
        }
    }
}

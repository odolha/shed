//! The logbook search (`C-b r`): commands run before, in this tab, this
//! bench or everywhere; the one chosen is typed into the focused pane (as a
//! paste, not run).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use shed_logbook::{Context, Entry, Logbook, Scope};

use crate::app::App;
use crate::overlay::{Overlay, frame_block};
use crate::text::{Input, centered, fit, pad, width};

pub struct Search {
    pub input: Input,
    pub scope: Scope,
    pub results: Vec<Entry>,
    pub sel: usize,
    cx: Context,
    book: Option<Logbook>,
    pub error: Option<String>,
}

impl Search {
    fn run(&mut self) {
        let Some(book) = &self.book else { return };
        match book.search_typed(self.scope, self.cx, &self.input.text, 200) {
            Ok(r) => {
                self.results = r;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.sel = self.sel.min(self.results.len().saturating_sub(1));
    }
}

impl App {
    pub(crate) fn open_logbook(&mut self) {
        let (Some(bench), Some(tab)) = (self.bench, self.active_tab().map(|t| t.id)) else {
            self.note("open a bench first");
            return;
        };
        if self.focused_pane().is_none() {
            self.note("the logbook types into a terminal pane");
            return;
        }
        let (book, error) = match Logbook::open_readonly(&self.logbook_path) {
            Ok(b) => (Some(b), None),
            Err(e) => (None, Some(format!("no logbook yet ({e})"))),
        };
        let mut s = Search { input: Input::default(), scope: Scope::Bench, results: Vec::new(), sel: 0, cx: Context { tab, bench }, book, error };
        s.run();
        self.overlay = Some(Overlay::Logbook(s));
    }

    pub(crate) fn logbook_key(&mut self, mut s: Search, ev: KeyEvent) {
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        match ev.code {
            KeyCode::Esc => return,
            KeyCode::Enter => {
                if let Some(e) = s.results.get(s.sel) {
                    let command = e.command.clone();
                    self.paste_into_pane(&command);
                }
                return;
            }
            KeyCode::Tab => {
                s.scope = s.scope.next();
                s.run();
            }
            KeyCode::Up => s.sel = s.sel.saturating_sub(1),
            KeyCode::Char('p') if ctrl => s.sel = s.sel.saturating_sub(1),
            KeyCode::Down => s.sel = (s.sel + 1).min(s.results.len().saturating_sub(1)),
            KeyCode::Char('n') if ctrl => s.sel = (s.sel + 1).min(s.results.len().saturating_sub(1)),
            // Ctrl+R again: older.
            KeyCode::Char('r') if ctrl => s.sel = (s.sel + 1).min(s.results.len().saturating_sub(1)),
            _ => {
                if s.input.key(&ev) == crate::text::Edit::Changed {
                    s.sel = 0;
                    s.run();
                }
            }
        }
        self.overlay = Some(Overlay::Logbook(s));
    }

    pub(crate) fn render_logbook(&mut self, f: &mut Frame, area: Rect) {
        let c = self.colors.clone();
        let Some(Overlay::Logbook(s)) = &mut self.overlay else { return };
        let r = centered(area, area.width.saturating_sub(4).min(110), area.height.saturating_sub(2).min(24));
        let title = format!("History · {} (Tab: local, bench, global)", s.scope.name());
        let block = frame_block(&c, &title, false).title_bottom(Line::styled(" Enter types it · Esc closes · tag:name narrows ", c.dim()));
        let inner = block.inner(r);
        f.render_widget(Clear, r);
        f.render_widget(block, r);
        let w = inner.width as usize;
        let mut lines = vec![Line::from(vec![Span::styled("› ", c.accent()), Span::raw(s.input.text.clone())])];
        if let Some(e) = &s.error {
            lines.push(Line::styled(fit(e, w), c.danger()));
        } else if s.results.is_empty() {
            lines.push(Line::styled("nothing found", c.dim()));
        }
        let rows = inner.height.saturating_sub(1) as usize;
        let top = s.sel.saturating_sub(rows.saturating_sub(1));
        let now = crate::app::now() as i64 * 1000;
        for (i, e) in s.results.iter().enumerate().skip(top).take(rows) {
            let age = shed_logbook::age(now - e.started_ms);
            let failed = e.exit.is_some_and(|x| x != 0);
            let meta = format!(" {age:>4} {}", fit(&e.tab_title, 18));
            let cmd = pad(&e.command.replace('\n', "⏎"), w.saturating_sub(width(&meta)));
            let mut line = Line::from(vec![Span::styled(cmd, if failed { c.danger() } else { c.base() }), Span::styled(meta, c.dim())]);
            if i == s.sel {
                line = line.style(c.selected());
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines).style(c.base()), inner);
        f.set_cursor_position((inner.x + 2 + width(&s.input.text[..s.input.cursor]) as u16, inner.y));
    }
}

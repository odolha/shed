//! Ctrl+R: searching the logbook from a pane's prompt.

use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString, Task, Window, div,
    prelude::*, px,
};
use shed_logbook::{Context as Here, Entry, Logbook, Scope};
use shed_model::Rgb;

use crate::logbook::age;
use crate::theme::{UiColors, hsla, hsla_alpha};

/// How many results are fetched, and how many rows are shown.
const FETCH: usize = 200;
const ROWS: usize = 10;

pub enum SearchEvent {
    /// Put this command on the prompt line; `run` presses Enter too.
    Insert { command: String, run: bool },
    Close,
}

pub struct LogbookSearch {
    focus: FocusHandle,
    here: Here,
    path: PathBuf,
    scope: Scope,
    query: String,
    /// Cursor position in characters.
    cursor: usize,
    results: Vec<Entry>,
    selected: usize,
    error: Option<String>,
    ui: UiColors,
    accent: Rgb,
    mono: SharedString,
    pending: Option<Task<()>>,
}

impl EventEmitter<SearchEvent> for LogbookSearch {}

impl Focusable for LogbookSearch {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LogbookSearch {
    pub fn new(here: Here, path: PathBuf, ui: UiColors, accent: Rgb, mono: SharedString, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus: cx.focus_handle(),
            here,
            path,
            scope: Scope::Local,
            query: String::new(),
            cursor: 0,
            results: Vec::new(),
            selected: 0,
            error: None,
            ui,
            accent,
            mono,
            pending: None,
        };
        this.refresh(cx);
        this
    }

    /// Runs the query off the UI thread; a newer query replaces an older one.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (path, scope, here) = (self.path.clone(), self.scope, self.here);
        // `tag:prod rest of query`
        let mut tag = None;
        let words: Vec<&str> = self
            .query
            .split_whitespace()
            .filter(|w| match w.strip_prefix("tag:") {
                Some(t) if !t.is_empty() => {
                    tag = Some(t.to_string());
                    false
                }
                _ => true,
            })
            .collect();
        let text = words.join(" ");
        self.pending = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let book = Logbook::open_readonly(&path)?;
                    book.search(scope, here, &text, tag.as_deref(), FETCH)
                })
                .await;
            this.update(cx, |this, cx| {
                match found {
                    Ok(results) => {
                        this.results = results;
                        this.error = None;
                    }
                    Err(e) => {
                        this.results.clear();
                        this.error = Some(if this.path.exists() { e.to_string() } else { "The logbook is empty.".into() });
                    }
                }
                this.selected = 0;
                cx.notify();
            })
            .ok();
        }));
    }

    fn byte(&self, chars: usize) -> usize {
        self.query.char_indices().nth(chars).map_or(self.query.len(), |(i, _)| i)
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = ks.modifiers;
        let len = self.query.chars().count();
        let mut edited = false;
        match ks.key.as_str() {
            "escape" => cx.emit(SearchEvent::Close),
            "enter" => {
                if let Some(e) = self.results.get(self.selected) {
                    cx.emit(SearchEvent::Insert { command: e.command.clone(), run: m.alt });
                }
            }
            "tab" => {
                self.scope = if m.shift { self.scope.next().next() } else { self.scope.next() };
                edited = true;
            }
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(self.results.len().saturating_sub(1)),
            "pageup" => self.selected = self.selected.saturating_sub(ROWS),
            "pagedown" => self.selected = (self.selected + ROWS).min(self.results.len().saturating_sub(1)),
            "r" if m.control => self.selected = (self.selected + 1).min(self.results.len().saturating_sub(1)),
            "u" if m.control => {
                self.query.clear();
                self.cursor = 0;
                edited = true;
            }
            "backspace" if self.cursor > 0 => {
                let (a, b) = (self.byte(self.cursor - 1), self.byte(self.cursor));
                self.query.replace_range(a..b, "");
                self.cursor -= 1;
                edited = true;
            }
            "delete" if self.cursor < len => {
                let (a, b) = (self.byte(self.cursor), self.byte(self.cursor + 1));
                self.query.replace_range(a..b, "");
                edited = true;
            }
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => self.cursor = (self.cursor + 1).min(len),
            "home" => self.cursor = 0,
            "end" => self.cursor = len,
            _ => match ks.key_char.as_deref() {
                Some(text) if !m.control && !m.platform => {
                    let at = self.byte(self.cursor);
                    self.query.insert_str(at, text);
                    self.cursor += text.chars().count();
                    edited = true;
                }
                _ => return,
            },
        }
        if edited {
            self.refresh(cx);
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for LogbookSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = &self.ui;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let here_tab = self.here.tab.to_string();

        let scopes = div().flex().gap_1().children([Scope::Local, Scope::Bench, Scope::Global].map(|s| {
            let on = s == self.scope;
            div()
                .px_1p5()
                .rounded_sm()
                .when(on, |d| d.bg(hsla_alpha(self.accent, 0.25)).text_color(hsla(self.accent)))
                .child(s.name())
        }));
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(12.0))
            .text_color(hsla(ui.text_dim))
            .child(scopes)
            .child("Tab scope · Enter insert · Alt+Enter run · Esc close");

        let at = self.byte(self.cursor);
        let (before, after) = self.query.split_at(at);
        let input = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(20.0))
            .font_family(self.mono.clone())
            .text_color(hsla(ui.text))
            .child(div().flex_none().text_color(hsla(self.accent)).child(">"))
            .child(div().flex_none().w(px(8.0)))
            .child(div().flex_none().child(before.to_string()))
            .child(div().flex_none().w(px(1.5)).h(px(15.0)).bg(hsla(ui.text)))
            .child(div().flex_none().child(after.to_string()));

        let start = self.selected.saturating_sub(ROWS - 1);
        let mut list = div().flex().flex_col().font_family(self.mono.clone()).text_size(px(13.0));
        if let Some(e) = &self.error {
            list = list.child(div().text_color(hsla(ui.text_dim)).child(e.clone()));
        } else if self.results.is_empty() {
            list = list.child(div().text_color(hsla(ui.text_dim)).child("No matches."));
        }
        for (i, e) in self.results.iter().enumerate().skip(start).take(ROWS) {
            let failed = e.exit.is_some_and(|c| c != 0);
            let dir = std::path::Path::new(&e.cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| e.cwd.clone());
            let mut meta = vec![dir];
            if e.tab != here_tab {
                meta.push(e.tab_title.clone());
            }
            meta.push(age(now - e.started_ms));
            list = list.child(
                div()
                    .flex()
                    .gap_2()
                    .px_1()
                    .rounded_sm()
                    .when(i == self.selected, |d| d.bg(hsla_alpha(self.accent, 0.2)))
                    .child(div().w(px(12.0)).flex_none().text_color(hsla(Rgb::hex(0xF0566A))).child(if failed { "✗" } else { "" }))
                    .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(e.command.replace('\n', " ⏎ ")))
                    .child(div().flex_none().text_color(hsla(ui.text_dim)).child(meta.join(" · "))),
            );
        }

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(hsla(ui.tab_active))
            .border_t_2()
            .border_color(hsla(self.accent))
            .text_size(px(13.0))
            .child(header)
            .child(input)
            .child(list)
    }
}

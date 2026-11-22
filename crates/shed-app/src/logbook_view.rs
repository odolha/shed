//! Ctrl+R: searching the logbook from a pane's prompt.

use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString, Task, Window, div,
    prelude::*, px,
};
use shed_logbook::{Context as Here, Entry, Logbook, Scope, age};
use shed_model::Rgb;

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
    /// A remote tab's server, whose history this searches (a copy fetched
    /// over ssh), and the fetch while it runs.
    server: Option<String>,
    fetching: Option<Task<()>>,
    /// Why there is nothing to search here (another workspace's history,
    /// not reachable yet).
    unavailable: Option<String>,
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
            server: None,
            fetching: None,
            unavailable: None,
        };
        this.refresh(cx);
        this
    }

    /// Nothing to search, saying why: another workspace's history (this
    /// PC's logbook is not that workspace's).
    pub fn unavailable(note: String, ui: UiColors, accent: Rgb, mono: SharedString, cx: &mut Context<Self>) -> Self {
        let here = Here { tab: shed_model::TabId::new(), bench: shed_model::BenchId::new() };
        let mut this = Self::new(here, PathBuf::new(), ui, accent, mono, cx);
        this.pending = None;
        this.error = Some(note.clone());
        this.unavailable = Some(note);
        this
    }

    /// Searches the history on a remote tab's server: what ran in that
    /// shell, from any PC, and everything else run there. A copy fetched
    /// earlier shows at once; the fresh one replaces it when it comes.
    pub fn remote(host: String, shell: String, ui: UiColors, accent: Rgb, mono: SharedString, cx: &mut Context<Self>) -> Self {
        let path = shed_logbook::remote_copy_path(&host);
        // Until the server says which of its tabs this is, nothing is "this tab".
        let unknown = Here { tab: shed_model::TabId::new(), bench: shed_model::BenchId::new() };
        let mut this = Self::new(unknown, path.clone(), ui, accent, mono, cx);
        this.server = Some(host.clone());
        this.fetching = Some(cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    let shells = shed_client::remote::list_shells(&host)?;
                    let here = shells.iter().find(|s| s.id == shell).and_then(|s| {
                        Some(Here { tab: shed_model::TabId(s.tab.parse().ok()?), bench: shed_model::BenchId(s.bench.parse().ok()?) })
                    });
                    let any = shed_client::remote::fetch_logbook(&host, &path)?;
                    anyhow::Ok((here, any))
                })
                .await;
            this.update(cx, |this, cx| {
                this.fetching = None;
                match fetched {
                    Ok((here, any)) => {
                        if let Some(here) = here {
                            this.here = here;
                        }
                        if !any {
                            let _ = std::fs::remove_file(&this.path);
                        }
                        this.refresh(cx);
                    }
                    Err(e) => {
                        this.results.clear();
                        this.error = Some(format!("Cannot fetch the history: {e}"));
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
        this
    }

    /// Runs the query off the UI thread; a newer query replaces an older one.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.unavailable.is_some() {
            return;
        }
        let (path, scope, here, query) = (self.path.clone(), self.scope, self.here, self.query.clone());
        self.pending = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let book = Logbook::open_readonly(&path)?;
                    book.search_typed(scope, here, &query, FETCH)
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
                        this.error = Some(if this.path.exists() {
                            e.to_string()
                        } else if this.fetching.is_some() {
                            "Fetching the history…".into()
                        } else {
                            "The logbook is empty.".into()
                        });
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
                .when(on, |d| d.bg(hsla_alpha(self.accent, 0.25)).text_color(hsla(self.ui.ink(self.accent))))
                .child(s.name())
        }));
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .text_size(crate::theme::us(12.0))
            .text_color(hsla(ui.text_dim))
            .child(div().flex().gap_2().child(scopes).children(self.server.as_ref().map(|host| {
                let text = if self.fetching.is_some() { format!("on {host} (fetching…)") } else { format!("on {host}") };
                div().child(text)
            })))
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
        let mut list = div().flex().flex_col().font_family(self.mono.clone()).text_size(crate::theme::us(13.0));
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
                    .child(div().w(px(12.0)).flex_none().text_color(hsla(ui.danger)).child(if failed { "✗" } else { "" }))
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
            .text_size(crate::theme::us(13.0))
            .child(header)
            .child(input)
            .child(list)
    }
}

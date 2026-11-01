//! The pegboard, the whole of the Shed window: every bench, open, hidden or
//! closed, with its tabs and a preview of each. From here benches are made,
//! opened, hidden, closed and deleted. Type to filter, arrows and Enter to
//! go to a tab, drag a tab onto another bench to move it there.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, Context, Entity, Focusable, KeyDownEvent, PromptLevel, SharedString, Subscription,
    Window, div, prelude::*, px, svg,
};
use shed_model::{Bench, BenchId, PaneId, Rgb, Tab, TabId};
use shed_proto::{ClientMsg, PegboardInfo, ShowTarget};
use shed_save::home_relative;

use crate::assets::icon;
use crate::bench_window::DraggedTab;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::state::AppState;
use crate::theme::{UiColors, hsla, hsla_alpha};

/// How often the previews refresh while the Shed window is active; ten
/// times slower while it is not.
const REFRESH: Duration = Duration::from_secs(1);
const IDLE_REFRESH_TICKS: u32 = 10;
const CARD_WIDTH: f32 = 300.0;
const DANGER: Rgb = Rgb::hex(0xF0566A);

/// The keeper's last answer: stored benches and previews.
pub struct PegboardData(pub PegboardInfo);

impl gpui::Global for PegboardData {}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    /// Running, with a window.
    Open,
    /// Running, without a window.
    Hidden,
    /// Only saved.
    Closed,
}

#[derive(Clone)]
struct Entry {
    bench: Bench,
    status: Status,
    saved: Option<String>,
    /// The tabs the filter lets through.
    tabs: Vec<TabId>,
}

pub struct Pegboard {
    state: Entity<AppState>,
    filter: Entity<LineEditor>,
    /// Position in the list of shown tabs.
    selected: usize,
    query: String,
    ticks: u32,
    _subscriptions: Vec<Subscription>,
}

impl Pegboard {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| LineEditor::new(String::new(), cx));
        window.focus(&filter.read(cx).focus_handle(cx), cx);
        let subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.observe_global::<PegboardData>(|_, cx| cx.notify()),
            cx.observe(&filter, |this, filter, cx| {
                let query = filter.read(cx).text().to_string();
                if query != this.query {
                    this.query = query;
                    this.selected = 0;
                }
                cx.notify();
            }),
            cx.subscribe(&filter, |this, filter, event: &EditorEvent, cx| match event {
                EditorEvent::Submit(_) => {
                    let entries = this.entries(cx);
                    if let Some((bench, tab)) = flat(&entries).get(this.selected).copied() {
                        this.open(&entries[bench], Some(tab), cx);
                    }
                }
                EditorEvent::Cancel => filter.update(cx, |f, cx| f.clear(cx)),
            }),
            // Fresh previews the moment Shed comes to the front.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.refresh(cx);
                }
            }),
        ];
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let asked = this.update_in(cx, |this, window, cx| {
                    this.ticks += 1;
                    if window.is_window_active() || this.ticks.is_multiple_of(IDLE_REFRESH_TICKS) {
                        this.refresh(cx);
                    }
                });
                if asked.is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        })
        .detach();
        Self { state, filter, selected: 0, query: String::new(), ticks: 0, _subscriptions: subscriptions }
    }

    fn refresh(&self, cx: &Context<Self>) {
        self.send(ClientMsg::Pegboard, cx);
    }

    fn send(&self, msg: ClientMsg, cx: &Context<Self>) {
        self.state.read(cx).conn.send_msg(msg);
    }

    /// Every bench by name, each with the tabs the filter matches.
    fn entries(&self, cx: &Context<Self>) -> Vec<Entry> {
        let state = self.state.read(cx);
        let live = state.tree.benches.iter().map(|b| {
            let status = if state.has_window(b.id) { Status::Open } else { Status::Hidden };
            (b.clone(), status, None)
        });
        let stored = cx.try_global::<PegboardData>().map(|d| d.0.stored.as_slice()).unwrap_or_default();
        let stored = stored.iter().filter(|s| state.tree.bench(s.bench.id).is_none());
        let stored = stored.map(|s| (s.bench.clone(), Status::Closed, Some(s.saved.clone())));
        let mut all: Vec<(Bench, Status, Option<String>)> = live.chain(stored).collect();
        // By name, so a bench stays where it is as it opens and closes.
        all.sort_by_key(|(b, _, _)| b.name.to_lowercase());

        let words: Vec<String> = self.query.split_whitespace().map(str::to_lowercase).collect();
        let matches = |text: &str| {
            let text = text.to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        };
        all.into_iter()
            .filter_map(|(bench, status, saved)| {
                let tabs: Vec<TabId> = if matches(&bench.name) {
                    bench.tabs.iter().map(|t| t.id).collect()
                } else {
                    bench.tabs.iter().filter(|t| matches(&format!("{} {}", bench.name, haystack(t)))).map(|t| t.id).collect()
                };
                (!tabs.is_empty()).then_some(Entry { bench, status, saved, tabs })
            })
            .collect()
    }

    /// Opens a bench, at `tab` if given: raises its window, gives a hidden
    /// one a window, or loads a closed one.
    fn open(&mut self, entry: &Entry, tab: Option<TabId>, cx: &mut Context<Self>) {
        if entry.status == Status::Closed {
            self.send(ClientMsg::Show(ShowTarget::Load { name: entry.bench.name.clone() }), cx);
        }
        if let Some(tab) = tab {
            self.send(ClientMsg::FocusTab { tab }, cx);
        }
        if entry.status != Status::Closed {
            let bench = entry.bench.id;
            self.state.update(cx, |state, cx| state.show(bench, cx));
        }
    }

    fn hide(&mut self, bench: BenchId, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| state.hide_bench(bench, cx));
    }

    /// Asks, then sends `msg`.
    fn confirm(&mut self, message: String, detail: &str, button: &str, msg: ClientMsg, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(PromptLevel::Warning, &message, Some(detail), &[button, "Cancel"], cx);
        let conn = self.state.read(cx).conn.clone();
        cx.spawn(async move |_, _| {
            if let Ok(0) = answer.await {
                conn.send_msg(msg);
            }
        })
        .detach();
    }

    fn close(&mut self, bench: &Bench, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm(
            format!("Close the bench “{}”?", bench.name),
            "It is saved, then the programs running in it stop and its teardown commands run. \
             Open it again from here to bring it back.",
            "Close Bench",
            ClientMsg::CloseBench { bench: bench.id },
            window,
            cx,
        );
    }

    fn delete(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm(
            format!("Delete the bench “{name}”?"),
            "It stops if it runs, and its saved file, scrollback and launcher are removed. This cannot be undone.",
            "Delete Bench",
            ClientMsg::DeleteBench { name },
            window,
            cx,
        );
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let count = flat(&self.entries(cx)).len();
        match event.keystroke.key.as_str() {
            "down" => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            "up" => self.selected = self.selected.saturating_sub(1),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn button(
        &self,
        id: String,
        label: &str,
        color: Rgb,
        filled: bool,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(SharedString::from(id))
            .px_2()
            .py_0p5()
            .rounded_sm()
            .text_size(px(12.0))
            .cursor_pointer()
            .text_color(hsla(color))
            .when(filled, |d| d.bg(hsla_alpha(color, 0.2)))
            .hover(move |s| s.bg(hsla_alpha(color, 0.3)))
            .child(label.to_string())
            .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
            .into_any_element()
    }

    fn render_bench(&self, entry: &Entry, first: usize, previews: &HashMap<PaneId, Vec<String>>, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let id = bench.id;
        let status = match entry.status {
            Status::Open => "open".to_string(),
            Status::Hidden => "hidden, still running".to_string(),
            Status::Closed => format!("closed, saved {}", entry.saved.as_deref().map(short_time).unwrap_or_default()),
        };
        let tab_count = bench.tabs.len();
        let mut header = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().size(px(10.0)).rounded_full().flex_none().bg(hsla(accent)))
            .child(div().text_size(px(15.0)).text_color(hsla(ui.text)).child(bench.name.clone()))
            .when_some(bench.badge.clone(), |el, badge| {
                el.child(div().px_1().rounded_sm().text_size(px(12.0)).bg(hsla_alpha(accent, 0.25)).text_color(hsla(accent)).child(badge))
            })
            .child(div().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child(format!(
                "{status} · {tab_count} tab{}",
                if tab_count == 1 { "" } else { "s" }
            )))
            .child(div().flex_1());

        if bench.closing {
            header = header.child(div().text_size(px(12.0)).text_color(hsla(accent)).child("closing…"));
        } else {
            let open_label = if entry.status == Status::Open { "Go to" } else { "Open" };
            let e = entry.clone();
            header = header.child(self.button(format!("open-{id}"), open_label, accent, true, cx, move |this, _, cx| this.open(&e, None, cx)));
            if entry.status == Status::Open {
                header = header.child(self.button(format!("hide-{id}"), "Hide", ui.text_dim, false, cx, move |this, _, cx| this.hide(id, cx)));
            }
            if entry.status != Status::Closed {
                let b = bench.clone();
                header = header.child(self.button(format!("close-{id}"), "Close…", ui.text_dim, false, cx, move |this, window, cx| {
                    this.close(&b, window, cx)
                }));
            }
            let name = bench.name.clone();
            header = header.child(self.button(format!("delete-{id}"), "Delete…", DANGER, false, cx, move |this, window, cx| {
                this.delete(name.clone(), window, cx)
            }));
        }

        let mut cards = div().flex().flex_wrap().gap_3();
        for (i, tab_id) in entry.tabs.iter().enumerate() {
            let Some(tab) = bench.tabs.iter().find(|t| t.id == *tab_id) else { continue };
            cards = cards.child(self.render_tab(entry, tab, first + i, previews, ui, cx));
        }

        let running = entry.status != Status::Closed;
        div()
            .id(SharedString::from(format!("bench-{id}")))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(hsla(ui.divider))
            .when(running, |el| {
                el.drag_over::<DraggedTab>(move |s, _, _, _| s.border_color(hsla(accent)))
                    .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                        this.send(ClientMsg::MoveTab { tab: d.tab, bench: id, index: usize::MAX }, cx);
                    }))
            })
            .child(header)
            .child(cards)
            .into_any_element()
    }

    fn render_tab(&self, entry: &Entry, tab: &Tab, index: usize, previews: &HashMap<PaneId, Vec<String>>, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let selected = index == self.selected;
        let active = tab.id == bench.active_tab;
        let title = tab.display_title();
        let pane = tab.active();
        let lines = previews.get(&tab.active_pane).cloned().unwrap_or_default();
        let mono: SharedString = self.state.read(cx).settings.font.family.clone().into();
        let tool = pane.and_then(|p| p.tool.as_ref()).map(|t| t.display());
        let waiting = tab.panes.iter().any(|p| p.waiting.is_some());

        let mut head = div().flex().items_center().gap_1p5().min_w_0();
        if waiting {
            head = head.child(svg().path(icon("tool-waiting")).size(px(12.0)).flex_none().text_color(hsla(accent)));
        } else if tool.is_some() {
            head = head.child(svg().path(icon("tool")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)));
        }
        if tab.private {
            head = head.child(svg().path(icon("private")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)));
        }
        head = head.child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(title.clone()));
        for tag in tab.all_tags() {
            head = head.child(
                div()
                    .flex_none()
                    .px_1()
                    .rounded_sm()
                    .text_size(px(11.0))
                    .bg(hsla_alpha(tag.color, 0.25))
                    .text_color(hsla(tag.color))
                    .child(tag.label.clone()),
            );
        }

        let preview = div()
            .h(px(12.0 * 11.0))
            .overflow_hidden()
            .flex()
            .flex_col()
            .justify_end()
            .font_family(mono)
            .text_size(px(9.0))
            .line_height(px(11.0))
            .text_color(hsla(ui.text_dim))
            .children(lines.into_iter().map(|l| div().whitespace_nowrap().overflow_hidden().child(if l.is_empty() { " ".to_string() } else { l })));

        let mut foot = vec![pane.map(|p| home_relative(&p.cwd)).unwrap_or_default()];
        if let Some(t) = &tool {
            foot.push(t.clone());
        }
        if tab.panes.len() > 1 {
            foot.push(format!("{} panes", tab.panes.len()));
        }

        let e = entry.clone();
        let tab_id = tab.id;
        let border = if selected { hsla(accent) } else if active { hsla_alpha(accent, 0.45) } else { hsla(ui.divider) };
        div()
            .id(SharedString::from(format!("tab-{}", tab.id)))
            .w(px(CARD_WIDTH))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .hover(move |s| s.border_color(hsla(accent)))
            .when(entry.status != Status::Closed, |el| {
                el.on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open(&e, Some(tab_id), cx)))
            .child(head)
            .child(preview)
            .child(div().truncate().text_size(px(11.0)).text_color(hsla(ui.text_dim)).child(foot.join(" · ")))
            .into_any_element()
    }
}

/// Every shown tab in order, as (entry, tab).
fn flat(entries: &[Entry]) -> Vec<(usize, TabId)> {
    entries.iter().enumerate().flat_map(|(i, e)| e.tabs.iter().map(move |t| (i, *t))).collect()
}

/// What the filter searches in a tab: its title, tags, folders and tools.
fn haystack(tab: &Tab) -> String {
    let mut text = tab.display_title();
    for tag in tab.all_tags() {
        text.push(' ');
        text.push_str(&tag.label);
    }
    for pane in &tab.panes {
        text.push(' ');
        text.push_str(&pane.cwd.to_string_lossy());
        if let Some(tool) = &pane.tool {
            text.push(' ');
            text.push_str(&tool.display());
        }
    }
    text
}

/// `2026-11-01T12:32:05Z` as `2026-11-01 12:32 UTC`.
fn short_time(saved: &str) -> String {
    match saved.get(..16) {
        Some(t) => format!("{} UTC", t.replace('T', " ")),
        None => saved.to_string(),
    }
}

impl Render for Pegboard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = self.state.read(cx).settings.ui.clone();
        let entries = self.entries(cx);
        let count = flat(&entries).len();
        if self.selected >= count {
            self.selected = count.saturating_sub(1);
        }
        let previews: HashMap<PaneId, Vec<String>> =
            cx.try_global::<PegboardData>().map(|d| d.0.previews.iter().cloned().collect()).unwrap_or_default();

        let mut list = div().flex().flex_col().gap_3();
        let mut first = 0;
        for entry in &entries {
            list = list.child(self.render_bench(entry, first, &previews, &ui, cx));
            first += entry.tabs.len();
        }
        if entries.is_empty() {
            let empty = if self.query.is_empty() { "No benches yet. Make one with “New bench”." } else { "Nothing matches." };
            list = list.child(div().text_color(hsla(ui.text_dim)).child(empty));
        }

        let new_bench = self.button("new-bench".into(), "+ New bench", ui.accent_fallback, true, cx, |this, _, cx| {
            this.send(ClientMsg::Show(ShowTarget::NewBench { name: None, cwd: None }), cx);
        });
        let search = div()
            .flex()
            .items_center()
            .gap_2()
            .child(svg().path(icon("pegboard")).size(px(18.0)).text_color(hsla(ui.text_dim)))
            .child(div().flex_1().max_w(px(480.0)).text_size(px(14.0)).child(self.filter.clone()))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(hsla(ui.text_dim))
                    .child("Filter by bench, tab, tag, folder or tool · ↑↓ and Enter to go · drag a tab onto a bench to move it"),
            )
            .child(new_bench);

        div()
            .id("pegboard")
            .key_context("Pegboard")
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .text_size(px(13.0))
            .on_key_down(cx.listener(Self::on_key))
            .child(search)
            .child(div().id("pegboard-list").flex_1().min_h_0().overflow_y_scroll().child(list))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_times_are_shortened() {
        assert_eq!(short_time("2026-11-01T12:32:05Z"), "2026-11-01 12:32 UTC");
        assert_eq!(short_time("soon"), "soon");
    }
}

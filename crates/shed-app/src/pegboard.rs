//! The pegboard: every bench, open, detached or stored, with its tabs and a
//! preview of each. Type to filter, arrows and Enter to jump, drag a tab onto
//! another bench to move it there.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, Focusable, KeyDownEvent, PromptLevel, SharedString,
    Subscription, Window, div, prelude::*, px, svg,
};
use shed_model::{Bench, BenchId, PaneId, Rgb, Tab, TabId};
use shed_proto::{ClientMsg, PegboardInfo, ShowTarget};
use shed_save::home_relative;

use crate::assets::icon;
use crate::bench_window::DraggedTab;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::state::AppState;
use crate::theme::{UiColors, hsla, hsla_alpha};

/// How often the previews refresh while the pegboard is open.
const REFRESH: Duration = Duration::from_secs(1);
const CARD_WIDTH: f32 = 300.0;

pub enum PegboardEvent {
    Close,
}

/// The keeper's last answer: stored benches and previews.
pub struct PegboardData(pub PegboardInfo);

impl gpui::Global for PegboardData {}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    /// Shown in a window of this Shed.
    Open,
    /// Running in the keeper without a window.
    Detached,
    /// Only saved.
    Stored,
}

struct Entry {
    bench: Bench,
    status: Status,
    saved: Option<String>,
    /// The tabs the filter lets through.
    tabs: Vec<TabId>,
}

pub struct Pegboard {
    /// The bench whose window shows this pegboard.
    here: BenchId,
    state: Entity<AppState>,
    filter: Entity<LineEditor>,
    /// Position in the list of shown tabs.
    selected: usize,
    query: String,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PegboardEvent> for Pegboard {}

impl Pegboard {
    pub fn new(here: BenchId, state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            cx.subscribe(&filter, |this, _, event: &EditorEvent, cx| match event {
                EditorEvent::Submit(_) => {
                    let entries = this.entries(cx);
                    if let Some((bench, tab)) = flat(&entries).get(this.selected).copied() {
                        this.jump(&entries[bench], Some(tab), cx);
                    }
                }
                EditorEvent::Cancel => cx.emit(PegboardEvent::Close),
            }),
        ];
        // Live previews: ask again every second until the pegboard closes.
        cx.spawn(async move |this, cx| {
            loop {
                let asked = this.update(cx, |this, cx| this.state.read(cx).conn.send_msg(ClientMsg::Pegboard));
                if asked.is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        })
        .detach();
        Self { here, state, filter, selected: 0, query: String::new(), _subscriptions: subscriptions }
    }

    /// Every bench, open ones first, each with the tabs the filter matches.
    fn entries(&self, cx: &Context<Self>) -> Vec<Entry> {
        let state = self.state.read(cx);
        let live = state.tree.benches.iter().map(|b| {
            let status = if state.has_window(b.id) { Status::Open } else { Status::Detached };
            (b.clone(), status, None)
        });
        let stored = cx.try_global::<PegboardData>().map(|d| d.0.stored.as_slice()).unwrap_or_default();
        let stored = stored.iter().filter(|s| state.tree.bench(s.bench.id).is_none());
        let stored = stored.map(|s| (s.bench.clone(), Status::Stored, Some(s.saved.clone())));
        let mut all: Vec<(Bench, Status, Option<String>)> = live.chain(stored).collect();
        all.sort_by_key(|(_, status, _)| match status {
            Status::Open => 0,
            Status::Detached => 1,
            Status::Stored => 2,
        });

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

    /// Goes to a bench, at `tab` if given: raises its window, opens one for
    /// it, or loads it.
    fn jump(&mut self, entry: &Entry, tab: Option<TabId>, cx: &mut Context<Self>) {
        let bench = entry.bench.id;
        let conn = self.state.read(cx).conn.clone();
        match entry.status {
            Status::Stored => conn.send_msg(ClientMsg::Show(ShowTarget::Load { name: entry.bench.name.clone() })),
            Status::Open | Status::Detached => {}
        }
        if let Some(tab) = tab {
            conn.send_msg(ClientMsg::FocusTab { tab });
        }
        if entry.status != Status::Stored && bench != self.here {
            self.state.update(cx, |state, cx| state.show(bench, cx));
        }
        cx.emit(PegboardEvent::Close);
    }

    fn delete(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Delete the bench “{name}”?"),
            Some("Its saved file, scrollback and launcher are removed. This cannot be undone."),
            &["Delete Bench", "Cancel"],
            cx,
        );
        let conn = self.state.read(cx).conn.clone();
        cx.spawn(async move |_, _| {
            if let Ok(0) = answer.await {
                conn.send_msg(ClientMsg::DeleteBench { name });
            }
        })
        .detach();
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

    fn render_bench(&self, entry: &Entry, first: usize, previews: &HashMap<PaneId, Vec<String>>, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let id = bench.id;
        let (status, action) = match entry.status {
            Status::Open if id == self.here => ("this window".to_string(), None),
            Status::Open => ("open".to_string(), Some("Go to")),
            Status::Detached => ("detached, still running".to_string(), Some("Open")),
            Status::Stored => {
                let saved = entry.saved.as_deref().map(short_time).unwrap_or_default();
                (format!("stored, saved {saved}"), Some("Load"))
            }
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
            .when(bench.closing, |el| el.child(div().text_size(px(12.0)).text_color(hsla(accent)).child("closing…")))
            .child(div().flex_1());
        if let Some(label) = action {
            let entry_bench = bench.clone();
            let (status, saved) = (entry.status, entry.saved.clone());
            header = header.child(
                div()
                    .id(SharedString::from(format!("go-{id}")))
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .bg(hsla_alpha(accent, 0.2))
                    .text_color(hsla(accent))
                    .hover(|s| s.bg(hsla_alpha(accent, 0.35)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let entry = Entry { bench: entry_bench.clone(), status, saved: saved.clone(), tabs: Vec::new() };
                        this.jump(&entry, None, cx);
                    })),
            );
        }
        if entry.status == Status::Stored {
            let name = bench.name.clone();
            header = header.child(
                div()
                    .id(SharedString::from(format!("delete-{id}")))
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .text_color(hsla(ui.text_dim))
                    .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(Rgb::hex(0xF0566A))))
                    .child("Delete…")
                    .on_click(cx.listener(move |this, _, window, cx| this.delete(name.clone(), window, cx))),
            );
        }

        let mut cards = div().flex().flex_wrap().gap_3();
        for (i, tab_id) in entry.tabs.iter().enumerate() {
            let Some(tab) = bench.tabs.iter().find(|t| t.id == *tab_id) else { continue };
            cards = cards.child(self.render_tab(entry, tab, first + i, previews, ui, cx));
        }

        let live = entry.status != Status::Stored;
        div()
            .id(SharedString::from(format!("bench-{id}")))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(hsla(ui.divider))
            .when(live, |el| {
                el.drag_over::<DraggedTab>(move |s, _, _, _| s.border_color(hsla(accent)))
                    .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                        this.state.read(cx).conn.send_msg(ClientMsg::MoveTab { tab: d.tab, bench: id, index: usize::MAX });
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

        let entry_bench = bench.clone();
        let (status, saved, tab_id) = (entry.status, entry.saved.clone(), tab.id);
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
            .when(status != Status::Stored, |el| {
                el.on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                let entry = Entry { bench: entry_bench.clone(), status, saved: saved.clone(), tabs: Vec::new() };
                this.jump(&entry, Some(tab_id), cx);
            }))
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
            list = list.child(div().text_color(hsla(ui.text_dim)).child("Nothing matches."));
        }

        let search = div()
            .flex()
            .items_center()
            .gap_2()
            .child(svg().path(icon("pegboard")).size(px(18.0)).text_color(hsla(ui.text_dim)))
            .child(div().flex_1().max_w(px(480.0)).text_size(px(14.0)).child(self.filter.clone()))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(hsla(ui.text_dim))
                    .child("Type to filter by bench, tab, tag, folder or tool · ↑↓ and Enter to go · drag a tab onto a bench to move it · Esc to close"),
            );

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

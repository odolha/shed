//! The pegboard, the whole of the Shed window: every bench, open, hidden or
//! closed, with its tabs and a preview of each. Benches are made, opened,
//! hidden, closed, deleted, renamed and recoloured here; tabs are renamed,
//! tagged, set up, opened, closed and moved. Type to filter, arrows and
//! Enter to go to a tab, drag a tab onto another to put it there.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, ClickEvent, Context, Entity, Focusable, Hsla, KeyDownEvent, MouseButton, MouseDownEvent,
    PromptLevel, SharedString, Subscription, TextAlign, TextRun, Window, canvas, div, point, prelude::*, px, svg,
};
use shed_model::{Bench, BenchColor, BenchId, PaneId, Rgb, Tab, TabId};
use shed_proto::{BenchRef, ClientMsg, PegboardInfo, SavedEdit, ShowTarget};
use shed_save::home_relative;

use crate::assets::icon;
use crate::bench_window::{DraggedTab, with_tag};
use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu::{self, Menu, MenuEntry, MenuHost, danger, more};
use crate::setup_panel::{PanelEvent, SetupPanel};
use crate::state::AppState;
use crate::theme::{UiColors, hsla, hsla_alpha};

/// How often the previews refresh while the Shed window is active (so they
/// look live), and while it is only on screen. A window nobody can see asks
/// for none: it refreshes when it shows again, and only checks now and then
/// that it has not missed that. The board redraws only when a preview changed.
const REFRESH: Duration = Duration::from_millis(250);
const IDLE_REFRESH: Duration = Duration::from_secs(1);
const HIDDEN_CHECK: Duration = Duration::from_secs(5);
const CARD_WIDTH: f32 = 300.0;
/// Every tab of the compact view is this wide, so they line up in columns.
const CHIP_WIDTH: f32 = 230.0;

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

impl Entry {
    fn key(&self) -> BenchKey {
        match self.status {
            Status::Closed => BenchKey::Closed(self.bench.name.clone()),
            Status::Open | Status::Hidden => BenchKey::Running(self.bench.id),
        }
    }
}

/// A bench as edits address it: a running one by id, a closed one by name.
#[derive(Clone, PartialEq)]
enum BenchKey {
    Running(BenchId),
    Closed(String),
}

impl BenchKey {
    fn to_ref(&self) -> BenchRef {
        match self {
            BenchKey::Running(id) => BenchRef::Running(*id),
            BenchKey::Closed(name) => BenchRef::Saved(name.clone()),
        }
    }
}

/// The payload of a bench being dragged by its grip.
#[derive(Clone)]
struct DraggedBench {
    id: BenchId,
    name: String,
}

impl Render for DraggedBench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::current_ui(cx);
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(hsla(ui.tab_active))
            .border_1()
            .border_color(hsla(ui.divider))
            .text_size(px(14.0))
            .text_color(hsla(ui.text))
            .child(self.name.clone())
    }
}

/// What can change about a tab, whether its bench runs or not.
enum TabChange {
    Rename(Option<String>),
    Tags(Vec<shed_model::Tag>),
    Private(bool),
}

/// What the inline text field is editing.
#[derive(Clone, PartialEq)]
enum Edit {
    BenchName(BenchKey),
    Badge(BenchKey),
    TabName(TabId),
    AddTag(TabId),
    /// A colour of one's own: `#rrggbb` or a colour's name.
    Color(BenchKey),
    /// A new link for the bench: `name url`.
    AddLink(BenchKey),
    /// The bench's link at this index.
    ChangeLink(BenchKey, usize),
    /// The name of the bench's link at this index.
    RenameLink(BenchKey, usize),
}

fn menu_item(label: impl Into<SharedString>, action: impl Fn(&mut Pegboard, &mut Window, &mut Context<Pegboard>) + 'static) -> MenuEntry<Pegboard> {
    menu::item(label, action)
}

pub struct Pegboard {
    state: Entity<AppState>,
    filter: Entity<LineEditor>,
    /// Position in the list of shown tabs.
    selected: usize,
    query: String,
    menu: Option<Menu<Self>>,
    editing: Option<(Edit, Entity<LineEditor>, Subscription)>,
    /// The Setup panel of one tab, over the right side.
    setup: Option<(Entity<SetupPanel>, Subscription)>,
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
            // Fresh previews the moment Shed comes to the front, or into view.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.refresh(cx);
                }
            }),
            cx.observe_window_visibility(window, |this, visibility, _, cx| {
                if visibility.is_visible() {
                    this.refresh(cx);
                }
            }),
        ];
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let next = this.update_in(cx, |this, window, cx| {
                    if !window.is_visible() {
                        return HIDDEN_CHECK;
                    }
                    this.refresh(cx);
                    // The compact view shows no previews: only the list needs keeping.
                    let compact = this.state.read(cx).settings.config.window.compact_pegboard;
                    if window.is_window_active() && !compact { REFRESH } else { IDLE_REFRESH }
                });
                let Ok(next) = next else { break };
                cx.background_executor().timer(next).await;
            }
        })
        .detach();
        Self {
            state,
            filter,
            selected: 0,
            query: String::new(),
            menu: None,
            editing: None,
            setup: None,
            _subscriptions: subscriptions,
        }
    }

    fn refresh(&self, cx: &Context<Self>) {
        self.send(ClientMsg::Pegboard, cx);
    }

    fn send(&self, msg: ClientMsg, cx: &Context<Self>) {
        self.state.read(cx).conn.send_msg(msg);
    }

    /// A tab and its bench, running or closed.
    fn find_tab(&self, tab: TabId, cx: &Context<Self>) -> Option<(BenchKey, Tab)> {
        let state = self.state.read(cx);
        for b in &state.tree.benches {
            if let Some(t) = b.tabs.iter().find(|t| t.id == tab) {
                return Some((BenchKey::Running(b.id), t.clone()));
            }
        }
        let stored = cx.try_global::<PegboardData>()?;
        stored.0.stored.iter().filter(|s| state.tree.bench(s.bench.id).is_none()).find_map(|s| {
            let t = s.bench.tabs.iter().find(|t| t.id == tab)?;
            Some((BenchKey::Closed(s.bench.name.clone()), t.clone()))
        })
    }

    /// Changes a tab: through the keeper if its bench runs, in its saved file if not.
    fn change_tab(&mut self, tab: TabId, change: TabChange, cx: &mut Context<Self>) {
        let Some((key, _)) = self.find_tab(tab, cx) else { return };
        let msg = match key {
            BenchKey::Running(_) => match change {
                TabChange::Rename(name) => ClientMsg::RenameTab { tab, name },
                TabChange::Tags(tags) => ClientMsg::SetTags { tab, tags },
                TabChange::Private(private) => ClientMsg::SetPrivate { tab, private },
            },
            BenchKey::Closed(name) => ClientMsg::EditSaved {
                name,
                edit: match change {
                    TabChange::Rename(name) => SavedEdit::RenameTab { tab, name },
                    TabChange::Tags(tags) => SavedEdit::TabTags { tab, tags },
                    TabChange::Private(private) => SavedEdit::TabPrivate { tab, private },
                },
            },
        };
        self.send(msg, cx);
        self.refresh(cx);
    }

    /// Closes a running bench's tab, or removes a closed one's, after asking.
    fn close_tab(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, t)) = self.find_tab(tab, cx) else { return };
        let conn = self.state.read(cx).conn.clone();
        crate::state::close_tab(&conn, &t, key.to_ref(), window, cx);
    }

    /// Asks for a folder and adds it to a bench as a project tab.
    fn new_project_tab(&mut self, key: &BenchKey, cx: &mut Context<Self>) {
        let picked = crate::state::pick_folder(cx);
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let picked = picked.await;
            this.update(cx, |this, cx| match picked {
                Ok(Some(root)) => {
                    let msg = match key {
                        BenchKey::Running(bench) => ClientMsg::NewProjectTab { bench, root },
                        BenchKey::Closed(name) => ClientMsg::EditSaved { name, edit: SavedEdit::NewProjectTab { root } },
                    };
                    this.send(msg, cx);
                    this.refresh(cx);
                }
                Ok(None) => {}
                Err(e) => this.state.update(cx, |state, cx| {
                    state.notice = Some(format!("No folder picker: {e}. Use “Open Folder as Project” in a terminal's menu, or `shed project`."));
                    cx.notify();
                }),
            })
            .ok();
        })
        .detach();
    }

    fn new_tab(&mut self, key: &BenchKey, cx: &mut Context<Self>) {
        self.send(
            match key {
                BenchKey::Running(bench) => ClientMsg::NewTab { bench: *bench, cwd: None },
                BenchKey::Closed(name) => ClientMsg::EditSaved { name: name.clone(), edit: SavedEdit::NewTab },
            },
            cx,
        );
        self.refresh(cx);
    }

    /// Puts a tab at `index` in a bench. Between running benches it moves with
    /// whatever runs in it; a closed bench's tabs move within it.
    fn move_tab(&mut self, tab: TabId, to: &BenchKey, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((from, t)) = self.find_tab(tab, cx) else { return };
        let to_name = match to {
            BenchKey::Running(id) => self.state.read(cx).tree.bench(*id).map(|b| b.name.clone()).unwrap_or_default(),
            BenchKey::Closed(name) => name.clone(),
        };
        let conn = self.state.read(cx).conn.clone();
        crate::state::move_tab(&conn, tab, &t.display_title(), from.to_ref(), to.to_ref(), &to_name, index, window, cx);
    }

    /// Every other bench, to move a tab to.
    fn move_menu(&mut self, tab: TabId, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some((from, _)) = self.find_tab(tab, cx) else { return Vec::new() };
        let state = self.state.read(cx);
        state
            .tab_destinations(&from.to_ref(), cx)
            .into_iter()
            .map(|d| {
                let key = match d.to {
                    BenchRef::Running(id) => BenchKey::Running(id),
                    BenchRef::Saved(name) => BenchKey::Closed(name),
                };
                let label = if d.closed { format!("{}  (closed)", d.name) } else { d.name };
                menu_item(label, move |this, window, cx| this.move_tab(tab, &key, usize::MAX, window, cx)).icon("bench")
            })
            .collect()
    }

    /// Moves bench `id` to where bench `to` is, every bench keeping its place
    /// in the order from then on.
    fn move_bench(&mut self, id: BenchId, to: BenchId, cx: &mut Context<Self>) {
        if id == to {
            return;
        }
        let query = std::mem::take(&mut self.query);
        let mut order: Vec<BenchId> = self.entries(cx).iter().map(|e| e.bench.id).collect();
        self.query = query;
        let (Some(from), Some(target)) = (order.iter().position(|b| *b == id), order.iter().position(|b| *b == to)) else { return };
        order.remove(from);
        order.insert(target, id);
        // At once, before the keeper's next answer.
        if let Some(mut data) = cx.try_global::<PegboardData>().map(|d| d.0.clone()) {
            data.order = order.clone();
            cx.set_global(PegboardData(data));
        }
        self.send(ClientMsg::SetBenchOrder(order), cx);
        cx.notify();
    }

    /// Every bench in order, each with the tabs the filter matches.
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
        // In the order the user arranged; the rest by name. Either way a
        // bench stays where it is as it opens and closes.
        let order = cx.try_global::<PegboardData>().map(|d| d.0.order.as_slice()).unwrap_or_default();
        all.sort_by_key(|(b, _, _)| (order.iter().position(|id| *id == b.id).unwrap_or(usize::MAX), b.name.to_lowercase()));

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

    fn set_color(&mut self, key: &BenchKey, color: BenchColor, cx: &mut Context<Self>) {
        self.send(
            match key {
                BenchKey::Running(bench) => ClientMsg::SetBenchColor { bench: *bench, color },
                BenchKey::Closed(name) => ClientMsg::EditSaved { name: name.clone(), edit: SavedEdit::Color(color) },
            },
            cx,
        );
        self.refresh(cx);
    }

    fn start_edit(&mut self, edit: Edit, initial: String, window: &mut Window, cx: &mut Context<Self>) {
        // Tab titles sit in narrow cards: no room for the hint there.
        let narrow = matches!(edit, Edit::TabName(_) | Edit::AddTag(_));
        let editor = cx.new(|cx| {
            let editor = LineEditor::selected(initial, cx).submit_on_blur(window, cx);
            if narrow { editor.without_hint() } else { editor }
        });
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let sub = cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, window, cx| {
            let Some((edit, _, _)) = this.editing.take() else { return };
            if let EditorEvent::Submit(text) = event {
                this.submit(edit, text.trim().to_string(), cx);
            }
            window.focus(&this.filter.read(cx).focus_handle(cx), cx);
            cx.notify();
        });
        self.editing = Some((edit, editor, sub));
        cx.notify();
    }

    fn submit(&mut self, edit: Edit, text: String, cx: &mut Context<Self>) {
        let msg = match edit {
            Edit::BenchName(_) if text.is_empty() => return,
            Edit::BenchName(BenchKey::Running(bench)) => ClientMsg::RenameBench { bench, name: text },
            Edit::BenchName(BenchKey::Closed(name)) => ClientMsg::EditSaved { name, edit: SavedEdit::Rename(text) },
            Edit::Badge(key) => {
                let badge: String = text.chars().filter(|c| !c.is_whitespace()).take(2).collect();
                let badge = (!badge.is_empty()).then_some(badge);
                match key {
                    BenchKey::Running(bench) => ClientMsg::SetBadge { bench, badge },
                    BenchKey::Closed(name) => ClientMsg::EditSaved { name, edit: SavedEdit::Badge(badge) },
                }
            }
            // Empty goes back to the title the shell sets.
            Edit::TabName(tab) => return self.change_tab(tab, TabChange::Rename((!text.is_empty()).then_some(text)), cx),
            Edit::AddTag(tab) => {
                let Some((_, t)) = self.find_tab(tab, cx) else { return };
                let Some(tags) = with_tag(&t.tags, &text) else { return };
                return self.change_tab(tab, TabChange::Tags(tags), cx);
            }
            Edit::AddLink(key) => return self.submit_link(key, None, &text, cx),
            Edit::ChangeLink(key, i) => return self.submit_link(key, Some(i), &text, cx),
            Edit::RenameLink(key, i) => {
                if let Some(links) = crate::links::renamed(&self.links_of(&key, cx), i, &text) {
                    self.set_links(&key, links, cx);
                }
                return;
            }
            Edit::Color(key) => {
                match BenchColor::parse(&text) {
                    Some(color) => self.set_color(&key, color, cx),
                    None => self.state.update(cx, |state, cx| {
                        state.notice = Some(format!("“{text}” is not a colour: use #rrggbb or a name such as teal"));
                        cx.notify();
                    }),
                }
                return;
            }
        };
        self.send(msg, cx);
        self.refresh(cx);
    }

    fn submit_link(&mut self, key: BenchKey, at: Option<usize>, text: &str, cx: &mut Context<Self>) {
        if let Some(link) = crate::links::parse(text) {
            let links = crate::links::with(&self.links_of(&key, cx), link, at);
            self.set_links(&key, links, cx);
        }
    }

    /// A bench's links, running or closed.
    fn links_of(&self, key: &BenchKey, cx: &Context<Self>) -> Vec<shed_model::ProjectLink> {
        match key {
            BenchKey::Running(id) => self.state.read(cx).tree.bench(*id).map(|b| b.links.clone()).unwrap_or_default(),
            BenchKey::Closed(name) => cx
                .try_global::<PegboardData>()
                .and_then(|d| d.0.stored.iter().find(|s| s.bench.name == *name).map(|s| s.bench.links.clone()))
                .unwrap_or_default(),
        }
    }

    fn set_links(&mut self, key: &BenchKey, links: Vec<shed_model::ProjectLink>, cx: &mut Context<Self>) {
        let msg = match key {
            BenchKey::Running(bench) => ClientMsg::SetBenchLinks { bench: *bench, links },
            BenchKey::Closed(name) => ClientMsg::EditSaved { name: name.clone(), edit: SavedEdit::Links(links) },
        };
        self.send(msg, cx);
        self.refresh(cx);
    }

    /// A bench's links as chips (click opens, right-click for more), and the
    /// field for one being added or changed.
    fn render_links(&self, entry: &Entry, ui: &UiColors, cx: &mut Context<Self>) -> gpui::Div {
        let key = entry.key();
        let accent = entry.bench.color.accent();
        let links = entry.bench.links.clone();
        let id = entry.bench.id;
        let mut row = div().flex().flex_none().items_center().gap_1();
        for (i, link) in entry.bench.links.iter().enumerate() {
            if let Some(editor) = self.editor_for(&Edit::ChangeLink(key.clone(), i)).or_else(|| self.editor_for(&Edit::RenameLink(key.clone(), i))) {
                row = row.child(div().w(px(260.0)).text_size(px(12.0)).child(editor));
                continue;
            }
            let (url, links, k) = (link.url.clone(), links.clone(), key.clone());
            row = row.child(
                crate::links::chip(format!("link-{id}-{i}"), link, accent, ui)
                    .tooltip(menu::tooltip("Open the link (right-click for more)"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        crate::links::open(url.clone(), false, cx)
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        let (k1, k2, k3) = (k.clone(), k.clone(), k.clone());
                        let initial = links.get(i).map(crate::links::unparse).unwrap_or_default();
                        let name = links.get(i).map(|l| l.name.clone()).unwrap_or_default();
                        let entries = crate::links::menu(
                            links.clone(),
                            i,
                            move |this: &mut Self, links, cx| this.set_links(&k1, links, cx),
                            move |this: &mut Self, window, cx| this.start_edit(Edit::RenameLink(k3.clone(), i), name.clone(), window, cx),
                            move |this: &mut Self, window, cx| this.start_edit(Edit::ChangeLink(k2.clone(), i), initial.clone(), window, cx),
                        );
                        this.open_menu(e.position, entries, cx);
                    })),
            );
        }
        if let Some(editor) = self.editor_for(&Edit::AddLink(key)) {
            row = row.child(div().w(px(260.0)).text_size(px(12.0)).child(editor)).child(div().text_size(px(11.0)).text_color(hsla(ui.text_dim)).child("name url"));
        }
        row
    }

    fn editor_for(&self, edit: &Edit) -> Option<Entity<LineEditor>> {
        self.editing.as_ref().filter(|(e, _, _)| e == edit).map(|(_, e, _)| e.clone())
    }

    fn open_setup(&mut self, bench: BenchId, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.new(|cx| SetupPanel::new(bench, Some(tab), self.state.clone(), cx));
        let sub = cx.subscribe_in(&panel, window, |this, _, event: &PanelEvent, window, cx| {
            if let PanelEvent::Close = event {
                this.setup = None;
            }
            window.focus(&this.filter.read(cx).focus_handle(cx), cx);
            cx.notify();
        });
        self.setup = Some((panel, sub));
        cx.notify();
    }

    fn bench_menu(&self, entry: &Entry) -> Vec<MenuEntry<Self>> {
        let key = entry.key();
        let bench = &entry.bench;
        let mut entries = Vec::new();
        let e = entry.clone();
        let open_label = if entry.status == Status::Open { "Go to Bench" } else { "Open Bench" };
        let open_icon = if entry.status == Status::Closed { "bench-load" } else { "go" };
        entries.push(menu_item(open_label, move |this, _, cx| this.open(&e, None, cx)).icon(open_icon));
        let k = key.clone();
        entries.push(menu_item("New Tab", move |this, _, cx| this.new_tab(&k, cx)).icon("tab-new"));
        let k = key.clone();
        entries.push(menu_item("New Project Tab…", move |this, _, cx| this.new_project_tab(&k, cx)).icon("project"));
        entries.push(MenuEntry::Separator);
        let k = key.clone();
        entries.push(menu_item("Add Link…", move |this, window, cx| this.start_edit(Edit::AddLink(k.clone()), String::new(), window, cx)).icon("link"));
        let (k, name) = (key.clone(), bench.name.clone());
        entries.push(menu_item("Rename…", move |this, window, cx| this.start_edit(Edit::BenchName(k.clone()), name.clone(), window, cx)).icon("pencil"));
        let (k, badge) = (key.clone(), bench.badge.clone().unwrap_or_default());
        entries.push(menu_item("Set Badge…", move |this, window, cx| this.start_edit(Edit::Badge(k.clone()), badge.clone(), window, cx)));
        entries.push(MenuEntry::Separator);
        entries.extend(self.color_entries(&key, bench.color));
        entries.push(MenuEntry::Separator);
        if entry.status == Status::Open {
            let id = bench.id;
            entries.push(menu_item("Hide", move |this, _, cx| this.hide(id, cx)).icon("detach"));
        }
        if entry.status != Status::Closed {
            let b = bench.clone();
            entries.push(danger("Close Bench…", move |this: &mut Self, window, cx| this.close(&b, window, cx)).icon("power"));
        }
        entries.push(MenuEntry::Separator);
        let id = bench.id;
        entries.push(menu_item("Move Up", move |this, _, cx| this.move_bench_by(id, -1, cx)));
        entries.push(menu_item("Move Down", move |this, _, cx| this.move_bench_by(id, 1, cx)));
        entries.push(MenuEntry::Separator);
        let name = bench.name.clone();
        entries.push(danger("Delete Bench…", move |this: &mut Self, window, cx| this.delete(name.clone(), window, cx)).icon("trash"));
        entries
    }

    /// Moves a bench one place up (-1) or down (1).
    fn move_bench_by(&mut self, id: BenchId, step: isize, cx: &mut Context<Self>) {
        let query = std::mem::take(&mut self.query);
        let order: Vec<BenchId> = self.entries(cx).iter().map(|e| e.bench.id).collect();
        self.query = query;
        let Some(at) = order.iter().position(|b| *b == id) else { return };
        let Some(to) = at.checked_add_signed(step).and_then(|i| order.get(i)) else { return };
        self.move_bench(id, *to, cx);
    }

    fn color_entries(&self, key: &BenchKey, current: BenchColor) -> Vec<MenuEntry<Self>> {
        let palette = BenchColor::NAMED
            .into_iter()
            .map(|color| {
                let key = key.clone();
                menu::swatch_of(color.accent(), color.name().unwrap_or_default(), current == color, move |this: &mut Self, _, cx| {
                    this.set_color(&key, color, cx)
                })
            })
            .collect();
        let (k, hex) = (key.clone(), format!("#{:06X}", current.accent().to_hex()));
        vec![
            MenuEntry::Palette(palette),
            menu_item("Custom Colour…", move |this, window, cx| this.start_edit(Edit::Color(k.clone()), hex.clone(), window, cx)).icon("bench-color"),
        ]
    }

    fn tab_menu(&self, entry: &Entry, tab: &Tab) -> Vec<MenuEntry<Self>> {
        let tab_id = tab.id;
        let e = entry.clone();
        let go = if entry.status == Status::Closed { "Open at This Tab" } else { "Go to Tab" };
        let mut entries = vec![menu_item(go, move |this, _, cx| this.open(&e, Some(tab_id), cx)).icon("go")];
        let bench = entry.bench.id;
        let title = tab.display_title();
        entries.push(menu_item("Rename…", move |this, window, cx| this.start_edit(Edit::TabName(tab_id), title.clone(), window, cx)).icon("pencil"));
        if tab.project_info().is_none() && tab.ai_info().is_none() {
            entries.push(menu_item("Setup…", move |this, window, cx| this.open_setup(bench, tab_id, window, cx)).icon("rack"));
        }
        entries.push(menu_item("Add Tag…", move |this, window, cx| this.start_edit(Edit::AddTag(tab_id), String::new(), window, cx)).icon("tag"));
        for tag in &tab.tags {
            let (label, tint) = (tag.label.clone(), tag.tint_tab);
            let l = label.clone();
            entries.push(menu_item(format!("Tint for “{label}”{}", if tint { "  ✓" } else { "" }), move |this, _, cx| {
                let Some((_, t)) = this.find_tab(tab_id, cx) else { return };
                let tags = t.tags.into_iter().map(|mut t| {
                    if t.label == l {
                        t.tint_tab = !t.tint_tab;
                    }
                    t
                });
                this.change_tab(tab_id, TabChange::Tags(tags.collect()), cx);
            }));
            entries.push(
                menu_item(format!("Remove Tag “{label}”"), move |this, _, cx| {
                    let Some((_, t)) = this.find_tab(tab_id, cx) else { return };
                    let tags = t.tags.into_iter().filter(|t| t.label != label).collect();
                    this.change_tab(tab_id, TabChange::Tags(tags), cx);
                })
                .icon("close"),
            );
        }
        let private = tab.private;
        entries.push(
            menu_item(if private { "Private  ✓" } else { "Private" }, move |this, _, cx| {
                this.change_tab(tab_id, TabChange::Private(!private), cx);
            })
            .icon("private"),
        );
        entries.push(more("Move to Bench", move |this: &mut Self, cx| this.move_menu(tab_id, cx)).icon("bench"));
        entries.push(MenuEntry::Separator);
        let close = if entry.status == Status::Closed { "Remove Tab…" } else { "Close Tab…" };
        entries.push(danger(close, move |this: &mut Self, window, cx| this.close_tab(tab_id, window, cx)).icon("close"));
        entries
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

    #[allow(clippy::too_many_arguments)]
    fn button(
        &self,
        id: String,
        glyph: &'static str,
        label: &str,
        color: Rgb,
        filled: bool,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let ink = self.state.read(cx).settings.ui.ink(color);
        div()
            .id(SharedString::from(id))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_sm()
            .text_size(px(12.0))
            .cursor_pointer()
            .text_color(hsla(ink))
            .when(filled, |d| d.bg(hsla_alpha(color, 0.2)))
            .hover(move |s| s.bg(hsla_alpha(color, 0.3)))
            .child(svg().path(icon(glyph)).size(px(13.0)).text_color(hsla(ink)))
            .child(label.to_string())
            .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
            .into_any_element()
    }

    /// A pencil shown while the pointer is over `group`: names can be edited.
    fn rename_button(
        &self,
        id: String,
        group: SharedString,
        ui: &UiColors,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(SharedString::from(id))
            .flex_none()
            .p_0p5()
            .rounded_sm()
            .invisible()
            .group_hover(group, |s| s.visible())
            .cursor_pointer()
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .tooltip(menu::tooltip("Rename"))
            .child(svg().path(icon("pencil")).size(px(12.0)).text_color(hsla(ui.text_dim)))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                on(this, window, cx)
            }))
            .into_any_element()
    }

    fn render_bench(&self, entry: &Entry, first: usize, previews: &HashMap<PaneId, Vec<String>>, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let id = bench.id;
        let key = entry.key();
        // State at a glance: a coloured badge, and what it means beside it.
        let palette = &self.state.read(cx).settings.palette;
        let (state_color, state_label, state_note) = match entry.status {
            Status::Open => (palette.ansi[2], "Open", String::new()),
            Status::Hidden => (palette.ansi[3], "Hidden", "running without a window".to_string()),
            Status::Closed => (ui.text_dim, "Closed", format!("saved {}", entry.saved.as_deref().map(short_time).unwrap_or_default())),
        };
        let state_badge = div()
            .flex()
            .items_center()
            .gap_1()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .text_size(px(11.0))
            .bg(hsla_alpha(state_color, 0.16))
            .text_color(hsla(state_color))
            .child(div().size(px(7.0)).rounded_full().when(entry.status == Status::Closed, |d| d.border_1().border_color(hsla(state_color))).when(entry.status != Status::Closed, |d| d.bg(hsla(state_color))))
            .child(state_label);
        let tab_count = bench.tabs.len();

        // The colour dot picks the colour; a double-click on the name renames.
        let (k, current) = (key.clone(), bench.color);
        let dot = div()
            .id(SharedString::from(format!("color-{id}")))
            .size(px(12.0))
            .rounded_full()
            .flex_none()
            .cursor_pointer()
            .bg(hsla(accent))
            .hover(|s| s.border_2().border_color(hsla(ui.text_dim)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let entries = this.color_entries(&k, current);
                this.open_menu(e.position, entries, cx);
            }));
        let name = match self.editor_for(&Edit::BenchName(key.clone())) {
            Some(editor) => div().w(px(240.0)).child(editor).into_any_element(),
            None => {
                let (k, n) = (key.clone(), bench.name.clone());
                let (k2, n2) = (key.clone(), bench.name.clone());
                let group = SharedString::from(format!("bench-name-{id}"));
                div()
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id(SharedString::from(format!("name-{id}")))
                            .text_size(px(15.0))
                            .text_color(hsla(ui.text))
                            .cursor_text()
                            .child(bench.name.clone())
                            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                if e.click_count() >= 2 {
                                    this.start_edit(Edit::BenchName(k.clone()), n.clone(), window, cx);
                                }
                            })),
                    )
                    .child(self.rename_button(format!("rename-bench-{id}"), group, ui, cx, move |this, window, cx| {
                        this.start_edit(Edit::BenchName(k2.clone()), n2.clone(), window, cx)
                    }))
                    .into_any_element()
            }
        };
        let badge = match self.editor_for(&Edit::Badge(key.clone())).or_else(|| self.editor_for(&Edit::Color(key.clone()))) {
            Some(editor) => Some(div().w(px(110.0)).child(editor).into_any_element()),
            None => bench.badge.clone().map(|badge| {
                div().px_1().rounded_sm().text_size(px(12.0)).bg(hsla_alpha(accent, 0.25)).text_color(hsla(ui.ink(accent))).child(badge).into_any_element()
            }),
        };
        // Dragging the grip moves the bench among the others.
        let grip = div()
            .id(SharedString::from(format!("grip-{id}")))
            .flex_none()
            .cursor_grab()
            .tooltip(menu::tooltip("Drag to move the bench"))
            .child(svg().path(icon("grip")).size(px(14.0)).text_color(hsla(ui.text_dim)))
            .on_drag(DraggedBench { id, name: bench.name.clone() }, |d, _, _, cx| cx.new(|_| d.clone()));
        let mut header = div()
            .flex()
            .items_center()
            .gap_2()
            .child(grip)
            .child(dot)
            .child(name)
            .children(badge)
            .child(state_badge)
            .child(div().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child(format!(
                "{}{tab_count} tab{}",
                if state_note.is_empty() { String::new() } else { format!("{state_note} · ") },
                if tab_count == 1 { "" } else { "s" }
            )))
            .child(self.render_links(entry, ui, cx))
            .child(div().flex_1());

        if bench.closing {
            header = header.child(div().text_size(px(12.0)).text_color(hsla(ui.ink(accent))).child("closing…"));
        } else {
            let (open_icon, open_label) = if entry.status == Status::Open { ("go", "Go to") } else { ("bench-load", "Open") };
            let e = entry.clone();
            header = header.child(self.button(format!("open-{id}"), open_icon, open_label, accent, true, cx, move |this, _, cx| this.open(&e, None, cx)));
            if entry.status == Status::Open {
                header = header.child(self.button(format!("hide-{id}"), "detach", "Hide", ui.text_dim, false, cx, move |this, _, cx| this.hide(id, cx)));
            }
            if entry.status != Status::Closed {
                let b = bench.clone();
                header = header.child(self.button(format!("close-{id}"), "power", "Close…", ui.text_dim, false, cx, move |this, window, cx| {
                    this.close(&b, window, cx)
                }));
            }
            let name = bench.name.clone();
            header = header.child(self.button(format!("delete-{id}"), "trash", "Delete…", ui.danger, false, cx, move |this, window, cx| {
                this.delete(name.clone(), window, cx)
            }));
            let e = entry.clone();
            header = header.child(
                div()
                    .id(SharedString::from(format!("more-{id}")))
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip("More: rename, badge, colour, new tab"))
                    .child(svg().path(icon("more")).size(px(15.0)).text_color(hsla(ui.text_dim)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        let entries = this.bench_menu(&e);
                        this.open_menu(ev.position, entries, cx);
                    })),
            );
        }

        let mut cards = div().flex().flex_wrap().gap_3();
        for (i, tab_id) in entry.tabs.iter().enumerate() {
            let Some(tab) = bench.tabs.iter().find(|t| t.id == *tab_id) else { continue };
            let index = bench.tabs.iter().position(|t| t.id == *tab_id).unwrap_or(i);
            cards = cards.child(self.render_tab(entry, tab, index, first + i, previews, ui, cx));
        }
        if !bench.closing {
            let k = key.clone();
            let ink = ui.ink(accent);
            cards = cards.child(
                div()
                    .id(SharedString::from(format!("new-tab-{id}")))
                    .w(px(120.0))
                    .h(px(60.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_md()
                    .border_1()
                    .border_dashed()
                    .border_color(hsla(ui.divider))
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .text_color(hsla(ui.text_dim))
                    .hover(move |s| s.border_color(hsla(accent)).text_color(hsla(ink)))
                    .child(svg().path(icon("tab-new")).size(px(14.0)).text_color(hsla(ui.text_dim)))
                    .child("New tab")
                    .on_click(cx.listener(move |this, _, _, cx| this.new_tab(&k, cx))),
            );
        }

        let e = entry.clone();
        let drop_key = key.clone();
        div()
            .id(SharedString::from(format!("bench-{id}")))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(hsla(ui.divider))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu(&e);
                this.open_menu(ev.position, entries, cx);
            }))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.border_color(hsla(accent)))
            .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| this.move_tab(d.tab, &drop_key, usize::MAX, window, cx)))
            .drag_over::<DraggedBench>(move |s, _, _, _| s.border_2().border_color(hsla(accent)))
            .on_drop(cx.listener(move |this, d: &DraggedBench, _, cx| this.move_bench(d.id, id, cx)))
            .child(header)
            .child(cards)
            .into_any_element()
    }

    /// The compact view of a bench: a header line (its colour, name, state,
    /// links, Open) and its tabs below in even columns, each with an icon.
    /// No borders: space and one colour (the bench's dot) carry the layout.
    fn render_bench_line(&self, entry: &Entry, first: usize, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let id = bench.id;
        let key = entry.key();
        let closed = entry.status == Status::Closed;
        let group = SharedString::from(format!("bench-line-{id}"));

        // The dot is the bench's colour (click to change it): filled while
        // it runs, a ring when closed.
        let (k, current) = (key.clone(), bench.color);
        let dot = div()
            .id(SharedString::from(format!("color-{id}")))
            .flex_none()
            .size(px(10.0))
            .rounded_full()
            .cursor_pointer()
            .when(closed, |d| d.border_2().border_color(hsla(accent)))
            .when(!closed, |d| d.bg(hsla(accent)))
            .tooltip(menu::tooltip("Colour"))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let entries = this.color_entries(&k, current);
                this.open_menu(e.position, entries, cx);
            }));
        let name = match self.editor_for(&Edit::BenchName(key.clone())) {
            Some(editor) => div().w(px(240.0)).child(editor).into_any_element(),
            None => {
                let (k, n, e) = (key.clone(), bench.name.clone(), entry.clone());
                div()
                    .id(SharedString::from(format!("name-{id}")))
                    .flex_none()
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(hsla(if closed { ui.text_dim } else { ui.text }))
                    .cursor_pointer()
                    .child(bench.name.clone())
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        if ev.click_count() >= 2 {
                            this.start_edit(Edit::BenchName(k.clone()), n.clone(), window, cx);
                        } else {
                            this.open(&e, None, cx);
                        }
                    }))
                    .into_any_element()
            }
        };
        let badge = match self.editor_for(&Edit::Badge(key.clone())).or_else(|| self.editor_for(&Edit::Color(key.clone()))) {
            Some(editor) => Some(div().w(px(110.0)).child(editor).into_any_element()),
            None => bench.badge.clone().map(|badge| {
                div().flex_none().px_1().rounded_sm().text_size(px(11.0)).bg(hsla_alpha(accent, 0.2)).text_color(hsla(ui.ink(accent))).child(badge).into_any_element()
            }),
        };
        let state = match entry.status {
            Status::Open => String::new(),
            Status::Hidden => "running, no window".into(),
            Status::Closed => format!("closed · saved {}", entry.saved.as_deref().map(short_time).unwrap_or_default()),
        };
        let grip = div()
            .id(SharedString::from(format!("grip-{id}")))
            .flex_none()
            .cursor_grab()
            .invisible()
            .group_hover(group.clone(), |s| s.visible())
            .tooltip(menu::tooltip("Drag to move the bench"))
            .child(svg().path(icon("grip")).size(px(12.0)).text_color(hsla(ui.text_dim)))
            .on_drag(DraggedBench { id, name: bench.name.clone() }, |d, _, _, cx| cx.new(|_| d.clone()));

        let mut header = div()
            .h(px(28.0))
            .flex()
            .items_center()
            .gap_2()
            .child(grip)
            .child(dot)
            .child(name)
            .children(badge)
            .when(!state.is_empty(), |d| d.child(div().flex_none().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child(state)))
            .child(div().flex_1())
            .child(self.render_quiet_links(entry, ui, cx));
        if bench.closing {
            header = header.child(div().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child("closing…"));
        } else {
            let (open_icon, open_label) = if entry.status == Status::Open { ("go", "Go to") } else { ("bench-load", "Open") };
            let e = entry.clone();
            header = header.child(self.quiet_button(format!("open-{id}"), open_icon, Some(open_label), ui, cx, move |this, _, cx| this.open(&e, None, cx)));
            let e = entry.clone();
            header = header.child(
                self.quiet_button(format!("more-{id}"), "more", None, ui, cx, |_, _, _| {})
                    .tooltip(menu::tooltip("More: hide, close, delete, rename, colour, links"))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        let entries = this.bench_menu(&e);
                        this.open_menu(ev.position, entries, cx);
                    })),
            );
        }

        // The tabs, in columns under the name.
        let mut tabs = div().pl(px(30.0)).flex().flex_wrap().gap_x_1p5().gap_y_0p5();
        for (i, tab_id) in entry.tabs.iter().enumerate() {
            let Some(tab) = bench.tabs.iter().find(|t| t.id == *tab_id) else { continue };
            let index = bench.tabs.iter().position(|t| t.id == *tab_id).unwrap_or(i);
            tabs = tabs.child(self.render_chip(entry, tab, index, first + i, ui, cx));
        }
        if !bench.closing {
            let k = key.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("new-tab-{id}")))
                    .h(px(26.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .text_color(hsla(ui.text_dim))
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .child(svg().path(icon("tab-new")).size(px(13.0)).text_color(hsla(ui.text_dim)))
                    .child("New tab")
                    .on_click(cx.listener(move |this, _, _, cx| this.new_tab(&k, cx))),
            );
        }

        let e = entry.clone();
        let drop_key = key.clone();
        div()
            .id(SharedString::from(format!("bench-{id}")))
            .group(group)
            .flex()
            .flex_col()
            .gap_1p5()
            .px_3()
            .pt_1p5()
            .pb_3()
            .rounded_lg()
            .border_1()
            .border_color(hsla(ui.divider))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu(&e);
                this.open_menu(ev.position, entries, cx);
            }))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(hsla_alpha(accent, 0.08)))
            .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| this.move_tab(d.tab, &drop_key, usize::MAX, window, cx)))
            .drag_over::<DraggedBench>(move |s, _, _, _| s.bg(hsla_alpha(accent, 0.08)))
            .on_drop(cx.listener(move |this, d: &DraggedBench, _, cx| this.move_bench(d.id, id, cx)))
            .child(header)
            .child(tabs)
            .into_any_element()
    }

    /// A small borderless button in the text's own colour.
    #[allow(clippy::too_many_arguments)]
    fn quiet_button(
        &self,
        id: String,
        glyph: &'static str,
        label: Option<&str>,
        ui: &UiColors,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let text = ui.text;
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .text_size(px(12.0))
            .cursor_pointer()
            .text_color(hsla(ui.text_dim))
            .hover(move |s| s.bg(hsla(ui.tab_hover)).text_color(hsla(text)))
            .child(svg().path(icon(glyph)).size(px(13.0)).text_color(hsla(ui.text_dim)))
            .children(label.map(str::to_string))
            .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
    }

    /// A bench's links on its header line: an icon and a name each, in the
    /// text's colours; click opens, right-click for more.
    fn render_quiet_links(&self, entry: &Entry, ui: &UiColors, cx: &mut Context<Self>) -> gpui::Div {
        let key = entry.key();
        let links = entry.bench.links.clone();
        let id = entry.bench.id;
        let text = ui.text;
        let mut row = div().flex().flex_none().items_center().gap_1();
        for (i, link) in entry.bench.links.iter().enumerate() {
            if let Some(editor) = self.editor_for(&Edit::ChangeLink(key.clone(), i)).or_else(|| self.editor_for(&Edit::RenameLink(key.clone(), i))) {
                row = row.child(div().w(px(260.0)).text_size(px(12.0)).child(editor));
                continue;
            }
            let (url, links, k) = (link.url.clone(), links.clone(), key.clone());
            row = row.child(
                div()
                    .id(SharedString::from(format!("link-{id}-{i}")))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .max_w(px(180.0))
                    .px_1p5()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .text_color(hsla(ui.text))
                    .hover(move |s| s.bg(hsla(ui.tab_hover)).text_color(hsla(text)))
                    .tooltip(menu::tooltip_text(link.url.clone()))
                    .child(svg().path(icon("link")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)))
                    .child(div().min_w_0().truncate().child(link.name.clone()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        crate::links::open(url.clone(), false, cx)
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        let (k1, k2, k3) = (k.clone(), k.clone(), k.clone());
                        let initial = links.get(i).map(crate::links::unparse).unwrap_or_default();
                        let name = links.get(i).map(|l| l.name.clone()).unwrap_or_default();
                        let entries = crate::links::menu(
                            links.clone(),
                            i,
                            move |this: &mut Self, links, cx| this.set_links(&k1, links, cx),
                            move |this: &mut Self, window, cx| this.start_edit(Edit::RenameLink(k3.clone(), i), name.clone(), window, cx),
                            move |this: &mut Self, window, cx| this.start_edit(Edit::ChangeLink(k2.clone(), i), initial.clone(), window, cx),
                        );
                        this.open_menu(e.position, entries, cx);
                    })),
            );
        }
        if let Some(editor) = self.editor_for(&Edit::AddLink(key)) {
            row = row.child(div().w(px(260.0)).text_size(px(12.0)).child(editor));
        }
        row
    }

    /// A tab in the compact view: an icon for what it is (or what it is
    /// doing), its title, tags as small dots, and what runs in it.
    #[allow(clippy::too_many_arguments)]
    fn render_chip(&self, entry: &Entry, tab: &Tab, index: usize, position: usize, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let running = entry.status != Status::Closed;
        let selected = position == self.selected;
        let active = tab.id == bench.active_tab && running;
        let tab_id = tab.id;
        let title = tab.display_title();
        let card_key = entry.key();
        let pane = tab.active();
        let tool = pane.and_then(|p| p.tool.as_ref());
        let (glyph, tint) = crate::bench_window::tab_icon(tab, accent, ui);
        // Beside the title: the program in front, else the folder's last part.
        let detail = match tool {
            Some(tool) => Some(tool.display()),
            None if tab.has_panes() => pane.and_then(|p| p.cwd.file_name()).map(|n| n.to_string_lossy().into_owned()).filter(|n| !title.contains(n.as_str())),
            None => None,
        };
        let full = match &detail {
            Some(d) => format!("{title} · {d}"),
            None => title.clone(),
        };
        let title_el = match self.editor_for(&Edit::TabName(tab_id)) {
            Some(editor) => div().flex_1().min_w_0().child(editor).into_any_element(),
            None => {
                let t = title.clone();
                div()
                    .id(SharedString::from(format!("title-{tab_id}")))
                    .flex_shrink(1.0)
                    .min_w_0()
                    .truncate()
                    .text_color(hsla(if running { ui.text } else { ui.text_dim }))
                    .child(title.clone())
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        if e.click_count() >= 2 {
                            cx.stop_propagation();
                            this.start_edit(Edit::TabName(tab_id), t.clone(), window, cx);
                        }
                    }))
                    .into_any_element()
            }
        };
        let tags = div().flex().flex_none().gap_0p5().children(tab.all_tags().map(|tag| div().size(px(6.0)).rounded_full().bg(hsla(tag.color))));
        let (e, e2) = (entry.clone(), entry.clone());
        let tab_for_menu = tab.clone();
        div()
            .id(SharedString::from(format!("tab-{tab_id}")))
            .relative()
            .h(px(26.0))
            .w(px(CHIP_WIDTH))
            .flex_none()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .rounded_md()
            .text_size(px(13.0))
            .cursor_pointer()
            // A light ground of their own, so tabs stand out from the bench.
            .when(selected, |d| d.bg(hsla_alpha(accent, 0.18)))
            .when(!selected, |d| d.bg(hsla_alpha(ui.text, 0.06)).hover(|s| s.bg(hsla_alpha(ui.text, 0.11))))
            .tooltip(menu::tooltip_text(full))
            .on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(hsla_alpha(accent, 0.14)))
            .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| {
                cx.stop_propagation();
                this.move_tab(d.tab, &card_key, index, window, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.open(&e, Some(tab_id), cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_tab(tab_id, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let entries = this.tab_menu(&e2, &tab_for_menu);
                this.open_menu(ev.position, entries, cx);
            }))
            // The tab the bench shows: a short bar in its colour.
            .when(active, |d| d.child(div().absolute().left_0().top(px(6.0)).h(px(14.0)).w(px(2.0)).rounded_full().bg(hsla(accent))))
            .child(svg().path(icon(glyph)).size(px(14.0)).flex_none().text_color(hsla(tint)))
            .child(title_el)
            .child(tags)
            .when(tab.private, |d| d.child(svg().path(icon("private")).size(px(11.0)).flex_none().text_color(hsla(ui.text_dim))))
            .child(div().flex_1())
            .children(detail.map(|d| div().flex_shrink_0().max_w(px(CHIP_WIDTH * 0.4)).truncate().text_size(px(11.5)).text_color(hsla(ui.text_dim)).child(d)))
            .into_any_element()
    }

    /// A tab's icons, title (double-click or the pencil renames it) and tags.
    fn tab_head(&self, tab: &Tab, accent: Rgb, ui: &UiColors, cx: &mut Context<Self>) -> gpui::Div {
        let tool = tab.active().and_then(|p| p.tool.as_ref());
        let waiting = tab.panes.iter().any(|p| p.waiting.is_some() || p.held.is_some());
        let tab_id = tab.id;
        let title = tab.display_title();
        let mut head = div().flex().items_center().gap_1p5().min_w_0();
        if tab.project_info().is_some() {
            head = head.child(svg().path(icon("project")).size(px(13.0)).flex_none().text_color(hsla(ui.ink(accent))));
        }
        if tab.ai_info().is_some() {
            head = head.child(svg().path(icon("ai")).size(px(13.0)).flex_none().text_color(hsla(ui.ink(accent))));
        }
        if tab.remote_info().is_some() {
            head = head.child(svg().path(icon("server")).size(px(13.0)).flex_none().text_color(hsla(ui.ink(accent))));
        }
        if waiting {
            head = head.child(svg().path(icon("tool-waiting")).size(px(12.0)).flex_none().text_color(hsla(accent)));
        } else if tool.is_some() {
            head = head.child(svg().path(icon("tool")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)));
        }
        if tab.private {
            head = head.child(svg().path(icon("private")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)));
        }
        head = match self.editor_for(&Edit::TabName(tab_id)) {
            Some(editor) => head.child(div().flex_1().min_w_0().child(editor)),
            None => {
                let (t, t2) = (title.clone(), title.clone());
                let group = SharedString::from(format!("tab-title-{tab_id}"));
                head.child(
                    div()
                        .group(group.clone())
                        .flex()
                        .items_center()
                        .gap_1()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .id(SharedString::from(format!("title-{tab_id}")))
                                .min_w_0()
                                .truncate()
                                .text_color(hsla(ui.text))
                                .child(title.clone())
                                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                    if e.click_count() >= 2 {
                                        cx.stop_propagation();
                                        this.start_edit(Edit::TabName(tab_id), t.clone(), window, cx);
                                    }
                                })),
                        )
                        .child(self.rename_button(format!("rename-tab-{tab_id}"), group, ui, cx, move |this, window, cx| {
                            this.start_edit(Edit::TabName(tab_id), t2.clone(), window, cx)
                        })),
                )
            }
        };
        for tag in tab.all_tags() {
            head = head.child(
                div()
                    .flex_none()
                    .px_1()
                    .rounded_sm()
                    .text_size(px(11.0))
                    .bg(hsla_alpha(tag.color, 0.25))
                    .text_color(hsla(ui.ink(tag.color)))
                    .child(tag.label.clone()),
            );
        }
        if let Some(editor) = self.editor_for(&Edit::AddTag(tab_id)) {
            head = head.child(div().w(px(90.0)).child(editor));
        }
        head
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tab(
        &self,
        entry: &Entry,
        tab: &Tab,
        index: usize,
        position: usize,
        previews: &HashMap<PaneId, Vec<String>>,
        ui: &UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let running = entry.status != Status::Closed;
        let selected = position == self.selected;
        let active = tab.id == bench.active_tab;
        let title = tab.display_title();
        let pane = tab.active();
        let lines = previews.get(&tab.active_pane).cloned().unwrap_or_default();
        let mono: SharedString = self.state.read(cx).settings.font.family.clone().into();
        let tool = pane.and_then(|p| p.tool.as_ref()).map(|t| t.display());
        let tab_id = tab.id;
        let card_key = entry.key();

        let project = tab.project_info().map(|p| p.root.clone());
        let ai = tab.ai_info().map(|a| a.folder.clone());
        let head = self.tab_head(tab, accent, ui, cx);
        // A closed bench's screen is a memory: dimmer than live ones. (By
        // colour: `opacity` draws through a window-sized layer, per card.)
        let color = if running { hsla(ui.text_dim) } else { hsla_alpha(ui.text_dim, 0.55) };
        let preview = div().h(px(PREVIEW_LINES as f32 * 11.0)).overflow_hidden();
        // A project or a conversation has no screen: its icon stands in.
        let glyph = if ai.is_some() { "ai" } else { "project" };
        let preview = match project.as_ref().or(ai.as_ref()) {
            Some(_) => preview
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon(glyph)).size(px(36.0)).text_color(hsla_alpha(ui.ink(accent), if running { 0.5 } else { 0.3 }))),
            None => preview.child(preview_text(lines, mono, color)),
        };

        let mut foot = match (&project, &ai) {
            (Some(root), _) => {
                let branch = self.state.read(cx).projects.get(&tab.id).and_then(|v| v.read(cx).branch_label());
                vec![home_relative(root), branch.unwrap_or_else(|| "project".into())]
            }
            (None, Some(folder)) => vec![home_relative(folder), "Claude Code".into()],
            (None, None) => vec![pane.map(|p| home_relative(&p.cwd)).unwrap_or_default()],
        };
        if let Some(t) = &tool {
            foot.push(t.clone());
        }
        if tab.panes.len() > 1 {
            foot.push(format!("{} panes", tab.panes.len()));
        }

        let (e, e2) = (entry.clone(), entry.clone());
        let tab_for_menu = tab.clone();
        let border = if selected { hsla(accent) } else if active { hsla_alpha(accent, 0.45) } else { hsla(ui.divider) };
        div()
            .id(SharedString::from(format!("tab-{tab_id}")))
            .group("card")
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
            .on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
            // Dropped on a card, a tab takes its place.
            .drag_over::<DraggedTab>(move |s, _, _, _| s.border_l_4().border_color(hsla(accent)))
            .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| {
                cx.stop_propagation();
                this.move_tab(d.tab, &card_key, index, window, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.open(&e, Some(tab_id), cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_tab(tab_id, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                let entries = this.tab_menu(&e2, &tab_for_menu);
                this.open_menu(ev.position, entries, cx);
            }))
            .child(head)
            .child(preview)
            .child(div().truncate().text_size(px(11.0)).text_color(hsla(ui.text_dim)).child(foot.join(" · ")))
            .into_any_element()
    }
}

const PREVIEW_LINES: usize = 12;

/// A card's last lines of screen, bottom-aligned. Painted line by line rather
/// than as an element per line: every hover lays out the whole Shed window,
/// and a dozen text elements per card made that the bulk of a frame.
fn preview_text(lines: Vec<String>, family: SharedString, color: Hsla) -> impl IntoElement {
    let font = gpui::font(family);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let line_height = px(11.0);
            let shown = &lines[lines.len().saturating_sub(PREVIEW_LINES)..];
            let top = bounds.bottom() - line_height * shown.len() as f32;
            for (i, text) in shown.iter().enumerate().filter(|(_, t)| !t.is_empty()) {
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: None,
                };
                let line = window.text_system().shape_line(SharedString::from(text.clone()), px(9.0), &[run]);
                let origin = point(bounds.left(), top + line_height * i as f32);
                let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
            }
        },
    )
    .size_full()
}

impl MenuHost for Pegboard {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.menu
    }

    fn menu_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A choice may have opened a text field, which keeps the keyboard.
        if self.editing.is_none() {
            window.focus(&self.filter.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
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
        let started = std::time::Instant::now();
        let element = self.render_board(cx);
        if crate::terminal_view::perf::enabled() {
            use std::sync::atomic::Ordering;
            crate::terminal_view::perf::BOARD_RENDERS.fetch_add(1, Ordering::Relaxed);
            crate::terminal_view::perf::BOARD_US.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        }
        element
    }
}

impl Pegboard {
    fn render_board(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ui = self.state.read(cx).settings.ui.clone();
        let entries = self.entries(cx);
        let count = flat(&entries).len();
        if self.selected >= count {
            self.selected = count.saturating_sub(1);
        }
        let previews: HashMap<PaneId, Vec<String>> =
            cx.try_global::<PegboardData>().map(|d| d.0.previews.iter().cloned().collect()).unwrap_or_default();

        let compact = self.state.read(cx).settings.config.window.compact_pegboard;
        let mut list = div().flex().flex_col().gap_3();
        let mut first = 0;
        for entry in &entries {
            list = list.child(if compact { self.render_bench_line(entry, first, &ui, cx) } else { self.render_bench(entry, first, &previews, &ui, cx) });
            first += entry.tabs.len();
        }
        if entries.is_empty() {
            let empty = if self.query.is_empty() { "No benches yet. Make one with “New bench”." } else { "Nothing matches." };
            list = list.child(div().text_color(hsla(ui.text_dim)).child(empty));
        }

        let new_bench = self.button("new-bench".into(), "bench-new", "New bench", ui.accent, true, cx, |this, _, cx| {
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
                    .child("Filter · ↑↓ Enter to go · double-click to rename · right-click for more · drag tabs to move them"),
            )
            .child(new_bench);

        let menu = self.menu.as_ref().map(|m| menu::render(m, &ui, cx));
        let setup = self.setup.as_ref().map(|(p, _)| p.clone());
        div()
            .id("pegboard")
            .key_context("Pegboard")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .text_size(px(13.0))
            .on_key_down(cx.listener(Self::on_key))
            .child(search)
            .child(div().id("pegboard-list").flex_1().min_h_0().overflow_y_scroll().child(list))
            .when_some(setup, |el, panel| el.child(div().absolute().top_0().right_0().h_full().child(panel)))
            .children(menu)
            .into_any_element()
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

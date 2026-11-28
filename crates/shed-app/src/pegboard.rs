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
use shed_model::{BenchColor, BenchId, PaneId, Rgb, Tab, TabId};
use shed_client::BenchEdit;
use shed_client::pegboard::{BenchStatus as Status, ListedBench as Entry, flat};
use shed_proto::{BenchRef, ClientMsg, PegboardInfo, ShowTarget};
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
            .text_size(crate::theme::us(14.0))
            .text_color(hsla(ui.text))
            .child(self.name.clone())
    }
}

/// What the inline text field is editing.
#[derive(Clone, PartialEq)]
enum Edit {
    BenchName(BenchRef),
    Badge(BenchRef),
    TabName(TabId),
    AddTag(TabId),
    /// A colour of one's own: `#rrggbb` or a colour's name.
    Color(BenchRef),
    /// A new link for the bench: `name url`.
    AddLink(BenchRef),
    /// The bench's link at this index.
    ChangeLink(BenchRef, usize),
    /// The name of the bench's link at this index.
    RenameLink(BenchRef, usize),
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
    /// Opening a bench runs its startup (the config's `start_on_open`), as
    /// of the last render.
    start_on_open: bool,
    /// The list's scroll, and its scrollbar.
    scroll: gpui::ScrollHandle,
    scroll_bar: crate::scrollbar::State,
    /// A view per bench, so a hover or a change in one draws only that one;
    /// `shown` is the order they are listed in, as of the last `sync`.
    cards: HashMap<BenchId, Entity<BenchCard>>,
    shown: Vec<BenchId>,
    /// The colours, as of the last `sync`: the board draws from what it
    /// keeps, never from `AppState`, so that changes it does not show (a
    /// title it does not draw, a terminal's size) draw nothing.
    ui: UiColors,
    /// Only open benches are listed ("Open benches only"), as of the last `sync`.
    open_only: bool,
    _subscriptions: Vec<Subscription>,
}

/// What a bench's view shows. Made again whenever anything may have changed,
/// and drawn again only when it differs.
#[derive(Clone, PartialEq)]
struct CardModel {
    /// The bench as listed, without what the view does not show (see `shown`).
    entry: Entry,
    /// The selected tab, as an index into `entry.tabs`.
    selected: Option<usize>,
    /// The inline field, while one edits something of this bench.
    editing: Option<(Edit, Entity<LineEditor>)>,
    compact: bool,
    ui: UiColors,
    /// The state badge's colours: open, hidden.
    open_color: Rgb,
    hidden_color: Rgb,
    mono: SharedString,
    /// The previews of the shown tabs' active panes (none in the compact view).
    previews: HashMap<PaneId, Vec<String>>,
    /// The Git branch of project tabs, where their view knows it.
    branches: HashMap<TabId, String>,
}

/// One bench on the board: its header and tab cards (or its line, compact).
/// Its listeners act on the `Pegboard`.
pub struct BenchCard {
    board: gpui::WeakEntity<Pegboard>,
    model: CardModel,
    /// How many times it was drawn (tests check what a change draws).
    drawn: usize,
    /// The app's beat as of this drawing (working marks turn with it).
    frame: u64,
}

impl BenchCard {
    /// A listener that runs on the board, as `cx.listener` would there.
    fn on<E: ?Sized>(&self, f: impl Fn(&mut Pegboard, &E, &mut Window, &mut Context<Pegboard>) + 'static) -> impl Fn(&E, &mut Window, &mut gpui::App) + 'static {
        let board = self.board.clone();
        move |event, window, cx| {
            board.update(cx, |this, cx| f(this, event, window, cx)).ok();
        }
    }
}

impl Render for BenchCard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let started = std::time::Instant::now();
        self.drawn += 1;
        self.frame = crate::pulse::frame();
        let ui = self.model.ui.clone();
        let entry = &self.model.entry;
        let element = if self.model.compact { self.render_bench_line(entry, &ui) } else { self.render_bench(entry, &ui) };
        if crate::terminal_view::perf::enabled() {
            use std::sync::atomic::Ordering;
            crate::terminal_view::perf::BOARD_RENDERS.fetch_add(1, Ordering::Relaxed);
            crate::terminal_view::perf::BOARD_US.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        }
        element
    }
}

/// Whether `edit` is of something in `entry`'s bench.
fn edits(edit: &Edit, entry: &Entry) -> bool {
    match edit {
        Edit::TabName(tab) | Edit::AddTag(tab) => entry.bench.tabs.iter().any(|t| t.id == *tab),
        Edit::BenchName(key) | Edit::Badge(key) | Edit::Color(key) | Edit::AddLink(key) | Edit::ChangeLink(key, _) | Edit::RenameLink(key, _) => {
            *key == entry.bench_ref()
        }
    }
}

/// A bench as its view shows it: without the sizes, setup and aliases of its
/// tabs, nor the titles programs set where the tab shows another (its name,
/// its project's, or the active pane's), which change often and show nowhere
/// here. Menus read the bench as it is when they open.
fn shown(mut entry: Entry) -> Entry {
    let bench = &mut entry.bench;
    bench.aliases.clear();
    bench.window = None;
    for tab in &mut bench.tabs {
        tab.aliases.clear();
        tab.zoom = 0.0;
        let named = tab.name.is_some() || tab.project_info().is_some() || tab.ai_info().is_some();
        let active = tab.active_pane;
        for pane in &mut tab.panes {
            pane.cols = 0;
            pane.rows = 0;
            pane.setup = Default::default();
            pane.shell_aliases.clear();
            if named || pane.id != active {
                pane.title = None;
            }
        }
    }
    entry
}

impl Pegboard {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| LineEditor::new(String::new(), cx));
        window.focus(&filter.read(cx).focus_handle(cx), cx);
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| this.sync(cx)),
            cx.observe_global::<PegboardData>(|this, cx| this.sync(cx)),
            cx.observe(&filter, |this, filter, cx| {
                let query = filter.read(cx).text().to_string();
                if query != this.query {
                    this.query = query;
                    this.selected = 0;
                    this.sync(cx);
                }
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
                    // Project tabs' branches are read from their views.
                    this.sync(cx);
                    // The compact view shows no previews: only the list needs keeping.
                    let compact = this.state.read(cx).settings.config.window.compact_pegboard;
                    if window.is_window_active() && !compact { REFRESH } else { IDLE_REFRESH }
                });
                let Ok(next) = next else { break };
                cx.background_executor().timer(next).await;
            }
        })
        .detach();
        let ui = state.read(cx).settings.ui.clone();
        let mut board = Self {
            state,
            filter,
            selected: 0,
            query: String::new(),
            menu: None,
            editing: None,
            setup: None,
            scroll: gpui::ScrollHandle::new(),
            scroll_bar: Default::default(),
            start_on_open: false,
            cards: HashMap::new(),
            shown: Vec::new(),
            ui,
            open_only: false,
            _subscriptions: subscriptions,
        };
        board.sync(cx);
        board
    }

    /// Brings every bench's view in line with the tree, the keeper's last
    /// answer, the filter and the selection: a view whose bench shows
    /// differently is drawn again, others are kept as they are; the board
    /// itself only when benches come, go or move, or its colours change.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let entries = self.entries(cx);
        let count = flat(&entries).len();
        if self.selected >= count {
            self.selected = count.saturating_sub(1);
        }
        let models: Vec<CardModel> = {
            let state = self.state.read(cx);
            let settings = &state.settings;
            let compact = settings.config.window.compact_pegboard;
            self.start_on_open = settings.config.start_on_open;
            let previews: HashMap<PaneId, &Vec<String>> = match cx.try_global::<PegboardData>() {
                Some(data) if !compact => data.0.previews.iter().map(|(pane, lines)| (*pane, lines)).collect(),
                _ => HashMap::new(),
            };
            let mono: SharedString = settings.font.family.clone().into();
            let mut first = 0;
            entries
                .into_iter()
                .map(|entry| {
                    let selected = (self.selected >= first && self.selected < first + entry.tabs.len()).then(|| self.selected - first);
                    first += entry.tabs.len();
                    let editing = self.editing.as_ref().filter(|(edit, _, _)| edits(edit, &entry)).map(|(edit, editor, _)| (edit.clone(), editor.clone()));
                    let tabs = || entry.bench.tabs.iter().filter(|t| entry.tabs.contains(&t.id));
                    let previews = tabs().filter_map(|t| Some((t.active_pane, previews.get(&t.active_pane)?.to_vec()))).collect();
                    let branches = tabs()
                        .filter(|t| t.project_info().is_some())
                        .filter_map(|t| Some((t.id, state.projects.get(&t.id)?.read(cx).branch_label()?)))
                        .collect();
                    CardModel {
                        entry: shown(entry),
                        selected,
                        editing,
                        compact,
                        ui: settings.ui.clone(),
                        open_color: settings.palette.ansi[2],
                        hidden_color: settings.palette.ansi[3],
                        mono: mono.clone(),
                        previews,
                        branches,
                    }
                })
                .collect()
        };
        let ids: Vec<BenchId> = models.iter().map(|m| m.entry.bench.id).collect();
        for model in models {
            let id = model.entry.bench.id;
            // An AI tab at work: its mark turns with the app's beat, redrawing
            // this bench's view alone.
            let working = model.entry.bench.tabs.iter().filter(|t| model.entry.tabs.contains(&t.id)).filter_map(|t| t.ai_info()).any(|a| a.overall_activity() == shed_model::AiActivity::Working);
            let card = match self.cards.get(&id) {
                Some(card) => {
                    card.update(cx, |card, cx| {
                        if card.model != model {
                            card.model = model;
                            cx.notify();
                        }
                    });
                    card.clone()
                }
                None => {
                    let board = cx.weak_entity();
                    let card = cx.new(|_| BenchCard { board, model, drawn: 0, frame: 0 });
                    self.cards.insert(id, card.clone());
                    card
                }
            };
            card.update(cx, |_, cx| crate::pulse::animate(cx, working));
        }
        self.cards.retain(|id, _| ids.contains(id));
        let (ui, open_only) = {
            let settings = &self.state.read(cx).settings;
            (settings.ui.clone(), settings.config.pegboard.open_only)
        };
        if ids != self.shown || ui != self.ui || open_only != self.open_only {
            self.shown = ids;
            self.ui = ui;
            self.open_only = open_only;
            cx.notify();
        }
    }

    /// The first bench's view (`SHED_PERF_FRAMES` draws it again and again,
    /// as hovering in it does).
    pub fn first_card(&self) -> Option<Entity<BenchCard>> {
        self.shown.first().and_then(|id| self.cards.get(id)).cloned()
    }

    fn refresh(&self, cx: &Context<Self>) {
        self.send(ClientMsg::Pegboard, cx);
    }

    fn send(&self, msg: ClientMsg, cx: &Context<Self>) {
        self.state.read(cx).conn.send_msg(msg);
    }

    /// A tab and its bench, running or closed.
    fn find_tab(&self, tab: TabId, cx: &Context<Self>) -> Option<(BenchRef, Tab)> {
        let state = self.state.read(cx);
        for b in &state.tree.benches {
            if let Some(t) = b.tabs.iter().find(|t| t.id == tab) {
                return Some((BenchRef::Running(b.id), t.clone()));
            }
        }
        let stored = cx.try_global::<PegboardData>()?;
        stored.0.stored.iter().filter(|s| state.tree.bench(s.bench.id).is_none()).find_map(|s| {
            let t = s.bench.tabs.iter().find(|t| t.id == tab)?;
            Some((BenchRef::Saved(s.bench.name.clone()), t.clone()))
        })
    }

    /// Changes a tab (`edit` names it): through the keeper if its bench
    /// runs, in its saved file if not.
    fn change_tab(&mut self, tab: TabId, edit: BenchEdit, cx: &mut Context<Self>) {
        let Some((key, _)) = self.find_tab(tab, cx) else { return };
        self.send(edit.to_msg(&key), cx);
        self.refresh(cx);
    }

    /// Closes a running bench's tab, or removes a closed one's, after asking.
    fn close_tab(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, t)) = self.find_tab(tab, cx) else { return };
        let conn = self.state.read(cx).conn.clone();
        crate::state::close_tab(&conn, &t, key, window, cx);
    }

    /// Asks for a folder and adds it to a bench as a project tab.
    fn new_project_tab(&mut self, key: &BenchRef, cx: &mut Context<Self>) {
        let picked = crate::state::pick_folder(cx);
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let picked = picked.await;
            this.update(cx, |this, cx| match picked {
                Ok(Some(root)) => {
                    this.send(BenchEdit::NewProjectTab { root }.to_msg(&key), cx);
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

    fn new_tab(&mut self, key: &BenchRef, cx: &mut Context<Self>) {
        self.send(BenchEdit::NewTab.to_msg(key), cx);
        self.refresh(cx);
    }

    /// Puts a tab at `index` in a bench. Between running benches it moves with
    /// whatever runs in it; a closed bench's tabs move within it.
    fn move_tab(&mut self, tab: TabId, to: &BenchRef, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((from, t)) = self.find_tab(tab, cx) else { return };
        let to_name = match to {
            BenchRef::Running(id) => self.state.read(cx).tree.bench(*id).map(|b| b.name.clone()).unwrap_or_default(),
            BenchRef::Saved(name) => name.clone(),
        };
        let conn = self.state.read(cx).conn.clone();
        crate::state::move_tab(&conn, tab, &t.display_title(), from, to.clone(), &to_name, index, window, cx);
    }

    /// Every other bench, to move a tab to.
    fn move_menu(&mut self, tab: TabId, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some((from, _)) = self.find_tab(tab, cx) else { return Vec::new() };
        let state = self.state.read(cx);
        state
            .tab_destinations(&from, cx)
            .into_iter()
            .map(|d| {
                let key = d.to;
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
        let mut order = self.order(cx);
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

    /// Every bench in order, each with the tabs the filter matches; only
    /// open ones while "Open benches only" is on.
    fn entries(&self, cx: &Context<Self>) -> Vec<Entry> {
        let open_only = self.state.read(cx).settings.config.pegboard.open_only;
        let mut entries = self.list(&self.query, cx);
        if open_only {
            entries.retain(|e| e.status != Status::Closed);
        }
        entries
    }

    /// Every bench in order, each with the tabs `query` matches.
    fn list(&self, query: &str, cx: &Context<Self>) -> Vec<Entry> {
        let state = self.state.read(cx);
        let data = cx.try_global::<PegboardData>().map(|d| &d.0);
        let stored = data.map(|d| d.stored.as_slice()).unwrap_or_default();
        let order = data.map(|d| d.order.as_slice()).unwrap_or_default();
        shed_client::pegboard::list(&state.tree, |b| state.has_window(b), stored, order, query)
    }

    /// Every bench's id in order, shown or not (moving one keeps the others' places).
    fn order(&self, cx: &Context<Self>) -> Vec<BenchId> {
        self.list("", cx).iter().map(|e| e.bench.id).collect()
    }

    /// Lists only open benches, or every one.
    fn set_open_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.selected = 0;
        self.state.update(cx, |state, cx| state.set_config_value(Some("pegboard"), "open_only", toml_edit::Value::from(on), cx));
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

    /// A bench as listed now (whatever the filter), by id.
    fn listed(&self, bench: BenchId, cx: &Context<Self>) -> Option<Entry> {
        self.list("", cx).into_iter().find(|e| e.bench.id == bench)
    }

    /// Opens bench `bench` (see `open`).
    fn open_bench(&mut self, bench: BenchId, tab: Option<TabId>, cx: &mut Context<Self>) {
        if let Some(entry) = self.listed(bench, cx) {
            self.open(&entry, tab, cx);
        }
    }

    /// A bench's menu, from what it is now.
    fn bench_menu_of(&self, bench: BenchId, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        self.listed(bench, cx).map(|e| self.bench_menu(&e)).unwrap_or_default()
    }

    /// A tab's menu, from what it is now.
    fn tab_menu_of(&self, tab: TabId, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some((key, t)) = self.find_tab(tab, cx) else { return Vec::new() };
        let bench = match &key {
            BenchRef::Running(id) => Some(*id),
            BenchRef::Saved(name) => cx.try_global::<PegboardData>().and_then(|d| d.0.stored.iter().find(|s| s.bench.name == *name)).map(|s| s.bench.id),
        };
        bench.and_then(|b| self.listed(b, cx)).map(|e| self.tab_menu(&e, &t)).unwrap_or_default()
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

    fn close(&mut self, bench: BenchId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.state.read(cx).tree.bench(bench).map(|b| b.name.clone()) else { return };
        self.confirm(
            format!("Close the bench “{name}”?"),
            "It is saved, then the programs running in it stop and its teardown commands run. \
             Open it again from here to bring it back.",
            "Close Bench",
            ClientMsg::CloseBench { bench },
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

    fn set_color(&mut self, key: &BenchRef, color: BenchColor, cx: &mut Context<Self>) {
        self.send(BenchEdit::Color(color).to_msg(key), cx);
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
            this.sync(cx);
        });
        self.editing = Some((edit, editor, sub));
        self.sync(cx);
    }

    fn submit(&mut self, edit: Edit, text: String, cx: &mut Context<Self>) {
        let msg = match edit {
            Edit::BenchName(_) if text.is_empty() => return,
            Edit::BenchName(key) => BenchEdit::Rename(text).to_msg(&key),
            Edit::Badge(key) => {
                let badge: String = text.chars().filter(|c| !c.is_whitespace()).take(2).collect();
                BenchEdit::Badge((!badge.is_empty()).then_some(badge)).to_msg(&key)
            }
            // Empty goes back to the title the shell sets.
            Edit::TabName(tab) => return self.change_tab(tab, BenchEdit::RenameTab { tab, name: (!text.is_empty()).then_some(text) }, cx),
            Edit::AddTag(tab) => {
                let Some((_, t)) = self.find_tab(tab, cx) else { return };
                let Some(tags) = with_tag(&t.tags, &text, &self.state.read(cx).settings.config) else { return };
                return self.change_tab(tab, BenchEdit::TabTags { tab, tags }, cx);
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

    fn submit_link(&mut self, key: BenchRef, at: Option<usize>, text: &str, cx: &mut Context<Self>) {
        if let Some(link) = crate::links::parse(text) {
            let links = crate::links::with(&self.links_of(&key, cx), link, at);
            self.set_links(&key, links, cx);
        }
    }

    /// A bench's links, running or closed.
    fn links_of(&self, key: &BenchRef, cx: &Context<Self>) -> Vec<shed_model::ProjectLink> {
        match key {
            BenchRef::Running(id) => self.state.read(cx).tree.bench(*id).map(|b| b.links.clone()).unwrap_or_default(),
            BenchRef::Saved(name) => cx
                .try_global::<PegboardData>()
                .and_then(|d| d.0.stored.iter().find(|s| s.bench.name == *name).map(|s| s.bench.links.clone()))
                .unwrap_or_default(),
        }
    }

    fn set_links(&mut self, key: &BenchRef, links: Vec<shed_model::ProjectLink>, cx: &mut Context<Self>) {
        self.send(BenchEdit::Links(links).to_msg(key), cx);
        self.refresh(cx);
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
        let key = entry.bench_ref();
        let bench = &entry.bench;
        let mut entries = Vec::new();
        let e = entry.clone();
        let open_label = if entry.status == Status::Open { "Go to Bench" } else { "Open Bench" };
        let open_icon = if entry.status == Status::Closed { "bench-load" } else { "go" };
        entries.push(menu_item(open_label, move |this, _, cx| this.open(&e, None, cx)).icon(open_icon));
        let has_startup = bench.tabs.iter().flat_map(|t| &t.panes).any(|p| !p.setup.startup.is_empty());
        let has_teardown = bench.tabs.iter().flat_map(|t| &t.panes).any(|p| !p.setup.teardown.is_empty());
        if entry.status == Status::Closed {
            // Opening is quiet unless the config starts benches; the other
            // way is here.
            let name = bench.name.clone();
            if self.start_on_open {
                entries.push(menu_item("Open Quietly (Nothing Runs)", move |this, _, cx| {
                    this.send(ClientMsg::Show(ShowTarget::LoadQuiet { name: name.clone() }), cx);
                }));
            } else {
                entries.push(menu_item("Open and Run Startup", move |this, _, cx| {
                    this.send(ClientMsg::Show(ShowTarget::LoadAndStart { name: name.clone() }), cx);
                }).icon("go"));
            }
        } else if !bench.closing {
            let id = bench.id;
            if has_startup {
                entries.push(menu_item(if bench.started { "Run Startup Again" } else { "Run Startup" }, move |this, _, cx| this.send(ClientMsg::RunStartup { bench: id }, cx)).icon("go"));
            }
            if has_teardown || bench.started {
                entries.push(menu_item("Run Teardown", move |this, _, cx| this.send(ClientMsg::RunTeardown { bench: id }, cx)).icon("stop"));
            }
        }
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
            let id = bench.id;
            entries.push(danger("Close Bench…", move |this: &mut Self, window, cx| this.close(id, window, cx)).icon("power"));
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
        let order = self.order(cx);
        let Some(at) = order.iter().position(|b| *b == id) else { return };
        let Some(to) = at.checked_add_signed(step).and_then(|i| order.get(i)) else { return };
        self.move_bench(id, *to, cx);
    }

    fn color_entries(&self, key: &BenchRef, current: BenchColor) -> Vec<MenuEntry<Self>> {
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
        if tab.remote_info().is_some() && entry.status != Status::Closed {
            entries.push(menu_item("Reconnect", move |this, _, cx| this.send(ClientMsg::ReconnectRemote { tab: tab_id }, cx)).icon("refresh"));
        }
        let bench = entry.bench.id;
        let title = tab.display_title();
        entries.push(menu_item("Rename…", move |this, window, cx| this.start_edit(Edit::TabName(tab_id), title.clone(), window, cx)).icon("pencil"));
        if tab.project_info().is_none() && tab.ai_info().is_none() {
            entries.push(menu_item("Setup…", move |this, window, cx| this.open_setup(bench, tab_id, window, cx)).icon("rack"));
        }
        // The tags Shed knows, to pick from; or a new one.
        entries.push(
            more("Add Tag", move |this: &mut Self, cx| {
                let on: Vec<String> = this.find_tab(tab_id, cx).map(|(_, t)| t.tags.iter().map(|t| t.label.clone()).collect()).unwrap_or_default();
                let known = crate::bench_window::known_tags(cx, this.state.read(cx));
                let mut entries: Vec<MenuEntry<Self>> = known
                    .into_iter()
                    .filter(|(label, ..)| !on.contains(label))
                    .map(|(label, color, _)| {
                        menu_item(label.clone(), move |this, _, cx| {
                            let Some((_, t)) = this.find_tab(tab_id, cx) else { return };
                            let Some(tags) = with_tag(&t.tags, &label, &this.state.read(cx).settings.config) else { return };
                            this.change_tab(tab_id, BenchEdit::TabTags { tab: tab_id, tags }, cx);
                        })
                        .dot(color)
                    })
                    .collect();
                if !entries.is_empty() {
                    entries.push(MenuEntry::Separator);
                }
                entries.push(menu_item("New Tag…", move |this, window, cx| this.start_edit(Edit::AddTag(tab_id), String::new(), window, cx)).icon("plus"));
                entries
            })
            .icon("tag"),
        );
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
                this.change_tab(tab_id, BenchEdit::TabTags { tab: tab_id, tags: tags.collect() }, cx);
            }));
            entries.push(
                menu_item(format!("Remove Tag “{label}”"), move |this, _, cx| {
                    let Some((_, t)) = this.find_tab(tab_id, cx) else { return };
                    let tags = t.tags.into_iter().filter(|t| t.label != label).collect();
                    this.change_tab(tab_id, BenchEdit::TabTags { tab: tab_id, tags }, cx);
                })
                .icon("close"),
            );
        }
        let private = tab.private;
        entries.push(
            menu_item(if private { "Private  ✓" } else { "Private" }, move |this, _, cx| {
                this.change_tab(tab_id, BenchEdit::TabPrivate { tab: tab_id, private: !private }, cx);
            })
            .icon("private"),
        );
        if entry.status != Status::Closed {
            entries.push(menu_item("Duplicate Tab", move |this, _, cx| this.send(ClientMsg::DuplicateTab { tab: tab_id, after: true }, cx)).icon("copy"));
        }
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
        self.sync(cx);
    }

}

impl BenchCard {
    /// A bench's links as chips (click opens, right-click for more), and the
    /// field for one being added or changed.
    fn render_links(&self, entry: &Entry, ui: &UiColors) -> gpui::Div {
        let key = entry.bench_ref();
        let accent = entry.bench.color.accent();
        let links = entry.bench.links.clone();
        let id = entry.bench.id;
        let mut row = div().flex().flex_none().items_center().gap_1();
        for (i, link) in entry.bench.links.iter().enumerate() {
            if let Some(editor) = self.editor_for(&Edit::ChangeLink(key.clone(), i)).or_else(|| self.editor_for(&Edit::RenameLink(key.clone(), i))) {
                row = row.child(div().w(px(260.0)).text_size(crate::theme::us(12.0)).child(editor));
                continue;
            }
            let (url, links, k) = (link.url.clone(), links.clone(), key.clone());
            row = row.child(
                crate::links::chip(format!("link-{id}-{i}"), link, accent, ui)
                    .tooltip(menu::tooltip("Open the link (right-click for more)"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(self.on(move |_, _, _, cx| {
                        cx.stop_propagation();
                        crate::links::open(url.clone(), false, cx)
                    }))
                    .on_mouse_down(MouseButton::Right, self.on(move |this, e: &MouseDownEvent, _, cx| {
                        let (k1, k2, k3) = (k.clone(), k.clone(), k.clone());
                        let initial = links.get(i).map(crate::links::unparse).unwrap_or_default();
                        let name = links.get(i).map(|l| l.name.clone()).unwrap_or_default();
                        let entries = crate::links::menu(
                            links.clone(),
                            i,
                            move |this: &mut Pegboard, links, cx| this.set_links(&k1, links, cx),
                            move |this: &mut Pegboard, window, cx| this.start_edit(Edit::RenameLink(k3.clone(), i), name.clone(), window, cx),
                            move |this: &mut Pegboard, window, cx| this.start_edit(Edit::ChangeLink(k2.clone(), i), initial.clone(), window, cx),
                        );
                        this.open_menu(e.position, entries, cx);
                    })),
            );
        }
        if let Some(editor) = self.editor_for(&Edit::AddLink(key)) {
            row = row.child(div().w(px(260.0)).text_size(crate::theme::us(12.0)).child(editor)).child(div().text_size(crate::theme::us(11.0)).text_color(hsla(ui.text_dim)).child("name url"));
        }
        row
    }

    fn editor_for(&self, edit: &Edit) -> Option<Entity<LineEditor>> {
        self.model.editing.as_ref().filter(|(e, _)| e == edit).map(|(_, e)| e.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn button(
        &self,
        id: String,
        glyph: &'static str,
        label: &str,
        color: Rgb,
        filled: bool,
        tip: &'static str,
        on: impl Fn(&mut Pegboard, &mut Window, &mut Context<Pegboard>) + 'static,
    ) -> AnyElement {
        button(id, glyph, label, color, filled, &self.model.ui).tooltip(menu::tooltip(tip)).on_click(self.on(move |this, _, window, cx| on(this, window, cx))).into_any_element()
    }

    /// A pencil shown while the pointer is over `group`: names can be edited.
    fn rename_button(
        &self,
        id: String,
        group: SharedString,
        ui: &UiColors,
        on: impl Fn(&mut Pegboard, &mut Window, &mut Context<Pegboard>) + 'static,
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
            .child(svg().path(icon("pencil")).size(crate::theme::us(12.0)).text_color(hsla(ui.text_dim)))
            .on_click(self.on(move |this, _, window, cx| {
                cx.stop_propagation();
                on(this, window, cx)
            }))
            .into_any_element()
    }

    fn render_bench(&self, entry: &Entry, ui: &UiColors) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let id = bench.id;
        let key = entry.bench_ref();
        // State at a glance: a coloured badge, and what it means beside it.
        let (state_color, state_label, state_note) = match entry.status {
            Status::Open => (self.model.open_color, "Open", String::new()),
            Status::Hidden => (self.model.hidden_color, "Hidden", "running without a window".to_string()),
            Status::Closed => (ui.text_dim, "Closed", format!("saved {}", entry.saved.as_deref().map(short_time).unwrap_or_default())),
        };
        let state_badge = div()
            .flex()
            .items_center()
            .gap_1()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .text_size(crate::theme::us(11.0))
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
            .tooltip(menu::tooltip("The bench's colour: click to choose"))
            .on_mouse_down(MouseButton::Left, self.on(move |this, e: &MouseDownEvent, _, cx| {
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
                            .text_size(crate::theme::us(15.0))
                            .text_color(hsla(ui.text))
                            .cursor_text()
                            .child(bench.name.clone())
                            .on_click(self.on(move |this, e: &ClickEvent, window, cx| {
                                if e.click_count() >= 2 {
                                    this.start_edit(Edit::BenchName(k.clone()), n.clone(), window, cx);
                                }
                            })),
                    )
                    .child(self.rename_button(format!("rename-bench-{id}"), group, ui, move |this, window, cx| {
                        this.start_edit(Edit::BenchName(k2.clone()), n2.clone(), window, cx)
                    }))
                    .into_any_element()
            }
        };
        let badge = match self.editor_for(&Edit::Badge(key.clone())).or_else(|| self.editor_for(&Edit::Color(key.clone()))) {
            Some(editor) => Some(div().w(px(110.0)).child(editor).into_any_element()),
            None => bench.badge.clone().map(|badge| {
                div().px_1().rounded_sm().text_size(crate::theme::us(12.0)).bg(hsla_alpha(accent, 0.25)).text_color(hsla(ui.ink(accent))).child(badge).into_any_element()
            }),
        };
        // Dragging the grip moves the bench among the others.
        let grip = div()
            .id(SharedString::from(format!("grip-{id}")))
            .flex_none()
            .cursor_grab()
            .tooltip(menu::tooltip("Drag to move the bench"))
            .child(svg().path(icon("grip")).size(crate::theme::us(14.0)).text_color(hsla(ui.text_dim)))
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
            .child(div().text_size(crate::theme::us(12.0)).text_color(hsla(ui.text_dim)).child(format!(
                "{}{tab_count} tab{}",
                if state_note.is_empty() { String::new() } else { format!("{state_note} · ") },
                if tab_count == 1 { "" } else { "s" }
            )))
            .child(self.render_links(entry, ui))
            .child(div().flex_1());

        if bench.closing {
            header = header.child(div().text_size(crate::theme::us(12.0)).text_color(hsla(ui.ink(accent))).child("closing…"));
        } else {
            let (open_icon, open_label) = if entry.status == Status::Open { ("go", "Go to") } else { ("bench-load", "Open") };
            let open_tip = match entry.status {
                Status::Open => "Bring the bench's window to the front",
                Status::Hidden => "Show the bench's window again (it kept running)",
                Status::Closed => "Open the bench: its window, tabs and startup",
            };
            header = header.child(self.button(format!("open-{id}"), open_icon, open_label, accent, true, open_tip, move |this, _, cx| this.open_bench(id, None, cx)));
            if entry.status == Status::Open {
                header = header.child(self.button(format!("hide-{id}"), "detach", "Hide", ui.text_dim, false, "Hide the window: the bench keeps running, and comes back from here", move |this, _, cx| this.hide(id, cx)));
            }
            if entry.status != Status::Closed {
                header = header.child(self.button(format!("close-{id}"), "power", "Close…", ui.text_dim, false, "Close the bench: its programs stop and it is saved, to open again later (asks first)", move |this, window, cx| this.close(id, window, cx)));
            }
            let name = bench.name.clone();
            header = header.child(self.button(format!("delete-{id}"), "trash", "Delete…", ui.danger, false, "Delete the bench: it stops, and its saved file, scrollback and launcher go (asks first)", move |this, window, cx| {
                this.delete(name.clone(), window, cx)
            }));
                header = header.child(
                div()
                    .id(SharedString::from(format!("more-{id}")))
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip("More: rename, badge, colour, new tab"))
                    .child(svg().path(icon("more")).size(crate::theme::us(15.0)).text_color(hsla(ui.text_dim)))
                    .on_mouse_down(MouseButton::Left, self.on(move |this, ev: &MouseDownEvent, _, cx| {
                        let entries = this.bench_menu_of(id, cx);
                        this.open_menu(ev.position, entries, cx);
                    })),
            );
        }

        let mut cards = div().flex().flex_wrap().gap_3();
        for (i, tab_id) in entry.tabs.iter().enumerate() {
            let Some(tab) = bench.tabs.iter().find(|t| t.id == *tab_id) else { continue };
            let index = bench.tabs.iter().position(|t| t.id == *tab_id).unwrap_or(i);
            cards = cards.child(self.render_tab(entry, tab, index, i, ui));
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
                    .text_size(crate::theme::us(12.0))
                    .text_color(hsla(ui.text_dim))
                    .hover(move |s| s.border_color(hsla(accent)).text_color(hsla(ink)))
                    .child(svg().path(icon("tab-new")).size(crate::theme::us(14.0)).text_color(hsla(ui.text_dim)))
                    .child("New tab")
                    .on_click(self.on(move |this, _, _, cx| this.new_tab(&k, cx))),
            );
        }

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
            .on_mouse_down(MouseButton::Right, self.on(move |this, ev: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu_of(id, cx);
                this.open_menu(ev.position, entries, cx);
            }))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.border_color(hsla(accent)))
            .on_drop(self.on(move |this, d: &DraggedTab, window, cx| this.move_tab(d.tab, &drop_key, usize::MAX, window, cx)))
            .drag_over::<DraggedBench>(move |s, _, _, _| s.border_2().border_color(hsla(accent)))
            .on_drop(self.on(move |this, d: &DraggedBench, _, cx| this.move_bench(d.id, id, cx)))
            .child(header)
            .child(cards)
            .into_any_element()
    }

    /// The compact view of a bench: a header line (its colour, name, state,
    /// links, Open) and its tabs below in even columns, each with an icon.
    /// No borders: space and one colour (the bench's dot) carry the layout.
    fn render_bench_line(&self, entry: &Entry, ui: &UiColors) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let id = bench.id;
        let key = entry.bench_ref();
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
            .on_mouse_down(MouseButton::Left, self.on(move |this, e: &MouseDownEvent, _, cx| {
                let entries = this.color_entries(&k, current);
                this.open_menu(e.position, entries, cx);
            }));
        let name = match self.editor_for(&Edit::BenchName(key.clone())) {
            Some(editor) => div().w(px(240.0)).child(editor).into_any_element(),
            None => {
                let (k, n) = (key.clone(), bench.name.clone());
                div()
                    .id(SharedString::from(format!("name-{id}")))
                    .flex_none()
                    .text_size(crate::theme::us(15.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(hsla(if closed { ui.text_dim } else { ui.text }))
                    .cursor_pointer()
                    .tooltip(menu::tooltip("Click to open the bench, double-click to rename it"))
                    .child(bench.name.clone())
                    .on_click(self.on(move |this, ev: &ClickEvent, window, cx| {
                        if ev.click_count() >= 2 {
                            this.start_edit(Edit::BenchName(k.clone()), n.clone(), window, cx);
                        } else {
                            this.open_bench(id, None, cx);
                        }
                    }))
                    .into_any_element()
            }
        };
        let badge = match self.editor_for(&Edit::Badge(key.clone())).or_else(|| self.editor_for(&Edit::Color(key.clone()))) {
            Some(editor) => Some(div().w(px(110.0)).child(editor).into_any_element()),
            None => bench.badge.clone().map(|badge| {
                div().flex_none().px_1().rounded_sm().text_size(crate::theme::us(11.0)).bg(hsla_alpha(accent, 0.2)).text_color(hsla(ui.ink(accent))).child(badge).into_any_element()
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
            .child(svg().path(icon("grip")).size(crate::theme::us(12.0)).text_color(hsla(ui.text_dim)))
            .on_drag(DraggedBench { id, name: bench.name.clone() }, |d, _, _, cx| cx.new(|_| d.clone()));

        let mut header = div()
            .h(crate::theme::us(28.0))
            .flex()
            .items_center()
            .gap_2()
            .child(grip)
            .child(dot)
            .child(name)
            .children(badge)
            .when(!state.is_empty(), |d| d.child(div().flex_none().text_size(crate::theme::us(12.0)).text_color(hsla(ui.text_dim)).child(state)))
            .child(div().flex_1())
            .child(self.render_quiet_links(entry, ui));
        if bench.closing {
            header = header.child(div().text_size(crate::theme::us(12.0)).text_color(hsla(ui.text_dim)).child("closing…"));
        } else {
            let (open_icon, open_label) = if entry.status == Status::Open { ("go", "Go to") } else { ("bench-load", "Open") };
            header = header.child(self.quiet_button(format!("open-{id}"), open_icon, Some(open_label), ui, move |this, _, cx| this.open_bench(id, None, cx)));
            header = header.child(
                self.quiet_button(format!("more-{id}"), "more", None, ui, |_, _, _| {})
                    .tooltip(menu::tooltip("More: hide, close, delete, rename, colour, links"))
                    .on_mouse_down(MouseButton::Left, self.on(move |this, ev: &MouseDownEvent, _, cx| {
                        let entries = this.bench_menu_of(id, cx);
                        this.open_menu(ev.position, entries, cx);
                    })),
            );
        }

        // The tabs, in columns under the name.
        let mut tabs = div().pl(px(30.0)).flex().flex_wrap().gap_x_1p5().gap_y_0p5();
        for (i, tab_id) in entry.tabs.iter().enumerate() {
            let Some(tab) = bench.tabs.iter().find(|t| t.id == *tab_id) else { continue };
            let index = bench.tabs.iter().position(|t| t.id == *tab_id).unwrap_or(i);
            tabs = tabs.child(self.render_chip(entry, tab, index, i, ui));
        }
        if !bench.closing {
            let k = key.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("new-tab-{id}")))
                    .h(crate::theme::us(26.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(crate::theme::us(12.0))
                    .text_color(hsla(ui.text_dim))
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .child(svg().path(icon("tab-new")).size(crate::theme::us(13.0)).text_color(hsla(ui.text_dim)))
                    .child("New tab")
                    .on_click(self.on(move |this, _, _, cx| this.new_tab(&k, cx))),
            );
        }

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
            .on_mouse_down(MouseButton::Right, self.on(move |this, ev: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu_of(id, cx);
                this.open_menu(ev.position, entries, cx);
            }))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(hsla_alpha(accent, 0.08)))
            .on_drop(self.on(move |this, d: &DraggedTab, window, cx| this.move_tab(d.tab, &drop_key, usize::MAX, window, cx)))
            .drag_over::<DraggedBench>(move |s, _, _, _| s.bg(hsla_alpha(accent, 0.08)))
            .on_drop(self.on(move |this, d: &DraggedBench, _, cx| this.move_bench(d.id, id, cx)))
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
        on: impl Fn(&mut Pegboard, &mut Window, &mut Context<Pegboard>) + 'static,
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
            .text_size(crate::theme::us(12.0))
            .cursor_pointer()
            .text_color(hsla(ui.text_dim))
            .hover(move |s| s.bg(hsla(ui.tab_hover)).text_color(hsla(text)))
            .child(svg().path(icon(glyph)).size(crate::theme::us(13.0)).text_color(hsla(ui.text_dim)))
            .children(label.map(str::to_string))
            .on_click(self.on(move |this, _, window, cx| on(this, window, cx)))
    }

    /// A bench's links on its header line: an icon and a name each, in the
    /// text's colours; click opens, right-click for more.
    fn render_quiet_links(&self, entry: &Entry, ui: &UiColors) -> gpui::Div {
        let key = entry.bench_ref();
        let links = entry.bench.links.clone();
        let id = entry.bench.id;
        let text = ui.text;
        let mut row = div().flex().flex_none().items_center().gap_1();
        for (i, link) in entry.bench.links.iter().enumerate() {
            if let Some(editor) = self.editor_for(&Edit::ChangeLink(key.clone(), i)).or_else(|| self.editor_for(&Edit::RenameLink(key.clone(), i))) {
                row = row.child(div().w(px(260.0)).text_size(crate::theme::us(12.0)).child(editor));
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
                    .text_size(crate::theme::us(12.0))
                    .text_color(hsla(ui.text))
                    .hover(move |s| s.bg(hsla(ui.tab_hover)).text_color(hsla(text)))
                    .tooltip(menu::tooltip_text(link.url.clone()))
                    .child(svg().path(icon("link")).size(crate::theme::us(12.0)).flex_none().text_color(hsla(ui.text_dim)))
                    .child(div().min_w_0().truncate().child(link.name.clone()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(self.on(move |_, _, _, cx| {
                        cx.stop_propagation();
                        crate::links::open(url.clone(), false, cx)
                    }))
                    .on_mouse_down(MouseButton::Right, self.on(move |this, e: &MouseDownEvent, _, cx| {
                        let (k1, k2, k3) = (k.clone(), k.clone(), k.clone());
                        let initial = links.get(i).map(crate::links::unparse).unwrap_or_default();
                        let name = links.get(i).map(|l| l.name.clone()).unwrap_or_default();
                        let entries = crate::links::menu(
                            links.clone(),
                            i,
                            move |this: &mut Pegboard, links, cx| this.set_links(&k1, links, cx),
                            move |this: &mut Pegboard, window, cx| this.start_edit(Edit::RenameLink(k3.clone(), i), name.clone(), window, cx),
                            move |this: &mut Pegboard, window, cx| this.start_edit(Edit::ChangeLink(k2.clone(), i), initial.clone(), window, cx),
                        );
                        this.open_menu(e.position, entries, cx);
                    })),
            );
        }
        if let Some(editor) = self.editor_for(&Edit::AddLink(key)) {
            row = row.child(div().w(px(260.0)).text_size(crate::theme::us(12.0)).child(editor));
        }
        row
    }

    /// A tab in the compact view: an icon for what it is (or what it is
    /// doing), its title, tags as small dots, and what runs in it.
    #[allow(clippy::too_many_arguments)]
    fn render_chip(&self, entry: &Entry, tab: &Tab, index: usize, position: usize, ui: &UiColors) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let running = entry.status != Status::Closed;
        let selected = self.model.selected == Some(position);
        let active = tab.id == bench.active_tab && running;
        let tab_id = tab.id;
        let title = tab.display_title();
        let card_key = entry.bench_ref();
        let pane = tab.active();
        let tool = pane.and_then(|p| p.tool.as_ref());
        let tab_icon = crate::bench_window::tab_icon(tab, accent, ui);
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
                    .on_click(self.on(move |this, e: &ClickEvent, window, cx| {
                        if e.click_count() >= 2 {
                            cx.stop_propagation();
                            this.start_edit(Edit::TabName(tab_id), t.clone(), window, cx);
                        }
                    }))
                    .into_any_element()
            }
        };
        let tags = div().flex().flex_none().gap_0p5().children(tab.all_tags().map(|tag| div().size(px(6.0)).rounded_full().bg(hsla(tag.color))));
        let bench_id = entry.bench.id;
        div()
            .id(SharedString::from(format!("tab-{tab_id}")))
            .relative()
            .h(crate::theme::us(26.0))
            .w(crate::theme::us(CHIP_WIDTH))
            .flex_none()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .rounded_md()
            .text_size(crate::theme::us(13.0))
            .cursor_pointer()
            // A light ground of their own, so tabs stand out from the bench.
            .when(selected, |d| d.bg(hsla_alpha(accent, 0.18)))
            .when(!selected, |d| d.bg(hsla_alpha(ui.text, 0.06)).hover(|s| s.bg(hsla_alpha(ui.text, 0.11))))
            .tooltip(menu::tooltip_text(full))
            .on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(hsla_alpha(accent, 0.14)))
            .on_drop(self.on(move |this, d: &DraggedTab, window, cx| {
                cx.stop_propagation();
                this.move_tab(d.tab, &card_key, index, window, cx);
            }))
            .on_click(self.on(move |this, _, _, cx| this.open_bench(bench_id, Some(tab_id), cx)))
            .on_mouse_down(MouseButton::Middle, self.on(move |this, _, window, cx| this.close_tab(tab_id, window, cx)))
            .on_mouse_down(MouseButton::Right, self.on(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let entries = this.tab_menu_of(tab_id, cx);
                this.open_menu(ev.position, entries, cx);
            }))
            // The tab the bench shows: a short bar in its colour.
            .when(active, |d| d.child(div().absolute().left_0().top(px(6.0)).h(px(14.0)).w(px(2.0)).rounded_full().bg(hsla(accent))))
            .child(match tab_icon.working {
                true => crate::pulse::working_mark(self.frame, crate::theme::us(14.0), tab_icon.color).into_any_element(),
                false => svg().path(icon(tab_icon.glyph)).size(crate::theme::us(14.0)).flex_none().text_color(hsla(tab_icon.color)).into_any_element(),
            })
            .child(title_el)
            .when(tab_icon.background > 0, |d| {
                d.child(crate::bench_window::background_badge(SharedString::from(format!("background-{tab_id}")), tab_icon.background, accent, ui, crate::theme::us(10.5)))
            })
            .child(tags)
            .when(tab.private, |d| d.child(svg().path(icon("private")).size(crate::theme::us(11.0)).flex_none().text_color(hsla(ui.text_dim))))
            .child(div().flex_1())
            .children(detail.map(|d| div().flex_shrink_0().max_w(crate::theme::us(CHIP_WIDTH * 0.4)).truncate().text_size(crate::theme::us(11.5)).text_color(hsla(ui.text_dim)).child(d)))
            .into_any_element()
    }

    /// A tab's icons, title (double-click or the pencil renames it) and tags.
    fn tab_head(&self, tab: &Tab, accent: Rgb, ui: &UiColors) -> gpui::Div {
        let tool = tab.active().and_then(|p| p.tool.as_ref());
        let waiting = tab.panes.iter().any(|p| p.waiting.is_some() || p.held.is_some());
        let tab_id = tab.id;
        let title = tab.display_title();
        let mut head = div().flex().items_center().gap_1p5().min_w_0();
        if tab.project_info().is_some() {
            head = head.child(svg().path(icon("project")).size(crate::theme::us(13.0)).flex_none().text_color(hsla(ui.ink(accent))));
        }
        if let Some(ai) = tab.ai_info() {
            head = head.child(svg().path(icon("ai")).size(crate::theme::us(13.0)).flex_none().text_color(hsla(ui.ink(accent))));
            // At work (turning), waiting for you, or on work it started in
            // the background.
            match ai.overall_activity() {
                shed_model::AiActivity::Working => head = head.child(crate::pulse::working_mark(self.frame, crate::theme::us(12.0), ui.ink(accent))),
                shed_model::AiActivity::Waiting => {
                    head = head.child(svg().path(icon("tool-waiting")).size(crate::theme::us(12.0)).flex_none().text_color(hsla(ui.ink(accent))));
                }
                shed_model::AiActivity::Background(n) => {
                    head = head
                        .child(svg().path(icon("hourglass")).size(crate::theme::us(12.0)).flex_none().text_color(hsla(ui.ink(accent))))
                        .child(crate::bench_window::background_badge(SharedString::from(format!("background-{tab_id}")), n, accent, ui, crate::theme::us(10.5)));
                }
                _ => {}
            }
        }
        if tab.remote_info().is_some() {
            head = head.child(svg().path(icon("server")).size(crate::theme::us(13.0)).flex_none().text_color(hsla(ui.ink(accent))));
        }
        if waiting {
            head = head.child(svg().path(icon("tool-waiting")).size(crate::theme::us(12.0)).flex_none().text_color(hsla(accent)));
        } else if tool.is_some() {
            head = head.child(svg().path(icon("tool")).size(crate::theme::us(12.0)).flex_none().text_color(hsla(ui.text_dim)));
        }
        if tab.private {
            head = head.child(svg().path(icon("private")).size(crate::theme::us(12.0)).flex_none().text_color(hsla(ui.text_dim)));
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
                                .on_click(self.on(move |this, e: &ClickEvent, window, cx| {
                                    if e.click_count() >= 2 {
                                        cx.stop_propagation();
                                        this.start_edit(Edit::TabName(tab_id), t.clone(), window, cx);
                                    }
                                })),
                        )
                        .child(self.rename_button(format!("rename-tab-{tab_id}"), group, ui, move |this, window, cx| {
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
                    .text_size(crate::theme::us(11.0))
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
        ui: &UiColors,
    ) -> AnyElement {
        let bench = &entry.bench;
        let accent = bench.color.accent();
        let running = entry.status != Status::Closed;
        let selected = self.model.selected == Some(position);
        let active = tab.id == bench.active_tab;
        let title = tab.display_title();
        let pane = tab.active();
        let lines = self.model.previews.get(&tab.active_pane).cloned().unwrap_or_default();
        let mono = self.model.mono.clone();
        let tool = pane.and_then(|p| p.tool.as_ref()).map(|t| t.display());
        let tab_id = tab.id;
        let card_key = entry.bench_ref();

        let project = tab.project_info().map(|p| p.root.clone());
        let ai = tab.ai_info().map(|a| a.folder.clone());
        let head = self.tab_head(tab, accent, ui);
        // A closed bench's screen is a memory: dimmer than live ones. (By
        // colour: `opacity` draws through a window-sized layer, per card.)
        let color = if running { hsla(ui.text_dim) } else { hsla_alpha(ui.text_dim, 0.55) };
        let preview = div().h(crate::theme::us(PREVIEW_LINES as f32 * 11.0)).overflow_hidden();
        // A project or a conversation has no screen: its icon stands in.
        let glyph = if ai.is_some() { "ai" } else { "project" };
        let preview = match project.as_ref().or(ai.as_ref()) {
            Some(_) => preview
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon(glyph)).size(crate::theme::us(36.0)).text_color(hsla_alpha(ui.ink(accent), if running { 0.5 } else { 0.3 }))),
            None => preview.child(preview_text(lines, mono, color)),
        };

        let mut foot = match (&project, &ai) {
            (Some(root), _) => {
                let branch = self.model.branches.get(&tab.id).cloned();
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

        let bench_id = entry.bench.id;
        let border = if selected { hsla(accent) } else if active { hsla_alpha(accent, 0.45) } else { hsla(ui.divider) };
        div()
            .id(SharedString::from(format!("tab-{tab_id}")))
            .group("card")
            .w(crate::theme::us(CARD_WIDTH))
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
            .on_drop(self.on(move |this, d: &DraggedTab, window, cx| {
                cx.stop_propagation();
                this.move_tab(d.tab, &card_key, index, window, cx);
            }))
            .on_click(self.on(move |this, _, _, cx| this.open_bench(bench_id, Some(tab_id), cx)))
            .on_mouse_down(MouseButton::Middle, self.on(move |this, _, window, cx| this.close_tab(tab_id, window, cx)))
            .on_mouse_down(MouseButton::Right, self.on(move |this, ev: &MouseDownEvent, _, cx| {
                let entries = this.tab_menu_of(tab_id, cx);
                this.open_menu(ev.position, entries, cx);
            }))
            .child(head)
            .child(preview)
            .child(div().truncate().text_size(crate::theme::us(11.0)).text_color(hsla(ui.text_dim)).child(foot.join(" · ")))
            .into_any_element()
    }
}

/// A small button with an icon and a label, in `color`.
fn button(id: String, glyph: &'static str, label: &str, color: Rgb, filled: bool, ui: &UiColors) -> gpui::Stateful<gpui::Div> {
    let ink = ui.ink(color);
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .text_size(crate::theme::us(12.0))
        .cursor_pointer()
        .text_color(hsla(ink))
        .when(filled, |d| d.bg(hsla_alpha(color, 0.2)))
        .hover(move |s| s.bg(hsla_alpha(color, 0.3)))
        .child(svg().path(icon(glyph)).size(crate::theme::us(13.0)).text_color(hsla(ink)))
        .child(label.to_string())
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
            let line_height = crate::theme::us(11.0);
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
                let line = window.text_system().shape_line(SharedString::from(text.clone()), crate::theme::us(9.0), &[run]);
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

/// `2026-11-01T12:32:05Z` as `2026-11-01 12:32 UTC`.
fn short_time(saved: &str) -> String {
    match saved.get(..16) {
        Some(t) => format!("{} UTC", t.replace('T', " ")),
        None => saved.to_string(),
    }
}

impl Render for Pegboard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_board(cx)
    }
}

impl Pegboard {
    /// "Open benches only": a mode toggle beside the filter.
    fn open_only_toggle(&self, ui: &UiColors, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let on = self.open_only;
        let tint = if on { ui.accent } else { ui.text_dim };
        div()
            .id("open-only")
            .flex_none()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(hsla(if on { ui.accent } else { ui.divider }))
            .cursor_pointer()
            .when(on, |d| d.bg(hsla_alpha(ui.accent, 0.2)))
            .when(!on, |d| d.hover(|s| s.bg(hsla(ui.tab_hover))))
            .tooltip(menu::tooltip(if on { "Open benches only (on): closed benches are not listed" } else { "Open benches only: leave closed benches out of the list" }))
            .child(svg().path(icon("open-only")).size(crate::theme::us(15.0)).text_color(hsla(tint)))
            .on_click(cx.listener(move |this, _, _, cx| this.set_open_only(!on, cx)))
    }

    fn render_board(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // From what `sync` kept: reading `AppState` here would draw the
        // window again at every change to it.
        let ui = self.ui.clone();
        let mut list = div().flex().flex_col().gap_3().children(self.shown.iter().filter_map(|id| self.cards.get(id).cloned()));
        if self.shown.is_empty() {
            let empty = match (self.query.is_empty(), self.open_only) {
                (false, _) => "Nothing matches.",
                (true, true) => "No open benches. Closed ones are listed with “Open benches only” off.",
                (true, false) => "No benches yet. Make one with “New bench”.",
            };
            list = list.child(div().text_color(hsla(ui.text_dim)).child(empty));
        }

        let new_bench = button("new-bench".into(), "bench-new", "New bench", ui.accent, true, &ui).on_click(cx.listener(|this, _, _, cx| {
            this.send(ClientMsg::Show(ShowTarget::NewBench { name: None, cwd: None }), cx);
        }));
        let search = div()
            .flex()
            .items_center()
            .gap_2()
            .child(svg().path(icon("pegboard")).size(crate::theme::us(18.0)).text_color(hsla(ui.text_dim)))
            .child(div().flex_1().max_w(px(480.0)).text_size(crate::theme::us(14.0)).child(self.filter.clone()))
            .child(self.open_only_toggle(&ui, cx))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_size(crate::theme::us(12.0))
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
            .text_size(crate::theme::us(13.0))
            .on_key_down(cx.listener(Self::on_key))
            .child(search)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(div().id("pegboard-list").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(list))
                    .child(crate::scrollbar::vertical(crate::scrollbar::Source::Handle(self.scroll.clone()), &self.scroll_bar, &ui, false).absolute().top_0().right_0().bottom_0()),
            )
            .when_some(setup, |el, panel| el.child(div().absolute().top_0().right_0().h_full().child(panel)))
            .children(menu)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench(name: &str, titles: &[(&str, Option<&str>)]) -> shed_model::Bench {
        let tabs: Vec<Tab> = titles
            .iter()
            .map(|(title, name)| {
                let mut pane = shed_model::Pane::new(PaneId::new(), "/tmp".into());
                pane.title = Some(title.to_string());
                let mut tab = Tab::with_pane(pane);
                tab.name = name.map(str::to_string);
                tab
            })
            .collect();
        shed_model::Bench {
            id: BenchId::new(),
            name: name.into(),
            color: BenchColor::Blue,
            badge: None,
            cwd: "/".into(),
            active_tab: tabs[0].id,
            tabs,
            aliases: Default::default(),
            closing: false,
            started: false,
            launcher: name.into(),
            window: None,
            links: Vec::new(),
            bookmarks: Vec::new(),
        }
    }

    /// A change to one bench draws that bench's view only, and a change the
    /// board does not show (a title under a tab's own name, a terminal's
    /// size) draws nothing.
    #[gpui::test]
    fn a_change_draws_only_its_bench(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let state = cx.new(|_| AppState::new(shed_client::keeper_conn::KeeperConn::detached(), crate::settings::Settings::default()));
        let (api, web) = (bench("api", &[("vim", Some("editor")), ("~/api", None)]), bench("web", &[("~/web", None)]));
        let (api_id, web_id) = (api.id, web.id);
        state.update(cx, |s, cx| {
            s.tree = shed_model::Shed { benches: vec![api, web], ..Default::default() };
            cx.notify();
        });
        let st = state.clone();
        let (board, cx) = cx.add_window_view(|window, cx| Pegboard::new(st, window, cx));
        cx.run_until_parked();
        let drawn = |cx: &mut gpui::VisualTestContext| {
            board.update(cx, |b, cx| [api_id, web_id].map(|id| b.cards.get(&id).map(|c| c.read(cx).drawn).unwrap_or_default()))
        };
        let change = |cx: &mut gpui::VisualTestContext, f: &dyn Fn(&mut shed_model::Shed)| {
            state.update(cx, |s, cx| {
                f(&mut s.tree);
                cx.notify();
            });
            cx.run_until_parked();
        };
        let [a, w] = drawn(cx);
        assert!(a > 0 && w > 0, "both drawn at first");

        // The title under the name "editor", and a terminal's size: not shown.
        change(cx, &|tree| {
            let tab = &mut tree.benches[0].tabs[0];
            tab.panes[0].title = Some("vim src/main.rs".into());
            tab.panes[0].cols = 132;
        });
        assert_eq!(drawn(cx), [a, w], "nothing shown changed");

        // A title the board shows: that bench only.
        change(cx, &|tree| tree.benches[0].tabs[1].panes[0].title = Some("~/api/src".into()));
        assert_eq!(drawn(cx), [a + 1, w]);
        change(cx, &|tree| tree.benches[1].name = "website".into());
        assert_eq!(drawn(cx), [a + 1, w + 1]);
    }

    /// "Open benches only" leaves closed benches out, filter and keys
    /// working on what is left.
    #[gpui::test]
    fn open_benches_only(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let state = cx.new(|_| AppState::new(shed_client::keeper_conn::KeeperConn::detached(), crate::settings::Settings::default()));
        let (api, old) = (bench("api", &[("~/api", None), ("~/api/db", None)]), bench("old", &[("~/old", None)]));
        let (api_id, old_id) = (api.id, old.id);
        state.update(cx, |s, cx| {
            s.tree = shed_model::Shed { benches: vec![api], ..Default::default() };
            cx.notify();
        });
        cx.update(|cx| {
            let stored = vec![shed_proto::StoredBench { bench: old, saved: "2026-11-01T12:00:00Z".into() }];
            cx.set_global(PegboardData(PegboardInfo { stored, previews: Vec::new(), order: Vec::new() }))
        });
        let st = state.clone();
        let (board, cx) = cx.add_window_view(|window, cx| Pegboard::new(st, window, cx));
        cx.run_until_parked();
        let shown = |cx: &mut gpui::VisualTestContext| board.update(cx, |b, _| b.shown.clone());
        assert_eq!(shown(cx), [api_id, old_id]);
        let open_only = |cx: &mut gpui::VisualTestContext, on: bool| {
            state.update(cx, |s, cx| {
                s.settings.config.pegboard.open_only = on;
                cx.notify();
            });
            cx.run_until_parked();
        };
        open_only(cx, true);
        assert_eq!(shown(cx), [api_id]);
        // Keys move through the open benches' tabs only.
        let flat_len = |cx: &mut gpui::VisualTestContext| board.update(cx, |b, cx| flat(&b.entries(cx)).len());
        assert_eq!(flat_len(cx), 2);
        // The filter applies to what is left.
        let query = |cx: &mut gpui::VisualTestContext, text: &str| {
            board.update(cx, |b, cx| {
                b.query = text.into();
                b.sync(cx);
            })
        };
        query(cx, "old");
        assert!(shown(cx).is_empty());
        query(cx, "db");
        assert_eq!(shown(cx), [api_id]);
        query(cx, "");
        open_only(cx, false);
        assert_eq!(shown(cx), [api_id, old_id]);
        // Moving a bench keeps the closed ones' places.
        open_only(cx, true);
        assert_eq!(board.update(cx, |b, cx| b.order(cx)), [api_id, old_id]);
    }

    #[test]
    fn saved_times_are_shortened() {
        assert_eq!(short_time("2026-11-01T12:32:05Z"), "2026-11-01 12:32 UTC");
        assert_eq!(short_time("soon"), "soon");
    }
}

//! A bench's window: the bench-colour stripe, the tab bar, and the active
//! tab's panes laid out by its split tree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, PromptLevel, SharedString, Subscription, WeakEntity, Window, canvas,
    div, prelude::*, px, relative, svg,
};
use shed_ai::Limits;
use shed_model::{
    Bench, BenchColor, BenchId, Direction, PaneId, PaneNode, ProjectLink, Rgb, SplitDir, Tab, TabDivider, TabId, Tag,
    groups, tag_color,
};
use shed_model::config::NewPaneDir;
use shed_proto::{BenchRef, ClientMsg, NewTabKind};

use crate::actions::*;
use crate::ai_view::{AiLimits, AiView};
use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu::{self, Menu, MenuEntry, MenuHost, danger, more};
use crate::project_view::ProjectView;
use crate::setup_panel::{PanelEvent, SetupPanel};
use crate::state::{AppState, Focus};
use crate::terminal_view::TerminalView;
use crate::theme::{UiColors, hsla, hsla_alpha};
use crate::window_place::{PlaceTracker, Placed};

/// A tab's height on the tab bar, before `[window] tab_bar_scale`.
const TAB_HEIGHT: f32 = 26.0;
/// The room on either side of a tab's icon and title.
const TAB_PAD_X: f32 = 8.0;
/// The space between groups of tabs (tabs in a group are 2 apart).
const GROUP_GAP: f32 = 18.0;

/// Where a new tab goes: after the shown tab (`None`), or just before or
/// after a tab (`true`: after), in its group.
type Place = Option<(TabId, bool)>;

/// What the inline text field is editing.
#[derive(Clone, Copy, PartialEq)]
enum EditTarget {
    TabName(TabId),
    BenchName,
    AddTag(TabId),
    Badge,
    /// A colour of one's own: `#rrggbb` or a colour's name.
    Color,
    /// A folder to open as a project tab, typed where there is no picker
    /// (placed as asked: `Place`).
    ProjectFolder(Place),
    /// A server to open a remote tab on (`host` or `user@host`).
    RemoteHost(Place),
    /// A new link for the bench: `name url`.
    AddLink,
    /// The bench's link at this index.
    EditLink(usize),
    /// The name of the bench's link at this index.
    RenameLink(usize),
    /// A bookmark's name.
    RenameBookmark(shed_model::BookmarkId),
}

impl EditTarget {
    fn describe(&self) -> &'static str {
        match self {
            EditTarget::TabName(_) => "Renaming the tab",
            EditTarget::BenchName => "Renaming the bench",
            EditTarget::AddTag(_) => "Adding a tag",
            EditTarget::Badge => "Setting the badge",
            EditTarget::Color => "Choosing a colour (#rrggbb or a name)",
            EditTarget::ProjectFolder(_) => "Opening a folder as a project tab",
            EditTarget::RemoteHost(_) => "Opening a remote tab: host, or user@host, as ssh takes it",
            EditTarget::AddLink | EditTarget::EditLink(_) => "A link: name, then the address (app localhost:3000)",
            EditTarget::RenameLink(_) => "The link's name",
            EditTarget::RenameBookmark(_) => "The bookmark's name",
        }
    }
}

/// A tab's one icon, the same on its bench's tab bar and in Shed: what it
/// is (terminal, program running, project, AI, remote), or that it waits
/// for you or is working; and its colour. An AI tab connected to the
/// browser shows a globe, so it is clear which one is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabIcon {
    pub glyph: &'static str,
    pub color: Rgb,
    /// An AI tab whose turn is over waits on this much work it started in
    /// the background (its hourglass carries the number).
    pub background: u32,
    /// Claude is at work in an AI tab: the working mark turns in place of
    /// the icon (`pulse::working_mark`).
    pub working: bool,
}

pub fn tab_icon(tab: &Tab, accent: Rgb, ui: &UiColors) -> TabIcon {
    let waiting = tab.panes.iter().any(|p| p.waiting.is_some() || p.held.is_some());
    let plain = |glyph, color| TabIcon { glyph, color, background: 0, working: false };
    if waiting {
        plain("tool-waiting", ui.ink(accent))
    } else if let Some(ai) = tab.ai_info() {
        let glyph = if ai.browser { "globe" } else { "ai" };
        // (Sessions going on in the background count too.)
        match ai.overall_activity() {
            shed_model::AiActivity::Waiting => plain("tool-waiting", ui.ink(accent)),
            shed_model::AiActivity::Working => TabIcon { working: true, ..plain(glyph, ui.ink(accent)) },
            shed_model::AiActivity::Background(n) => TabIcon { background: n, ..plain("hourglass", ui.ink(accent)) },
            shed_model::AiActivity::Idle => plain(glyph, if ai.browser { ui.text } else { ui.text_dim }),
        }
    } else if tab.project_info().is_some() {
        plain("project", ui.text_dim)
    } else if tab.remote_info().is_some() {
        plain("server", ui.text_dim)
    } else if tab.panes.iter().any(|p| p.tool.is_some()) {
        plain("tool", ui.text_dim)
    } else {
        plain("terminal", ui.text_dim)
    }
}

/// The number of background tasks an AI tab waits on, small beside its
/// hourglass (tab bar, Shed's tab cards), saying so on hover.
pub fn background_badge(id: impl Into<gpui::ElementId>, n: u32, accent: Rgb, ui: &UiColors, size: Pixels) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px(size * 0.3)
        .rounded_sm()
        .text_size(size)
        .font_weight(FontWeight::MEDIUM)
        .bg(hsla_alpha(accent, 0.22))
        .text_color(hsla(ui.ink(accent)))
        .tooltip(menu::tooltip_text(format!("Claude's turn is over; it is {}", shed_model::waiting_on(n))))
        .child(n.to_string())
}

/// `tags` with the one `text` describes added: "prod", or "prod red". Without
/// a colour, the one set for the label (`[tag_colors]`), else its own.
pub fn with_tag(tags: &[Tag], text: &str, config: &shed_model::config::Config) -> Option<Vec<Tag>> {
    let mut words = text.split_whitespace();
    let label = words.next()?;
    let color = words.next().and_then(tag_color).unwrap_or_else(|| config.tag_color_of(label));
    let mut tags: Vec<Tag> = tags.iter().filter(|t| t.label != label).cloned().collect();
    tags.push(Tag { label: label.to_string(), color, tint_tab: false });
    Some(tags)
}

/// Every tag label Shed knows: on tabs of running and closed benches, and
/// with a colour set; each with its colour and how many tabs carry it.
pub fn known_tags(cx: &gpui::App, state: &crate::state::AppState) -> Vec<(String, Rgb, usize)> {
    let mut found: std::collections::BTreeMap<String, (Rgb, usize)> = std::collections::BTreeMap::new();
    let config = &state.settings.config;
    let closed = cx.try_global::<crate::pegboard::PegboardData>().map(|p| p.0.stored.iter().map(|s| &s.bench).collect::<Vec<_>>()).unwrap_or_default();
    for bench in state.tree.benches.iter().chain(closed) {
        for tag in bench.tabs.iter().flat_map(|t| &t.tags) {
            found.entry(tag.label.clone()).or_insert((tag.color, 0)).1 += 1;
        }
    }
    for label in config.tag_colors.keys() {
        found.entry(label.clone()).or_insert((config.tag_color_of(label), 0));
    }
    found.into_iter().map(|(label, (color, n))| (label.clone(), if config.tag_colors.contains_key(&label) { config.tag_color_of(&label) } else { color }, n)).collect()
}

/// About how wide the tab bar's parts are, at scale `s`: each tab as wide
/// as it was laid out last (`measured`), else by its characters, as text is
/// not measured before it is laid out.
struct BarWidths {
    /// Each group's tabs, and whether the group starts a row of its own.
    groups: Vec<(bool, Vec<f32>)>,
    /// The usage limits (0 without).
    limits: f32,
    /// New Tab, Bookmarks, Run Startup or Teardown, Setup, the bench's name.
    actions: Vec<f32>,
    links: Vec<f32>,
}

fn bar_widths(bench: &BarState, s: f32, limits: bool, lifecycle: bool, setup: bool, measured: &HashMap<TabId, f32>) -> BarWidths {
    let ch = 7.4 * s;
    let tab = |t: &BarTab| {
        if let Some(w) = measured.get(&t.id) {
            return *w;
        }
        let tags: f32 = t.tags.iter().map(|g| g.label.chars().count() as f32 * 6.2 * s + 6.0 * s).sum();
        2.0 * TAB_PAD_X * s
            + 20.0 * s
            + t.title.chars().count() as f32 * ch
            + tags
            + if t.zoom != 0.0 { 44.0 * s } else { 0.0 }
            + if t.private { 18.0 * s } else { 0.0 }
            + if t.icon.background > 0 { 22.0 * s } else { 0.0 }
    };
    let mut groups: Vec<(bool, Vec<f32>)> = Vec::new();
    for (i, t) in bench.tabs.iter().enumerate() {
        if i == 0 || t.divider != TabDivider::None {
            groups.push((i > 0 && t.divider == TabDivider::Row, Vec::new()));
        }
        groups.last_mut().unwrap().1.push(tab(t));
    }
    // New Tab, Bookmarks.
    let mut actions = vec![32.0 * s, 32.0 * s];
    if lifecycle {
        actions.push(32.0 * s);
    }
    if setup {
        actions.push(32.0 * s);
    }
    actions.push(bench.name.chars().count() as f32 * 8.0 * s + 54.0 * s);
    let links = bench.links.iter().map(|l| l.name.chars().count().min(22) as f32 * 7.0 * s + 40.0 * s).collect();
    BarWidths { groups, limits: if limits { 176.0 * s } else { 0.0 }, actions, links }
}

impl BarWidths {
    /// How many rows the tabs take in `room` (their area's width), wrapping
    /// as the tab bar does: a group onto the next row whole, inside only
    /// when it alone is wider.
    fn tab_rows(&self, room: f32, s: f32) -> usize {
        let room = room - 8.0 * s;
        let (gap, inner) = (GROUP_GAP * s, 2.0 * s);
        let (mut rows, mut x) = (1, 0.0f32);
        for (row, tabs) in &self.groups {
            if *row && x > 0.0 {
                rows += 1;
                x = 0.0;
            }
            let w = tabs.iter().sum::<f32>() + inner * tabs.len().saturating_sub(1) as f32;
            if x > 0.0 && x + gap + w > room {
                rows += 1;
                x = 0.0;
            }
            if x == 0.0 && w > room {
                for t in tabs {
                    if x > 0.0 && x + inner + t > room {
                        rows += 1;
                        x = 0.0;
                    }
                    x += if x > 0.0 { inner + t } else { *t };
                }
            } else {
                x += if x > 0.0 { gap + w } else { w };
            }
        }
        rows
    }
}

/// Items side by side, `gap` apart.
fn row_width(items: &[f32], gap: f32) -> f32 {
    items.iter().sum::<f32>() + gap * items.len().saturating_sub(1) as f32
}

/// Where a tab dragged on the tab bar would go.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DropAt {
    /// Next to this tab, in its group (`true`: after it).
    Tab(TabId, bool),
    /// A group of its own, in the gap before this tab's group.
    NewGroup(TabId),
    /// A row of its own, before the row this tab starts.
    NewRow(TabId),
}

/// Where the tab bar's tabs were laid out last (window coordinates), in
/// order, each with its group: for drops, the group menu and the lines
/// between groups.
#[derive(Default)]
struct BarLayout {
    tabs: Vec<(TabId, usize, Bounds<Pixels>)>,
    /// How wide the bar took each tab to be when it arranged its controls.
    assumed: Vec<f32>,
    /// Drawn again to arrange the controls by the tabs' real widths.
    recheck: bool,
    /// Where the Bookmarks button is: the list opens under it.
    bookmarks_button: Option<Bounds<Pixels>>,
}

impl BarLayout {
    /// The tabs by rows, top to bottom (positions in `tabs`).
    fn rows(&self) -> Vec<Vec<usize>> {
        let mut rows: Vec<Vec<usize>> = Vec::new();
        for (i, (_, _, b)) in self.tabs.iter().enumerate() {
            if b.size.width <= px(0.0) {
                continue;
            }
            match rows.last_mut() {
                Some(row) if (self.tabs[row[0]].2.top() - b.top()).abs() < px(2.0) => row.push(i),
                _ => rows.push(vec![i]),
            }
        }
        rows
    }

    /// The row nearest `y`.
    fn row_at(&self, y: Pixels) -> Option<Vec<usize>> {
        self.rows().into_iter().min_by_key(|row| {
            let b = self.tabs[row[0]].2;
            let d = if y < b.top() { b.top() - y } else if y > b.bottom() { y - b.bottom() } else { px(0.0) };
            (f32::from(d) * 100.0) as i64
        })
    }

    /// Where a tab dropped at `at` goes: at a row's very start, a row of its
    /// own; in the gap between two groups, a group of its own; else next to
    /// the nearest tab, in its group.
    fn drop_at(&self, at: gpui::Point<Pixels>, s: f32) -> Option<DropAt> {
        let row = self.row_at(at.y)?;
        let first = &self.tabs[row[0]];
        if at.x < first.2.left() + px(8.0 * s) {
            return Some(DropAt::NewRow(first.0));
        }
        for (k, &i) in row.iter().enumerate() {
            let (id, group, b) = self.tabs[i];
            if at.x < b.left() && k > 0 {
                let (prev, prev_group, _) = self.tabs[row[k - 1]];
                return Some(if prev_group != group { DropAt::NewGroup(id) } else { DropAt::Tab(prev, true) });
            }
            if at.x < b.right() {
                return Some(DropAt::Tab(id, at.x > b.center().x));
            }
        }
        row.last().map(|&i| DropAt::Tab(self.tabs[i].0, true))
    }

    /// The group at `at`: the one it is in, or whose line it is on; past a
    /// row's end, the row's last.
    fn group_at(&self, at: gpui::Point<Pixels>) -> Option<usize> {
        let row = self.row_at(at.y)?;
        let i = row.iter().copied().find(|&i| at.x < self.tabs[i].2.right()).or(row.last().copied())?;
        Some(self.tabs[i].1)
    }

    /// Faint lines between groups that share a row (none at a row's start).
    fn separators(&self, s: f32) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        for row in self.rows() {
            for pair in row.windows(2) {
                let ((_, ga, a), (_, gb, b)) = (self.tabs[pair[0]], self.tabs[pair[1]]);
                if ga != gb {
                    let h = b.size.height * 0.56;
                    let x = (a.right() + b.left()) / 2.0;
                    out.push(Bounds::new(gpui::point(x - px(0.5 * s), b.top() + (b.size.height - h) / 2.0), gpui::size(px(1.0 * s), h)));
                }
            }
        }
        out
    }

    /// The drop caret for `hint`: a bar beside a tab, in the gap between
    /// groups, or at the row's start with a stroke along its top.
    fn caret(&self, hint: DropAt, left: Pixels, s: f32) -> Vec<Bounds<Pixels>> {
        let find = |t: TabId| self.tabs.iter().position(|(id, _, _)| *id == t);
        let w = px(2.0 * s);
        let bar = |x: Pixels, b: Bounds<Pixels>| Bounds::new(gpui::point(x - w / 2.0, b.top() - px(1.0 * s)), gpui::size(w, b.size.height + px(2.0 * s)));
        match hint {
            DropAt::Tab(t, after) => {
                let Some(i) = find(t) else { return Vec::new() };
                let b = self.tabs[i].2;
                vec![bar(if after { b.right() + px(1.0 * s) } else { b.left() - px(1.0 * s) }, b)]
            }
            DropAt::NewGroup(t) => {
                let Some(i) = find(t) else { return Vec::new() };
                let b = self.tabs[i].2;
                let prev = i.checked_sub(1).map(|p| self.tabs[p].2.right()).filter(|r| *r < b.left()).unwrap_or(b.left() - px(GROUP_GAP * s));
                vec![bar((prev + b.left()) / 2.0, b)]
            }
            DropAt::NewRow(t) => {
                let Some(i) = find(t) else { return Vec::new() };
                let b = self.tabs[i].2;
                let x = left + px(2.0 * s);
                let caret = bar(x, b);
                vec![caret, Bounds::new(caret.origin, gpui::size(px(12.0 * s), w))]
            }
        }
    }
}

fn item(label: impl Into<SharedString>, action: impl Fn(&mut BenchWindow, &mut Window, &mut Context<BenchWindow>) + 'static) -> MenuEntry<BenchWindow> {
    menu::item(label, action)
}

/// A split border being dragged.
struct SplitDrag {
    tab: TabId,
    path: Vec<bool>,
    dir: SplitDir,
    layout: PaneNode,
}

/// The payload of a tab being dragged, drawn under the pointer.
#[derive(Clone)]
pub struct DraggedTab {
    pub tab: TabId,
    pub title: String,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::current_ui(cx);
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(hsla(ui.tab_active))
            .border_1()
            .border_color(hsla(ui.divider))
            .text_size(px(13.0))
            .text_color(hsla(ui.text))
            .child(self.title.clone())
    }
}

pub struct BenchWindow {
    bench: BenchId,
    state: Entity<AppState>,
    /// What the window shows of `AppState`, as of its last change: the
    /// window draws from this, never from `AppState` (which changes with
    /// every title a program sets, in any bench).
    shown: Shown,
    /// What the tab bar shows of `AppState` (`None` once the bench is gone).
    bar_state: Option<BarState>,
    bar: Entity<TabBar>,
    /// How many times it was drawn (tests check what a change draws).
    drawn: usize,
    /// What was last given keyboard focus, so focus follows the keeper's
    /// active pane without fighting the user's clicks.
    focused: Option<Focus>,
    title: String,
    editing: Option<(EditTarget, Entity<LineEditor>, Subscription)>,
    menu: Option<Menu<Self>>,
    /// Where each split container was laid out last frame, by path in the tree.
    split_bounds: Rc<RefCell<HashMap<Vec<bool>, Bounds<Pixels>>>>,
    drag: Option<SplitDrag>,
    /// The Setup panel, while open.
    setup: Option<(Entity<SetupPanel>, Subscription)>,
    /// A touchpad's Ctrl+scroll so far, towards a zoom step.
    wheel_zoom: Rc<std::cell::Cell<f32>>,
    place: PlaceTracker,
    /// The shells each server keeps, as last asked (for New Remote Tab).
    remote_shells: HashMap<String, RemoteShells>,
    /// The server whose shells the open menu lists.
    remote_menu: Option<String>,
    /// While a tab is dragged over the tab bar: where it would go.
    drop_hint: Option<DropAt>,
    /// The bookmarks list is open.
    bookmarks_open: bool,
    /// Why the bookmark chosen could not be gone to, in the list.
    bookmark_problem: Option<String>,
    _following: Subscription,
}

/// What a bench window draws of `AppState` outside its tab bar. Made again
/// at every change to `AppState`, and drawn again only when it differs.
#[derive(Clone, PartialEq)]
struct Shown {
    ui: UiColors,
    /// The terminal background, which the window is cleared to.
    background: Rgb,
    /// What is wrong in `shed.toml`.
    errors: Vec<String>,
    /// The bench's colour and its shown tab (`None` once the bench is gone).
    bench: Option<(Rgb, Content)>,
    /// The bench's bookmarks and folder (for the bookmarks list).
    bookmarks: Vec<shed_model::Bookmark>,
    folder: std::path::PathBuf,
}

/// The shown tab's content: the views it holds, and how its panes are laid out.
#[derive(Clone, PartialEq)]
enum Content {
    None,
    Project(Option<Entity<ProjectView>>),
    Ai(Option<Entity<AiView>>),
    Panes(Panes),
}

/// A terminal tab: its layout, its active pane, and its panes' views.
#[derive(Clone, PartialEq)]
struct Panes {
    tab: TabId,
    layout: PaneNode,
    active: PaneId,
    views: Vec<(PaneId, Option<Entity<TerminalView>>)>,
}

impl Shown {
    fn of(bench_id: BenchId, state: &AppState) -> Self {
        let settings = &state.settings;
        let bench = state.tree.bench(bench_id).map(|b| {
            let content = match b.active_tab() {
                Some(tab) if tab.project_info().is_some() => Content::Project(state.projects.get(&tab.id).cloned()),
                Some(tab) if tab.ai_info().is_some() => Content::Ai(state.ai_views.get(&tab.id).cloned()),
                Some(tab) => Content::Panes(Panes {
                    tab: tab.id,
                    layout: tab.layout.clone(),
                    active: tab.active_pane,
                    views: tab.panes.iter().map(|p| (p.id, state.panes.get(&p.id).cloned())).collect(),
                }),
                None => Content::None,
            };
            (b.color.accent(), content)
        });
        let (bookmarks, folder) = state.tree.bench(bench_id).map(|b| (b.bookmarks.clone(), b.cwd.clone())).unwrap_or_default();
        Self { ui: settings.ui.clone(), background: settings.palette.background, errors: settings.errors.clone(), bench, bookmarks, folder }
    }
}

enum RemoteShells {
    Looking(Vec<shed_client::remote::RemoteShell>),
    Found(Vec<shed_client::remote::RemoteShell>),
    Failed(String),
}

impl Placed for BenchWindow {
    fn tracker(&mut self) -> &mut PlaceTracker {
        &mut self.place
    }
}

impl BenchWindow {
    pub fn new(bench: BenchId, state: Entity<AppState>, place: PlaceTracker, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Closing the window closes the bench, after asking; hiding is its own button.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |this, cx| this.close_from_window(window, cx)).ok();
            false
        });
        cx.observe(&state, |this, _, cx| this.follow_state(cx)).detach();
        let me = cx.entity().downgrade();
        let bar = cx.new(|_| TabBar { window: me, model: None, drawn: 0, layout: Rc::default() });
        // The usage limits' time left keeps up: the tab bar draws again
        // each minute while it shows them.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(60)).await;
            let alive = this.update(cx, |this, cx| {
                if this.bar_state.as_ref().is_some_and(|b| b.limits.is_some()) {
                    this.bar.update(cx, |_, cx| cx.notify());
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
        // SHED_PERF_FRAMES=<n>: redraw the tab bar n times a second, as
        // hovering in it does, to measure what a frame of this window costs.
        if let Some(fps) = std::env::var("SHED_PERF_FRAMES").ok().and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0) {
            let bar = bar.downgrade();
            cx.spawn_in(window, async move |_, cx| loop {
                cx.background_executor().timer(std::time::Duration::from_millis(1000 / fps)).await;
                if bar.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            })
            .detach();
        }
        let following = PlaceTracker::follow(window, cx);
        let shown = Shown::of(bench, state.read(cx));
        let mut this = Self {
            bench,
            state,
            shown,
            bar_state: None,
            bar,
            drawn: 0,
            focused: None,
            drop_hint: None,
            title: String::new(),
            editing: None,
            menu: None,
            split_bounds: Rc::default(),
            drag: None,
            setup: None,
            wheel_zoom: Rc::default(),
            place,
            remote_shells: HashMap::new(),
            remote_menu: None,
            bookmarks_open: false,
            bookmark_problem: None,
            _following: following,
        };
        this.follow_state(cx);
        this
    }

    /// Takes what the window and its tab bar show from `AppState`, after a
    /// change to it: each draws again only if what it shows changed.
    fn follow_state(&mut self, cx: &mut Context<Self>) {
        let limits = cx.try_global::<AiLimits>().map(|l| l.0);
        let state = self.state.read(cx);
        let shown = Shown::of(self.bench, state);
        let bar = state.tree.bench(self.bench).map(|b| BarState::of(b, state, limits));
        if shown != self.shown {
            self.shown = shown;
            cx.notify();
        }
        if bar != self.bar_state {
            self.bar_state = bar;
            self.update_bar(cx);
        }
    }

    /// Gives the tab bar what it shows now; it draws again if that changed.
    fn update_bar(&mut self, cx: &mut Context<Self>) {
        let model = self.bar_state.clone().map(|bench| BarModel {
            bench,
            editing: self.editing.as_ref().map(|(t, e, _)| (*t, e.clone())),
            drop_hint: self.drop_hint,
            setup_open: self.setup.is_some(),
            bookmarks_open: self.bookmarks_open,
        });
        self.bar.update(cx, |bar, cx| {
            if bar.model != model {
                // An AI tab at work: its mark turns with the app's beat,
                // redrawing the bar alone.
                let working = model.as_ref().is_some_and(|m| m.bench.tabs.iter().any(|t| t.icon.working));
                crate::pulse::animate(cx, working);
                bar.model = model;
                cx.notify();
            }
        });
    }

    /// Draws the window again after a change to its own state (a text
    /// field, the Setup panel, a menu), and the tab bar if it shows it.
    fn redraw(&mut self, cx: &mut Context<Self>) {
        self.update_bar(cx);
        cx.notify();
    }

    fn toggle_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.setup.take().is_none() {
            let panel = cx.new(|cx| SetupPanel::new(self.bench, None, self.state.clone(), cx));
            let sub = cx.subscribe_in(&panel, window, |this, _, event: &PanelEvent, window, cx| match event {
                PanelEvent::Close => {
                    this.setup = None;
                    this.refocus(window, cx);
                }
                PanelEvent::Done => this.refocus(window, cx),
            });
            self.setup = Some((panel, sub));
        } else {
            self.refocus(window, cx);
        }
        self.redraw(cx);
    }

    /// Follows the tree: window title, and keyboard focus on the active pane.
    /// The caller passes what it needs from the app state, which may be
    /// mid-update and so cannot be read here.
    pub fn sync(&mut self, title: String, active: Option<(Focus, FocusHandle)>, window: &mut Window, cx: &mut Context<Self>) {
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        let active_id = active.as_ref().map(|(id, _)| *id);
        if active_id != self.focused && self.editing.is_none() {
            if let Some((_, handle)) = &active {
                window.focus(handle, cx);
            }
            self.focused = active_id;
        }
    }

    /// Re-applies focus after an overlay (a text field, a menu) closes.
    fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.redraw(cx);
        let Some((focus, handle)) = self.state.read(cx).active_focus(self.bench, cx) else { return };
        window.focus(&handle, cx);
        self.focused = Some(focus);
    }

    fn send(&self, msg: ClientMsg, cx: &Context<Self>) {
        self.state.read(cx).conn.send_msg(msg);
    }

    fn bench<'a>(&self, cx: &'a Context<Self>) -> Option<&'a Bench> {
        self.state.read(cx).tree.bench(self.bench)
    }

    fn active_tab<'a>(&self, cx: &'a Context<Self>) -> Option<&'a Tab> {
        self.bench(cx).and_then(|b| b.active_tab())
    }

    fn tab<'a>(&self, tab: TabId, cx: &'a Context<Self>) -> Option<&'a Tab> {
        self.bench(cx).and_then(|b| b.tabs.iter().find(|t| t.id == tab))
    }

    fn cycle_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(bench) = self.bench(cx) else { return };
        let Some(i) = bench.tab_index(bench.active_tab) else { return };
        let n = bench.tabs.len() as isize;
        let tab = bench.tabs[(i as isize + delta).rem_euclid(n) as usize].id;
        self.send(ClientMsg::FocusTab { tab }, cx);
    }

    /// Where a new pane next to `pane` opens: its directory, or the bench's (`None`).
    fn new_pane_cwd(&self, pane: Option<PaneId>, cx: &Context<Self>) -> Option<std::path::PathBuf> {
        let state = self.state.read(cx);
        match state.settings.new_pane_dir {
            NewPaneDir::Bench => None,
            NewPaneDir::Current => pane.and_then(|p| state.tree.pane(p)).map(|p| p.cwd.clone()),
        }
    }

    fn new_tab(&mut self, cx: &mut Context<Self>) {
        let cwd = self.new_pane_cwd(self.active_tab(cx).map(|t| t.active_pane), cx);
        self.send(ClientMsg::NewTab { bench: self.bench, cwd }, cx);
    }

    fn split_pane(&mut self, pane: PaneId, dir: SplitDir, cx: &mut Context<Self>) {
        let cwd = self.new_pane_cwd(Some(pane), cx);
        self.send(ClientMsg::Split { pane, dir, cwd }, cx);
    }

    fn split(&mut self, dir: SplitDir, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_tab(cx).map(|t| t.active_pane) {
            self.split_pane(pane, dir, cx);
        }
    }

    fn move_focus(&mut self, dir: Direction, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab(cx) else { return };
        if let Some(pane) = tab.layout.neighbor(tab.active_pane, dir) {
            self.send(ClientMsg::FocusPane { pane }, cx);
        }
    }

    /// Closes a tab, after asking.
    fn close_tab(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tab(tab, cx).cloned() else { return };
        let conn = self.state.read(cx).conn.clone();
        crate::state::close_tab(&conn, &tab, BenchRef::Running(self.bench), window, cx);
    }

    /// Opens a tab of `kind` where `place` says.
    fn open_tab(&self, place: Place, kind: NewTabKind, cx: &Context<Self>) {
        let bench = self.bench;
        let msg = match (place, kind) {
            (Some((tab, after)), kind) => ClientMsg::NewTabNextToOfKind { tab, after, kind },
            (None, NewTabKind::Terminal { cwd }) => ClientMsg::NewTab { bench, cwd },
            (None, NewTabKind::Project { root }) => ClientMsg::NewProjectTab { bench, root },
            (None, NewTabKind::Ai { folder }) => ClientMsg::NewAiTab { bench, folder },
            (None, NewTabKind::Remote { host, join }) => ClientMsg::NewRemoteTab { bench, host, join },
        };
        self.send(msg, cx);
    }

    /// The kinds of tab to open, as the New Tab button's menu has them, and
    /// a tab's New Tab to the Left and Right: a terminal, a project (its
    /// folder asked for), an AI tab (in which folder), a remote tab (on
    /// which server).
    fn new_tab_menu(place: Place) -> Vec<MenuEntry<Self>> {
        vec![
            item("New Terminal Tab", move |this, _, cx| match place {
                Some((tab, after)) => this.new_tab_beside(tab, after, cx),
                None => this.new_tab(cx),
            })
            .icon("terminal"),
            item("New Project Tab…", move |this, window, cx| this.new_project_tab(place, window, cx)).icon("project"),
            more("New AI Tab", move |this: &mut Self, cx| this.ai_folder_menu(place, cx)).icon("ai"),
            more("New Remote Tab", move |this: &mut Self, cx| this.remote_host_menu(place, cx)).icon("server"),
        ]
    }

    /// Asks for a folder and opens it as a project tab; where the desktop
    /// has no folder picker, the folder is typed instead.
    fn new_project_tab(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        let picked = crate::state::pick_folder(cx);
        cx.spawn_in(window, async move |this, cx| {
            let picked = picked.await;
            this.update_in(cx, |this, window, cx| match picked {
                Ok(Some(root)) => this.open_tab(place, NewTabKind::Project { root }, cx),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("no folder picker: {e}");
                    this.start_edit(EditTarget::ProjectFolder(place), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Where a new AI tab can work: the bench's projects, its folder, or one to pick.
    fn ai_folder_menu(&mut self, place: Place, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some(bench) = self.bench(cx) else { return Vec::new() };
        let mut folders: Vec<std::path::PathBuf> = bench.tabs.iter().filter_map(|t| Some(t.project_info()?.root.clone())).collect();
        if !folders.contains(&bench.cwd) {
            folders.push(bench.cwd.clone());
        }
        let mut entries: Vec<MenuEntry<Self>> = folders
            .into_iter()
            .map(|folder| {
                let label = shed_save::home_relative(&folder);
                item(label, move |this, _, cx| this.open_tab(place, NewTabKind::Ai { folder: folder.clone() }, cx)).icon("folder")
            })
            .collect();
        entries.push(item("Other Folder…", move |_, window, cx| {
            let picked = crate::state::pick_folder(cx);
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(Some(folder)) = picked.await {
                    this.update(cx, |this, cx| this.open_tab(place, NewTabKind::Ai { folder }, cx)).ok();
                }
            })
            .detach();
        }));
        entries
    }

    /// Servers for a new remote tab: those remote tabs used (latest first,
    /// marked so), those in `~/.ssh/config` (not Git hosts), or one to type.
    fn remote_host_menu(&mut self, place: Place, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let recent = shed_client::remote::recent_hosts(&self.state.read(cx).tree);
        let mut entries: Vec<MenuEntry<Self>> = shed_client::remote::known_hosts(&recent)
            .into_iter()
            .map(|host| {
                let used = recent.contains(&host);
                more(host.clone(), move |this: &mut Self, cx| this.remote_shell_menu(&host, place, cx)).icon(if used { "history" } else { "server" })
            })
            .collect();
        entries.push(item("Other Host…", move |this, window, cx| this.start_edit(EditTarget::RemoteHost(place), window, cx)));
        entries
    }

    /// A server's menu: a new shell, or one it keeps already (from another
    /// PC, or closed here), asked for over ssh while the menu is open.
    fn remote_shell_menu(&mut self, host: &str, place: Place, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        self.remote_menu = Some(host.to_string());
        let known = match self.remote_shells.remove(host) {
            Some(RemoteShells::Found(list) | RemoteShells::Looking(list)) => list,
            _ => Vec::new(),
        };
        self.remote_shells.insert(host.to_string(), RemoteShells::Looking(known));
        let h = host.to_string();
        cx.spawn(async move |this, cx| {
            let asked = h.clone();
            let found = cx.background_executor().spawn(async move { shed_client::remote::list_shells(&asked) }).await;
            this.update(cx, |this, cx| {
                let shells = match found {
                    Ok(list) => RemoteShells::Found(list),
                    Err(e) => RemoteShells::Failed(e.to_string()),
                };
                this.remote_shells.insert(h.clone(), shells);
                if this.remote_menu.as_deref() == Some(h.as_str()) {
                    let entries = this.remote_shell_entries(&h, place, cx);
                    if let Some(menu) = &mut this.menu {
                        menu.entries = entries;
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        self.remote_shell_entries(host, place, cx)
    }

    fn remote_shell_entries(&self, host: &str, place: Place, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let h = host.to_string();
        let mut entries = vec![
            item("New Shell", move |this, _, cx| this.open_tab(place, NewTabKind::Remote { host: h.clone(), join: None }, cx)).icon("plus"),
            MenuEntry::Separator,
        ];
        // Shells open on this PC already are not offered again.
        let tree = &self.state.read(cx).tree;
        let open_here = |id: &str| tree.benches.iter().flat_map(|b| &b.tabs).any(|t| t.remote_info().is_some_and(|r| r.host == host && r.id == id));
        let note = |text: String| item(text, |_, _, _| {});
        match self.remote_shells.get(host) {
            Some(RemoteShells::Failed(e)) => entries.push(note(format!("Cannot list its shells: {e}"))),
            Some(RemoteShells::Found(list) | RemoteShells::Looking(list)) => {
                let looking = matches!(self.remote_shells.get(host), Some(RemoteShells::Looking(_)));
                let mut any = false;
                for shell in list.iter().filter(|s| !open_here(&s.id)) {
                    any = true;
                    let (h, id) = (host.to_string(), shell.id.clone());
                    entries.push(
                        item(shed_client::remote::describe(shell), move |this, _, cx| {
                            this.open_tab(place, NewTabKind::Remote { host: h.clone(), join: Some(id.clone()) }, cx)
                        })
                        .icon("terminal"),
                    );
                }
                if looking {
                    entries.push(note("Looking for its shells…".into()));
                } else if !any {
                    entries.push(note("No other shells kept there".into()));
                }
            }
            None => entries.push(note("Looking for its shells…".into())),
        }
        entries
    }

    /// Opens the project a pane is in (its Git repository, else its folder).
    fn open_pane_project(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let Some(cwd) = self.state.read(cx).tree.pane(pane).map(|p| p.cwd.clone()) else { return };
        let root = shed_project::project_root(&cwd);
        self.send(ClientMsg::NewProjectTab { bench: self.bench, root }, cx);
    }

    /// Closes a pane; a tab's only pane takes the tab along, so that asks first.
    fn close_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.bench(cx).and_then(|b| b.tabs.iter().find(|t| t.panes.iter().any(|p| p.id == pane)));
        match tab {
            Some(tab) if tab.panes.len() == 1 => self.close_tab(tab.id, window, cx),
            _ => self.send(ClientMsg::ClosePane { pane }, cx),
        }
    }

    fn close_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench(cx) else { return };
        let message = format!("Close the bench “{}”?", bench.name);
        let name = bench.name.clone();
        // Kept with the saved bench, so it opens the same next time.
        self.place.record(window);
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(
                "Closing stops every tool in it and keeps it saved, to load again later. \
                 Deleting also removes the saved bench and its launcher.\n\n\
                 To keep the tools running, hide the bench instead.",
            ),
            &["Close Bench", "Delete Bench", "Cancel"],
            cx,
        );
        let (bench, conn) = (self.bench, self.state.read(cx).conn.clone());
        cx.spawn(async move |_, _| match answer.await {
            Ok(0) => conn.send_msg(ClientMsg::CloseBench { bench }),
            Ok(1) => conn.send_msg(ClientMsg::DeleteBench { name }),
            _ => {}
        })
        .detach();
    }

    /// The window's close button: closes the bench, after asking, or hides it.
    fn close_from_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench(cx) else { return };
        if bench.closing {
            return;
        }
        let message = format!("Close the bench “{}”?", bench.name);
        self.place.record(window);
        // Asked from the taskbar, the window may be minimized or behind others,
        // and the question is drawn inside it.
        window.activate();
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(
                "Closing stops every tool in it and keeps it saved, to load again later.\n\n\
                 Hiding closes only the window: the tools keep running, and Shed shows it again.",
            ),
            &["Close Bench", "Hide", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| match answer.await {
            Ok(0) => this.update(cx, |this, cx| this.send(ClientMsg::CloseBench { bench: this.bench }, cx)).ok(),
            Ok(1) => this.update_in(cx, |this, window, cx| this.hide(window, cx)).ok(),
            _ => None,
        })
        .detach();
    }

    /// Closes the window; the bench keeps running, and Shed shows it again.
    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.place.record(window);
        let (bench, state) = (self.bench, self.state.clone());
        // Closing this window from its own update would borrow it twice.
        cx.defer(move |cx| state.update(cx, |state, cx| state.hide_bench(bench, cx)));
    }

    fn start_edit(&mut self, target: EditTarget, window: &mut Window, cx: &mut Context<Self>) {
        let current = match target {
            EditTarget::TabName(tab) => self.tab(tab, cx).map(|t| t.display_title()),
            EditTarget::BenchName => self.bench(cx).map(|b| b.name.clone()),
            EditTarget::AddTag(_) => Some(String::new()),
            EditTarget::Badge => self.bench(cx).map(|b| b.badge.clone().unwrap_or_default()),
            EditTarget::Color => self.bench(cx).map(|b| format!("#{:06X}", b.color.accent().to_hex())),
            EditTarget::ProjectFolder(_) => self.bench(cx).map(|b| shed_save::home_relative(&b.cwd)),
            EditTarget::RemoteHost(_) | EditTarget::AddLink => Some(String::new()),
            EditTarget::EditLink(i) => self.bench(cx).and_then(|b| b.links.get(i)).map(crate::links::unparse),
            EditTarget::RenameLink(i) => self.bench(cx).and_then(|b| b.links.get(i)).map(|l| l.name.clone()),
            EditTarget::RenameBookmark(id) => self.bench(cx).and_then(|b| b.bookmarks.iter().find(|k| k.id == id)).map(|k| k.label.clone()),
        };
        let Some(current) = current else { return };
        let editor = cx.new(|cx| LineEditor::selected(current, cx).submit_on_blur(window, cx).without_hint());
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let sub = cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, window, cx| {
            let Some((target, _, _)) = this.editing.take() else { return };
            if let EditorEvent::Submit(text) = event {
                this.submit_edit(target, text.clone(), cx);
            }
            this.refocus(window, cx);
        });
        self.editing = Some((target, editor, sub));
        self.redraw(cx);
    }

    fn submit_edit(&mut self, target: EditTarget, text: String, cx: &mut Context<Self>) {
        let msg = match target {
            EditTarget::TabName(tab) => ClientMsg::RenameTab { tab, name: Some(text) },
            EditTarget::BenchName => ClientMsg::RenameBench { bench: self.bench, name: text },
            EditTarget::Color => match BenchColor::parse(&text) {
                Some(color) => ClientMsg::SetBenchColor { bench: self.bench, color },
                None => return,
            },
            EditTarget::ProjectFolder(place) => return self.open_tab(place, NewTabKind::Project { root: text.trim().into() }, cx),
            EditTarget::RemoteHost(_) if text.trim().is_empty() => return,
            EditTarget::RemoteHost(place) => return self.open_tab(place, NewTabKind::Remote { host: text.trim().to_string(), join: None }, cx),
            EditTarget::RenameBookmark(_) if text.trim().is_empty() => return,
            EditTarget::RenameBookmark(id) => ClientMsg::RenameBookmark { bench: self.bench, id, label: text },
            EditTarget::RenameLink(i) => {
                let Some(bench) = self.bench(cx) else { return };
                let Some(links) = crate::links::renamed(&bench.links, i, &text) else { return };
                ClientMsg::SetBenchLinks { bench: self.bench, links }
            }
            EditTarget::AddLink | EditTarget::EditLink(_) => {
                let Some(link) = crate::links::parse(&text) else { return };
                let Some(bench) = self.bench(cx) else { return };
                let at = match target {
                    EditTarget::EditLink(i) => Some(i),
                    _ => None,
                };
                ClientMsg::SetBenchLinks { bench: self.bench, links: crate::links::with(&bench.links, link, at) }
            }
            EditTarget::Badge => {
                let badge: String = text.chars().filter(|c| !c.is_whitespace()).take(2).collect();
                ClientMsg::SetBadge { bench: self.bench, badge: (!badge.is_empty()).then_some(badge) }
            }
            EditTarget::AddTag(tab) => {
                let Some(t) = self.tab(tab, cx) else { return };
                let Some(tags) = with_tag(&t.tags, &text, &self.state.read(cx).settings.config) else { return };
                ClientMsg::SetTags { tab, tags }
            }
        };
        self.send(msg, cx);
    }

    fn tab_menu(&self, tab_id: TabId, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some(tab) = self.tab(tab_id, cx) else { return Vec::new() };
        let mut entries = vec![
            // Any kind of tab, as the New Tab button's menu offers them.
            more("New Tab to the Left", move |_: &mut Self, _| Self::new_tab_menu(Some((tab_id, false)))).icon("tab-new"),
            more("New Tab to the Right", move |_: &mut Self, _| Self::new_tab_menu(Some((tab_id, true)))).icon("tab-new"),
            MenuEntry::Separator,
            item("Rename Tab…", move |this, window, cx| this.start_edit(EditTarget::TabName(tab_id), window, cx)).icon("pencil"),
            // The tags Shed knows, to pick from; or a new one.
            more("Add Tag", move |this: &mut Self, cx| {
                let on: Vec<String> = this.tab(tab_id, cx).map(|t| t.tags.iter().map(|t| t.label.clone()).collect()).unwrap_or_default();
                let known = known_tags(cx, this.state.read(cx));
                let mut entries: Vec<MenuEntry<Self>> = known
                    .into_iter()
                    .filter(|(label, ..)| !on.contains(label))
                    .map(|(label, color, _)| {
                        item(label.clone(), move |this, _, cx| {
                            let Some(t) = this.tab(tab_id, cx) else { return };
                            let Some(tags) = with_tag(&t.tags, &label, &this.state.read(cx).settings.config) else { return };
                            this.send(ClientMsg::SetTags { tab: tab_id, tags }, cx);
                        })
                        .dot(color)
                    })
                    .collect();
                if !entries.is_empty() {
                    entries.push(MenuEntry::Separator);
                }
                entries.push(item("New Tag…", move |this, window, cx| this.start_edit(EditTarget::AddTag(tab_id), window, cx)).icon("plus"));
                entries
            })
            .icon("tag"),
        ];
        // Groups: into another, or a new one from here.
        if let Some(bench) = self.bench(cx) {
            let now = groups::of(bench);
            let all = groups::groups(&now);
            let at = now.iter().position(|(t, _)| *t == tab_id).unwrap_or(0);
            let alone_last = all.last().is_some_and(|g| g.len() == 1 && g.start == at);
            if all.len() > 1 || !alone_last {
                entries.push(more("Move to Group", move |this: &mut Self, cx| this.move_to_group_menu(tab_id, cx)).icon("tab"));
            }
            if !all.iter().any(|g| g.start == at) {
                entries.push(item("New Group From Here", move |this, _, cx| this.send(ClientMsg::SetTabDivider { tab: tab_id, divider: TabDivider::Space }, cx)));
            }
        }
        if tab.remote_info().is_some() {
            entries.push(item("Reconnect", move |this, _, cx| this.send(ClientMsg::ReconnectRemote { tab: tab_id }, cx)).icon("refresh"));
        }
        match tab.project_info() {
            _ if tab.ai_info().is_some() => {}
            Some(project) => {
                let (root, path) = (project.root.clone(), project.root.display().to_string());
                entries.push(
                    item("Open Terminal Here", move |this, _, cx| this.send(ClientMsg::NewTab { bench: this.bench, cwd: Some(root.clone()) }, cx))
                        .icon("terminal"),
                );
                entries.push(item("Copy Path", move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.clone()))));
            }
            None => entries.push(
                item("Setup…", move |this, window, cx| {
                    this.send(ClientMsg::FocusTab { tab: tab_id }, cx);
                    if this.setup.is_none() {
                        this.toggle_setup(window, cx);
                    }
                })
                .icon("rack"),
            ),
        }
        for tag in &tab.tags {
            let label = tag.label.clone();
            let tint = tag.tint_tab;
            entries.push(item(format!("Tint for “{}”{}", tag.label, if tint { "  ✓" } else { "" }), {
                let label = label.clone();
                move |this, _, cx| {
                    let Some(t) = this.tab(tab_id, cx) else { return };
                    let tags = t.tags.iter().cloned().map(|mut t| {
                        if t.label == label {
                            t.tint_tab = !t.tint_tab;
                        }
                        t
                    });
                    let tags = tags.collect();
                    this.send(ClientMsg::SetTags { tab: tab_id, tags }, cx);
                }
            }));
            entries.push(
                item(format!("Remove Tag “{}”", tag.label), move |this, _, cx| {
                    let Some(t) = this.tab(tab_id, cx) else { return };
                    let tags = t.tags.iter().filter(|t| t.label != label).cloned().collect();
                    this.send(ClientMsg::SetTags { tab: tab_id, tags }, cx);
                })
                .icon("close"),
            );
        }
        let private = tab.private;
        entries.push(
            item(if private { "Private  ✓" } else { "Private" }, move |this, _, cx| {
                this.send(ClientMsg::SetPrivate { tab: tab_id, private: !private }, cx);
            })
            .icon("private"),
        );
        let held: Vec<PaneId> = tab.panes.iter().filter(|p| p.held.is_some()).map(|p| p.id).collect();
        if !held.is_empty() {
            entries.push(item("Start Now", move |this, _, cx| {
                for pane in &held {
                    this.send(ClientMsg::StartNow { pane: *pane }, cx);
                }
            }).icon("go"));
        }
        entries.push(more("Move to Bench", move |this: &mut Self, cx| this.move_menu(tab_id, cx)).icon("bench"));
        entries.push(MenuEntry::Separator);
        entries.push(danger("Close Tab…", move |this: &mut Self, window, cx| this.close_tab(tab_id, window, cx)).icon("close"));
        entries
    }

    /// Whether dropping `tab` at `at` would change anything.
    fn drop_changes(&self, tab: TabId, at: DropAt, cx: &Context<Self>) -> bool {
        let Some(bench) = self.bench(cx) else { return false };
        let now = groups::of(bench);
        match at {
            DropAt::Tab(t, _) => t != tab,
            DropAt::NewGroup(t) => groups::alone_before(&now, tab, Some(t), TabDivider::Space) != now,
            DropAt::NewRow(t) => groups::alone_before(&now, tab, Some(t), TabDivider::Row) != now,
        }
    }

    /// A tab dropped on the tab bar: next to another, into its group; or a
    /// group or a row of its own.
    fn drop_tab(&mut self, tab: TabId, at: DropAt, cx: &mut Context<Self>) {
        let (before, divider) = match at {
            DropAt::Tab(to, after) => {
                if to != tab {
                    self.send(ClientMsg::MoveTabNextTo { tab, to, after }, cx);
                }
                return;
            }
            DropAt::NewGroup(t) => (t, TabDivider::Space),
            DropAt::NewRow(t) => (t, TabDivider::Row),
        };
        self.arrange(|now| Some(groups::alone_before(now, tab, Some(before), divider)), Some(tab), cx);
    }

    /// Rearranges the bench's tabs at once, as `change` makes them from how
    /// they are now.
    fn arrange(&self, change: impl FnOnce(&[(TabId, TabDivider)]) -> Option<groups::Arrangement>, focus: Option<TabId>, cx: &Context<Self>) {
        let Some(bench) = self.bench(cx) else { return };
        let now = groups::of(bench);
        if let Some(tabs) = change(&now).filter(|tabs| *tabs != now) {
            self.send(ClientMsg::ArrangeTabs { bench: self.bench, tabs, focus }, cx);
        }
    }

    /// A group's menu (right-click between its tabs or on the line before it).
    fn group_menu(&self, group: usize, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some(bench) = self.bench(cx) else { return Vec::new() };
        let now = groups::of(bench);
        let all = groups::groups(&now);
        let Some(range) = all.get(group).cloned() else { return Vec::new() };
        let ids: Vec<TabId> = now[range.clone()].iter().map(|(t, _)| *t).collect();
        let (first, last) = (ids[0], ids[ids.len() - 1]);
        let mut entries = vec![item("New Tab in Group", move |this, _, cx| this.new_tab_beside(last, true, cx)).icon("tab-new")];
        if group > 0 {
            let row = now[range.start].1 == TabDivider::Row;
            entries.push(item(if row { "Start on a New Row  ✓" } else { "Start on a New Row" }, move |this, _, cx| {
                let divider = if row { TabDivider::Space } else { TabDivider::Row };
                this.send(ClientMsg::SetTabDivider { tab: first, divider }, cx);
            }));
            entries.push(item("Merge with Previous Group", move |this, _, cx| this.send(ClientMsg::SetTabDivider { tab: first, divider: TabDivider::None }, cx)));
        }
        // Moved by its first tab, the group being whichever it is in then.
        let moved = move |right: bool| {
            move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                this.arrange(|now| groups::move_group(now, groups::group_of(now, first)?, right), None, cx);
            }
        };
        if group > 0 {
            entries.push(item("Move Group Left", moved(false)));
        }
        if group + 1 < all.len() {
            entries.push(item("Move Group Right", moved(true)));
        }
        entries.push(MenuEntry::Separator);
        let n = ids.len();
        entries.push(
            danger(if n == 1 { "Close Group's Tab…".to_string() } else { format!("Close Group's {n} Tabs…") }, move |this: &mut Self, window, cx| {
                this.close_tabs(&ids, window, cx)
            })
            .icon("close"),
        );
        entries
    }

    /// A new terminal tab just before or after `tab`, in its group.
    fn new_tab_beside(&mut self, tab: TabId, after: bool, cx: &mut Context<Self>) {
        let pane = self.tab(tab, cx).filter(|t| !t.panes.is_empty()).map(|t| t.active_pane);
        let cwd = self.new_pane_cwd(pane, cx);
        self.send(ClientMsg::NewTabNextTo { tab, after, cwd }, cx);
    }

    /// Closes several tabs, after asking once (one tab: as Close Tab asks).
    fn close_tabs(&mut self, ids: &[TabId], window: &mut Window, cx: &mut Context<Self>) {
        let tabs: Vec<Tab> = ids.iter().filter_map(|t| self.tab(*t, cx).cloned()).collect();
        if let [tab] = tabs.as_slice() {
            let conn = self.state.read(cx).conn.clone();
            crate::state::close_tab(&conn, tab, BenchRef::Running(self.bench), window, cx);
            return;
        }
        if tabs.is_empty() {
            return;
        }
        let names: Vec<String> = tabs.iter().map(|t| format!("“{}”", t.display_title())).collect();
        let message = format!("Close the group's {} tabs?", tabs.len());
        let detail = format!(
            "{}.\n\nThe programs running in them stop and their teardown commands run; AI tabs keep their sessions, remote tabs' shells keep running on their servers.",
            names.join(", ")
        );
        let answer = window.prompt(PromptLevel::Warning, &message, Some(&detail), &["Close Tabs", "Cancel"], cx);
        let ids: Vec<TabId> = tabs.iter().map(|t| t.id).collect();
        let conn = self.state.read(cx).conn.clone();
        cx.spawn(async move |_, _| {
            if let Ok(0) = answer.await {
                for tab in ids {
                    conn.send_msg(ClientMsg::CloseTab { tab });
                }
            }
        })
        .detach();
    }

    /// The other groups, to move a tab into (each by its first tab's title),
    /// or a group of its own at the end.
    fn move_to_group_menu(&mut self, tab_id: TabId, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some(bench) = self.bench(cx) else { return Vec::new() };
        let now = groups::of(bench);
        let mine = groups::group_of(&now, tab_id);
        let all = groups::groups(&now);
        let mut entries: Vec<MenuEntry<Self>> = all
            .iter()
            .enumerate()
            .filter(|(g, _)| Some(*g) != mine)
            .map(|(_, range)| {
                let title = bench.tabs[range.start].display_title();
                let label = if range.len() > 1 { format!("{title}  +{}", range.len() - 1) } else { title };
                let last = now[range.end - 1].0;
                item(label, move |this, _, cx| this.send(ClientMsg::MoveTabNextTo { tab: tab_id, to: last, after: true }, cx))
            })
            .collect();
        let alone_last = groups::alone_before(&now, tab_id, None, TabDivider::Space) == now;
        if !alone_last {
            if !entries.is_empty() {
                entries.push(MenuEntry::Separator);
            }
            entries.push(
                item("New Group", move |this, _, cx| {
                    this.arrange(|now| Some(groups::alone_before(now, tab_id, None, TabDivider::Space)), Some(tab_id), cx);
                })
                .icon("plus"),
            );
        }
        entries
    }

    /// Every other bench, to move a tab to.
    fn move_menu(&mut self, tab_id: TabId, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let from = BenchRef::Running(self.bench);
        let destinations = self.state.read(cx).tab_destinations(&from, cx);
        destinations
            .into_iter()
            .map(|d| {
                let label = if d.closed { format!("{}  (closed)", d.name) } else { d.name.clone() };
                let from = from.clone();
                item(label, move |this: &mut Self, window, cx| {
                    let Some(title) = this.tab(tab_id, cx).map(|t| t.display_title()) else { return };
                    let conn = this.state.read(cx).conn.clone();
                    crate::state::move_tab(&conn, tab_id, &title, from.clone(), d.to.clone(), &d.name, usize::MAX, window, cx);
                })
                .icon("bench")
            })
            .collect()
    }

    fn pane_menu(&self, pane: PaneId, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let mut entries = Vec::new();
        if self.state.read(cx).tree.pane(pane).is_some_and(|p| p.held.is_some()) {
            entries.push(item("Start Now", move |this, _, cx| this.send(ClientMsg::StartNow { pane }, cx)).icon("go"));
            entries.push(MenuEntry::Separator);
        }
        entries.extend([
            item("Copy", |_, window, cx| window.dispatch_action(Box::new(Copy), cx)),
            item("Paste", |_, window, cx| window.dispatch_action(Box::new(Paste), cx)),
            MenuEntry::Separator,
            item("Split Right", move |this, _, cx| this.split_pane(pane, SplitDir::Right, cx)).icon("split-right"),
            item("Split Down", move |this, _, cx| this.split_pane(pane, SplitDir::Down, cx)).icon("split-down"),
            item("Open Folder as Project", move |this, _, cx| this.open_pane_project(pane, cx)).icon("project"),
            MenuEntry::Separator,
            danger("Close Pane", move |this: &mut Self, window, cx| this.close_pane(pane, window, cx)).icon("close"),
        ]);
        entries
    }

    /// Ctrl (Cmd) with the wheel zooms the shown tab, whatever is under the
    /// mouse: heard first (the capture phase), so it does not also scroll.
    fn ctrl_wheel_zoom(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let pending = self.wheel_zoom.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let (view, pending) = (view.clone(), pending.clone());
                window.on_mouse_event(move |e: &gpui::ScrollWheelEvent, phase, _, cx| {
                    if phase != gpui::DispatchPhase::Capture || !e.modifiers.secondary() {
                        return;
                    }
                    cx.stop_propagation();
                    // A notch is a step; a touchpad's pixels add up to one.
                    let step = match e.delta {
                        gpui::ScrollDelta::Lines(d) => d.y.signum(),
                        gpui::ScrollDelta::Pixels(d) => {
                            let total = pending.get() + f32::from(d.y);
                            if total.abs() < 40.0 {
                                pending.set(total);
                                return;
                            }
                            pending.set(0.0);
                            total.signum()
                        }
                    };
                    if step != 0.0 {
                        view.update(cx, |this, cx| this.zoom_tab(step, cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    /// Zooms the shown tab by `delta` points (0 goes back to the usual size).
    fn zoom_tab(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some(tab) = self.bench(cx).and_then(|b| b.active_tab().cloned()) else { return };
        let base = self.state.read(cx).settings.font.size;
        let zoom = if delta == 0.0 { 0.0 } else { (base + tab.zoom + delta).clamp(6.0, 72.0) - base };
        self.send(ClientMsg::SetZoom { tab: tab.id, zoom }, cx);
    }

    fn bench_menu(&self, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let current = self.bench(cx).map(|b| b.color);
        let mut entries = vec![
            item("New Project Tab…", |this, window, cx| this.new_project_tab(None, window, cx)).icon("project"),
            more("New AI Tab", |this: &mut Self, cx| this.ai_folder_menu(None, cx)).icon("ai"),
            more("New Remote Tab", |this: &mut Self, cx| this.remote_host_menu(None, cx)).icon("server"),
            MenuEntry::Separator,
            item("Rename Bench…", |this, window, cx| this.start_edit(EditTarget::BenchName, window, cx)).icon("pencil"),
            item("Set Badge…", |this, window, cx| this.start_edit(EditTarget::Badge, window, cx)),
            item("Add Link…", |this, window, cx| this.start_edit(EditTarget::AddLink, window, cx)).icon("link"),
            MenuEntry::Separator,
        ];
        entries.push(MenuEntry::Palette(
            BenchColor::NAMED
                .into_iter()
                .map(|color| {
                    menu::swatch_of(color.accent(), color.name().unwrap_or_default(), current == Some(color), move |this: &mut Self, _, cx| {
                        this.send(ClientMsg::SetBenchColor { bench: this.bench, color }, cx);
                    })
                })
                .collect(),
        ));
        entries.push(item("Custom Colour…", |this, window, cx| this.start_edit(EditTarget::Color, window, cx)).icon("bench-color"));
        // Light or dark, for all of Shed (as in Settings).
        entries.push(
            more("Appearance", |this: &mut Self, cx| {
                use shed_model::config::Appearance;
                let now = this.state.read(cx).settings.config.appearance();
                [(Appearance::Auto, "Auto: follow the desktop", "theme-auto"), (Appearance::Light, "Light", "theme-light"), (Appearance::Dark, "Dark", "theme-dark")]
                    .into_iter()
                    .map(|(a, label, glyph)| {
                        let label = if now == Some(a) { format!("{label}  ✓") } else { label.to_string() };
                        item(label, move |this, _, cx| this.state.update(cx, |state, cx| state.set_appearance(a, cx))).icon(glyph)
                    })
                    .collect()
            })
            .icon("theme-auto"),
        );
        entries.push(MenuEntry::Separator);
        if let Some(b) = self.bench(cx) {
            let has_startup = b.tabs.iter().flat_map(|t| &t.panes).any(|p| !p.setup.startup.is_empty());
            if has_startup {
                entries.push(item(if b.started { "Run Startup Again" } else { "Run Startup" }, |this, _, cx| this.send(ClientMsg::RunStartup { bench: this.bench }, cx)).icon("go"));
            }
            entries.push(item("Run Teardown", |this, _, cx| this.send(ClientMsg::RunTeardown { bench: this.bench }, cx)).icon("stop"));
        }
        entries.push(item("Show Shed", |this, _, cx| this.state.update(cx, |state, cx| state.show_shed(cx))).icon("shed"));
        entries.push(item("Hide Bench", |this, window, cx| this.hide(window, cx)).icon("detach"));
        entries.push(danger("Close Bench…", |this: &mut Self, window, cx| this.close_bench(window, cx)).icon("power"));
        entries
    }

}

/// A tab as the tab bar shows it.
#[derive(Clone, PartialEq)]
struct BarTab {
    id: TabId,
    title: String,
    /// Its icon and the icon's colour (`tab_icon`).
    icon: TabIcon,
    private: bool,
    zoom: f32,
    tags: Vec<Tag>,
    divider: TabDivider,
}

/// What the tab bar shows of `AppState`: its bench's tabs and controls.
#[derive(Clone, PartialEq)]
struct BarState {
    tabs: Vec<BarTab>,
    active: TabId,
    name: String,
    badge: Option<String>,
    accent: Rgb,
    links: Vec<ProjectLink>,
    closing: bool,
    started: bool,
    has_startup: bool,
    has_teardown: bool,
    /// The shown tab is a terminal tab (it has a Setup).
    terminal: bool,
    /// Claude's usage limits, on a bench with an AI tab, and the palette
    /// their bars are coloured from.
    limits: Option<(Limits, shed_emu::Palette)>,
    ui: UiColors,
    /// How large the bar is drawn (`[window] tab_bar_scale`).
    scale: f32,
    /// The terminal font's size, which a tab's zoom is relative to.
    base: f32,
}

impl BarState {
    fn of(bench: &Bench, state: &AppState, limits: Option<Limits>) -> Self {
        let ui = state.settings.ui.clone();
        let accent = bench.color.accent();
        let panes = || bench.tabs.iter().flat_map(|t| &t.panes);
        let tabs = bench
            .tabs
            .iter()
            .map(|t| BarTab {
                id: t.id,
                title: t.display_title(),
                icon: tab_icon(t, accent, &ui),
                private: t.private,
                zoom: t.zoom,
                tags: t.all_tags().cloned().collect(),
                divider: t.divider,
            })
            .collect();
        let ai = bench.tabs.iter().any(|t| t.ai_info().is_some());
        Self {
            tabs,
            active: bench.active_tab,
            name: bench.name.clone(),
            badge: bench.badge.clone(),
            accent,
            links: bench.links.clone(),
            closing: bench.closing,
            started: bench.started,
            has_startup: panes().any(|p| !p.setup.startup.is_empty()),
            has_teardown: panes().any(|p| !p.setup.teardown.is_empty()),
            terminal: bench.active_tab().is_some_and(|t| t.project_info().is_none() && t.ai_info().is_none()),
            limits: limits.filter(|_| ai).map(|l| (l, state.settings.palette.clone())),
            scale: state.settings.config.window.tab_bar_scale.clamp(0.7, 2.0) * crate::theme::ui_scale(),
            base: state.settings.font.size,
            ui,
        }
    }
}

/// What the tab bar draws: its bench, and the window's own state that shows
/// on it (a text field in it, where a dragged tab would go, the Setup panel).
#[derive(Clone, PartialEq)]
struct BarModel {
    bench: BarState,
    editing: Option<(EditTarget, Entity<LineEditor>)>,
    drop_hint: Option<DropAt>,
    setup_open: bool,
    bookmarks_open: bool,
}

impl BarModel {
    fn editor_for(&self, target: EditTarget) -> Option<Entity<LineEditor>> {
        self.editing.as_ref().filter(|(t, _)| *t == target).map(|(_, e)| e.clone())
    }
}

/// A bench window's tab bar, in a view of its own: a terminal's output, an
/// AI tab's ticks or a project's changes draw the window without building
/// the bar again, and the bar draws again only when what it shows changed.
/// Its listeners act on the `BenchWindow`.
pub struct TabBar {
    window: WeakEntity<BenchWindow>,
    model: Option<BarModel>,
    /// How many times it was drawn (tests check what a change draws).
    drawn: usize,
    /// Where its tabs were laid out last.
    layout: Rc<RefCell<BarLayout>>,
}

impl TabBar {
    /// A listener that runs on the window, as `cx.listener` would there.
    fn on<E: ?Sized>(&self, f: impl Fn(&mut BenchWindow, &E, &mut Window, &mut Context<BenchWindow>) + 'static) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = self.window.clone();
        move |event, window, cx| {
            view.update(cx, |this, cx| f(this, event, window, cx)).ok();
        }
    }

    fn render_bar(&self, m: &BarModel, width: Pixels, me: WeakEntity<TabBar>, cx: &App) -> impl IntoElement + use<> {
        let b = &m.bench;
        let s = b.scale;
        let ui = &b.ui;
        let accent = b.accent;
        let frame = crate::pulse::frame();
        // Tabs wrap onto more rows when they do not fit.
        let pad = px(4.0 * s);
        let pad_y = px(6.0 * s);
        let tab_h = px(TAB_HEIGHT * s);
        let row_gap = px(6.0 * s);
        // Where the tabs land is noted as they are laid out (each group's
        // tabs, by position), for drops, the group menu and the lines
        // between groups.
        let layout = self.layout.clone();
        // The tabs as wide as they were last (the controls are arranged by
        // that); drawn once more when they turn out otherwise (new tabs, a
        // new title), but not again after that.
        let (measured, may_recheck) = {
            let mut l = layout.borrow_mut();
            let measured: HashMap<TabId, f32> = l.tabs.iter().filter(|t| t.2.size.width > px(0.0)).map(|t| (t.0, f32::from(t.2.size.width))).collect();
            (measured, !std::mem::take(&mut l.recheck))
        };
        {
            let mut group = 0;
            layout.borrow_mut().tabs = b
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    if i > 0 && t.divider != TabDivider::None {
                        group += 1;
                    }
                    (t.id, group, Bounds::default())
                })
                .collect();
        }
        // Its rows centred when the controls beside them are taller. A
        // group starting a row of its own starts another run of rows.
        let mut tabs = div().relative().flex_1().min_w_0().flex().flex_col().justify_center().gap_y(row_gap).px(pad).py(pad_y);
        let run = || div().flex().flex_wrap().gap_x(px(GROUP_GAP * s)).gap_y(row_gap);
        let mut section = run();
        // Tabs in groups, split where a tab has a divider before it: a group
        // goes onto the next row whole, and wraps inside only when it alone
        // is wider than the bar. Groups are set apart by space and a faint
        // line.
        let group = |start: usize| {
            let (layout, me) = (layout.clone(), me.clone());
            div().flex().flex_wrap().min_w_0().gap_x(px(2.0 * s)).gap_y(row_gap).on_children_prepainted(move |bounds, _, cx| {
                let mut layout = layout.borrow_mut();
                let mut off = false;
                for (j, b) in bounds.into_iter().enumerate() {
                    off |= layout.assumed.get(start + j).is_some_and(|w| (w - f32::from(b.size.width)).abs() > 1.0);
                    if let Some(t) = layout.tabs.get_mut(start + j) {
                        t.2 = b;
                    }
                }
                if off && may_recheck && !layout.recheck {
                    layout.recheck = true;
                    let me = me.clone();
                    cx.defer(move |cx| {
                        me.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
            })
        };
        let mut current = group(0);
        let base = b.base;

        for (i, tab) in b.tabs.iter().enumerate() {
            if i > 0 && tab.divider != TabDivider::None {
                section = section.child(std::mem::replace(&mut current, group(i)));
                if tab.divider == TabDivider::Row {
                    tabs = tabs.child(std::mem::replace(&mut section, run()));
                }
            }
            let active = tab.id == b.active;
            let tab_id = tab.id;
            let title = tab.title.clone();
            let tint = tab.tags.iter().find(|t| t.tint_tab).map(|t| t.color);
            let mut el = div()
                .id(("tab", i))
                .group("tab")
                .flex()
                .items_center()
                .gap(px(6.0 * s))
                .h(tab_h)
                .px(px(TAB_PAD_X * s))
                .max_w(px(260.0 * s))
                .rounded_md()
                .cursor_pointer()
                .on_click(self.on(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_edit(EditTarget::TabName(tab_id), window, cx);
                    } else {
                        this.send(ClientMsg::FocusTab { tab: tab_id }, cx);
                    }
                }))
                .on_mouse_down(MouseButton::Middle, self.on(move |this, _: &MouseDownEvent, window, cx| {
                    this.close_tab(tab_id, window, cx);
                }))
                .on_mouse_down(MouseButton::Right, self.on(move |this, e: &MouseDownEvent, _, cx| {
                    // The tab's menu, not its group's.
                    cx.stop_propagation();
                    let entries = this.tab_menu(tab_id, cx);
                    this.open_menu(e.position, entries, cx);
                }))
                // Where it goes when dropped: the bar works that out.
                .on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()));
            // The shown tab: a soft fill of the bench's colour, stronger text.
            el = match (active, tint) {
                (true, Some(t)) => el.bg(hsla_alpha(t, 0.35)).text_color(hsla(ui.text)).font_weight(FontWeight::MEDIUM),
                (false, Some(t)) => el.bg(hsla_alpha(t, 0.16)).hover(|s| s.bg(hsla_alpha(t, 0.26))),
                (true, None) => el.bg(hsla_alpha(accent, 0.2)).text_color(hsla(ui.text)).font_weight(FontWeight::MEDIUM),
                (false, None) => el.hover(|s| s.bg(hsla_alpha(ui.text, 0.06)).text_color(hsla(ui.text))),
            };
            // One icon, as in Shed: what the tab is, or that it waits or works.
            let tab_icon = tab.icon;
            el = match tab_icon.working {
                true => el.child(crate::pulse::working_mark(frame, px(14.0 * s), tab_icon.color)),
                false => el.child(svg().path(icon(tab_icon.glyph)).size(px(14.0 * s)).flex_none().text_color(hsla(tab_icon.color))),
            };
            if tab.private {
                el = el.child(svg().path(icon("private")).size(px(12.0 * s)).flex_none().text_color(hsla(ui.text_dim)));
            }
            match m.editor_for(EditTarget::TabName(tab_id)) {
                Some(editor) => el = el.child(editor),
                None => el = el.child(div().truncate().child(title)),
            }
            // Work Claude started in the background, waited on: how much.
            if tab_icon.background > 0 {
                el = el.child(background_badge(("background", i), tab_icon.background, accent, ui, px(10.5 * s)));
            }
            // A zoomed tab says how much; a click puts it back.
            if tab.zoom != 0.0 {
                let pct = ((base + tab.zoom) / base * 100.0).round();
                el = el.child(
                    div()
                        .id(("zoom", i))
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(10.5 * s))
                        .bg(hsla_alpha(ui.text_dim, 0.18))
                        .hover(|st| st.bg(hsla_alpha(ui.text_dim, 0.3)))
                        .tooltip(menu::tooltip(if cfg!(target_os = "macos") { "Zoom: click for the usual size (Cmd+0)" } else { "Zoom: click for the usual size (Ctrl+0)" }))
                        .child(format!("{pct}%"))
                        .on_click(self.on(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.send(ClientMsg::SetZoom { tab: tab_id, zoom: 0.0 }, cx);
                        })),
                );
            }
            // Tags as small text in their colours.
            for tag in &tab.tags {
                el = el.child(
                    div()
                        .flex_none()
                        .text_size(px(11.0 * s))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(hsla(ui.ink(tag.color)))
                        .child(tag.label.clone()),
                );
            }
            if let Some(editor) = m.editor_for(EditTarget::AddTag(tab_id)) {
                el = el.child(editor);
            }
            current = current.child(el);
        }

        // The new-tab button leads the controls on the right.
        let new_tab = div()
            .id("new-tab")
            .rounded_md()
            .flex()
            .items_center()
            .px_2()
            .cursor_pointer()
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .tooltip(menu::tooltip(if cfg!(target_os = "macos") {
                "New terminal tab (Cmd+T); right-click for a project, AI or remote tab"
            } else {
                "New terminal tab (Ctrl+Shift+T); right-click for a project, AI or remote tab"
            }))
            .on_click(self.on(|this, _: &ClickEvent, _, cx| this.new_tab(cx)))
            .on_mouse_down(MouseButton::Right, self.on(|this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let entries = BenchWindow::new_tab_menu(None);
                this.open_menu(e.position, entries, cx);
            }))
            .child(svg().path(icon("tab-new")).size(px(16.0 * s)).text_color(hsla(ui.text_dim)));
        // The bench's bookmarks, in a list under it.
        let marks_open = m.bookmarks_open;
        let marks_at = layout.clone();
        let bookmarks = div()
            .id("bookmarks-button")
            .relative()
            .rounded_md()
            .flex()
            .items_center()
            .px_2()
            .cursor_pointer()
            .when(marks_open, |d| d.bg(hsla_alpha(accent, 0.2)))
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .tooltip(menu::tooltip(if cfg!(target_os = "macos") { "Bookmarks (Cmd+Shift+M)" } else { "Bookmarks (Ctrl+Shift+M)" }))
            .on_click(self.on(|this, _: &ClickEvent, _, cx| this.toggle_bookmarks(cx)))
            .child(svg().path(icon("bookmark")).size(px(16.0 * s)).text_color(hsla(if marks_open { ui.ink(accent) } else { ui.text_dim })))
            .child(canvas(move |bounds, _, _| marks_at.borrow_mut().bookmarks_button = Some(bounds), |_, _, _, _| {}).absolute().top_0().left_0().size_full());
        // Drawn over the tabs once they are laid out: the faint lines between
        // groups on a row, and while a tab is dragged, where it would go.
        let hint = m.drop_hint.filter(|_| cx.has_active_drag());
        let (line, caret) = (hsla_alpha(ui.text, 0.16), hsla(ui.ink(accent)));
        let marks = {
            let layout = layout.clone();
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let layout = layout.borrow();
                    for b in layout.separators(s) {
                        window.paint_quad(gpui::PaintQuad::new(b).background(line));
                    }
                    for b in hint.map(|h| layout.caret(h, bounds.left(), s)).unwrap_or_default() {
                        window.paint_quad(gpui::PaintQuad::new(b).corner_radii(px(1.0 * s)).background(caret));
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        // A tab dragged over the bar goes where the pointer says (`drop_at`);
        // right-clicking between tabs is for their group.
        let (on_move, on_drop, on_group) = (layout.clone(), layout.clone(), layout.clone());
        let tabs = tabs
            .child(section.child(current))
            .child(marks)
            .on_drag_move::<DraggedTab>(self.on(move |this, e: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                let at = e.event.position;
                let hint = if e.bounds.contains(&at) { on_move.borrow().drop_at(at, s) } else { None };
                let hint = hint.filter(|h| this.drop_changes(e.drag(cx).tab, *h, cx));
                if this.drop_hint != hint {
                    this.drop_hint = hint;
                    this.redraw(cx);
                }
            }))
            .on_drop(self.on(move |this, d: &DraggedTab, window, cx| {
                // Where the last move said, or (no move came) where it is now.
                let at = this.drop_hint.take().or_else(|| on_drop.borrow().drop_at(window.mouse_position(), s));
                if let Some(at) = at {
                    this.drop_tab(d.tab, at, cx);
                }
                this.redraw(cx);
            }))
            .on_mouse_down(MouseButton::Right, self.on(move |this, e: &MouseDownEvent, _, cx| {
                let Some(group) = on_group.borrow().group_at(e.position) else { return };
                let entries = this.group_menu(group, cx);
                this.open_menu(e.position, entries, cx);
            }));
        // Startup and teardown on demand: a bench opens quietly, and runs its
        // teardown when closed only if its startup ran.
        let (has_startup, has_teardown) = (b.has_startup, b.has_teardown);
        // A small icon like the bar's others, in the bench's colour; what it
        // does in its tooltip.
        let run = |id: &'static str, glyph: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .px_2()
                .rounded_md()
                .cursor_pointer()
                .hover(|st| st.bg(hsla(ui.tab_hover)))
                .tooltip(menu::tooltip(tip))
                .child(svg().path(icon(glyph)).size(px(16.0 * s)).text_color(hsla(ui.ink(accent))))
        };
        let lifecycle_shown = !b.closing && (has_startup && !b.started || b.started && has_teardown);
        let lifecycle = if b.closing {
            None
        } else if has_startup && !b.started {
            Some(run("run-startup", "go", "Run Startup: each pane's startup now; closing then runs the teardown").on_click(self.on(|this, _: &ClickEvent, _, cx| {
                this.send(ClientMsg::RunStartup { bench: this.bench }, cx);
            })))
        } else if b.started && has_teardown {
            Some(run("run-teardown", "stop", "Run Teardown: stop what runs and run each pane's teardown; the bench stays open").on_click(self.on(|this, _: &ClickEvent, _, cx| {
                this.send(ClientMsg::RunTeardown { bench: this.bench }, cx);
            })))
        } else {
            None
        };
        // Claude's usage limits, on benches that talk to it.
        let limits = b.limits.as_ref().and_then(|(l, palette)| crate::ai_view::limit_bars(*l, ui, palette, s));
        // Setup is for terminal tabs: startup, teardown and aliases.
        let terminal = b.terminal;
        let setup_open = m.setup_open;
        let setup = (terminal || setup_open).then(|| {
            div()
                .id("setup-button")
                .flex()
                .items_center()
                .px_2()
                .rounded_md()
                .cursor_pointer()
                .when(setup_open, |d| d.bg(hsla_alpha(accent, 0.2)))
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .tooltip(menu::tooltip(if cfg!(target_os = "macos") { "Setup: startup, teardown, aliases (Cmd+Shift+S)" } else { "Setup: startup, teardown, aliases (Ctrl+Shift+S)" }))
                .on_click(self.on(|this, _: &ClickEvent, window, cx| this.toggle_setup(window, cx)))
                .child(svg().path(icon("rack")).size(px(16.0 * s)).text_color(hsla(if setup_open { ui.ink(accent) } else { ui.text_dim })))
        });
        // The controls on the right, on a panel of their own, in rows: New
        // Tab, Bookmarks, Run Startup, Setup and the bench's name; the usage limits; its
        // links. Beside one row of tabs, all on one row in that order; beside
        // two, two (the actions over the limits and links); else three,
        // narrower, leaving the tabs the width. Each row is centred in the
        // panel. The tabs' widths are as laid out last (`BarLayout::assumed`).
        let ctl_h = px(24.0 * s);
        let actions: Vec<AnyElement> = [new_tab.h(ctl_h).into_any_element(), bookmarks.h(ctl_h).into_any_element()]
            .into_iter()
            .chain(lifecycle.map(|l| l.h(ctl_h).into_any_element()))
            .chain(setup.map(|d| d.h(ctl_h).into_any_element()))
            .chain(std::iter::once(div().h(ctl_h).flex().items_center().child(self.render_bench_label(m)).into_any_element()))
            .collect();
        let links: Vec<AnyElement> = self.render_links(b).into_iter().map(|l| div().h(ctl_h).flex().items_center().child(l).into_any_element()).collect();
        let limits = limits.map(|l| l.h(tab_h).px_1().into_any_element());
        let w = bar_widths(b, s, limits.is_some(), lifecycle_shown, terminal || setup_open, &measured);
        layout.borrow_mut().assumed = w.groups.iter().flat_map(|g| g.1.iter().copied()).collect();
        let gap = 4.0 * s;
        let (l, a, k) = (w.limits, row_width(&w.actions, gap), row_width(&w.links, gap));
        let room = f32::from(width);
        // The panel's padding (left and right) and margin.
        let panel_px = 10.0 * s;
        let frame = 2.0 * panel_px + f32::from(pad) + 2.0;
        let one = row_width(&[a, l, k].into_iter().filter(|w| *w > 0.0).collect::<Vec<_>>(), gap);
        let cap = |share: f32| l.max(a).max(room * share);
        let two = a.max(row_width(&[l, k].into_iter().filter(|w| *w > 0.0).collect::<Vec<_>>(), gap).min(cap(0.3)));
        let three = l.max(a).max(k.min(cap(0.25)));
        let rows = if w.tab_rows(room - one - frame, s) <= 1 {
            1
        } else if w.tab_rows(room - two - frame, s) <= 2 {
            2
        } else {
            3
        };
        let line = move || div().flex().flex_wrap().justify_center().items_center().gap_x(px(gap)).gap_y(px(4.0 * s));
        // As tall as the tab rows (the bar's padding above and below), its
        // rows centred.
        let mut panel = div()
            .flex_none()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap_y(px(4.0 * s))
            .mr(pad)
            .my(pad_y)
            .px(px(panel_px))
            .py(px(2.0 * s))
            .rounded_md()
            .bg(hsla_alpha(ui.text, 0.05));
        let max_w = |w: f32| px(w.ceil() + 2.0 + 2.0 * panel_px);
        panel = match rows {
            1 => panel.child(line().flex_nowrap().children(actions).children(limits).children(links)),
            2 => panel.max_w(max_w(two)).child(line().children(actions)).when(l > 0.0 || k > 0.0, |p| p.child(line().children(limits).children(links))),
            _ => panel
                .max_w(max_w(three))
                .child(line().children(actions))
                .children(limits.map(|l| line().child(l)))
                .when(k > 0.0, |p| p.child(line().children(links))),
        };
        div()
            .id("tab-bar")
            .flex()
            .flex_none()
            .items_stretch()
            // Set apart from the panes: a ground of the bench's colour, a line under it.
            .bg(hsla_alpha(accent, 0.09))
            .border_b_1()
            .border_color(hsla_alpha(accent, 0.35))
            .text_size(px(13.5 * s))
            .text_color(hsla(ui.text_dim))
            .child(tabs)
            .child(panel)
    }

    /// The bench's links, on the tab bar: click opens, right-click for more.
    fn render_links(&self, b: &BarState) -> Vec<AnyElement> {
        let accent = b.accent;
        let links = b.links.clone();
        b.links.iter().enumerate().map(|(i, link)| {
            let url = link.url.clone();
            let links = links.clone();
            crate::links::chip(format!("link-{i}"), link, accent, &b.ui)
                .tooltip(menu::tooltip("Open the link (right-click for more)"))
                .on_click(self.on(move |_, _: &ClickEvent, _, cx| crate::links::open(url.clone(), false, cx)))
                .on_mouse_down(MouseButton::Right, self.on(move |this, e: &MouseDownEvent, _, cx| {
                    let entries = crate::links::menu(
                        links.clone(),
                        i,
                        |this: &mut BenchWindow, links, cx| this.send(ClientMsg::SetBenchLinks { bench: this.bench, links }, cx),
                        move |this: &mut BenchWindow, window, cx| this.start_edit(EditTarget::RenameLink(i), window, cx),
                        move |this: &mut BenchWindow, window, cx| this.start_edit(EditTarget::EditLink(i), window, cx),
                    );
                    this.open_menu(e.position, entries, cx);
                }))
                .into_any_element()
        })
        .collect()
    }

    fn render_bench_label(&self, m: &BarModel) -> AnyElement {
        let b = &m.bench;
        let s = b.scale;
        let in_label = |t: &EditTarget| !matches!(t, EditTarget::TabName(_) | EditTarget::AddTag(_));
        let editor = m.editing.as_ref().filter(|(t, _)| in_label(t)).map(|(_, e)| e.clone());
        if let Some(editor) = editor {
            return div().flex().items_center().px_2().child(editor).into_any_element();
        }
        let accent = b.accent;
        div()
            .id("bench-label")
            .flex()
            .items_center()
            .gap_1p5()
            .h_full()
            .px_3()
            .rounded_md()
            .cursor_pointer()
            .text_color(hsla(b.ui.ink(accent)))
            .hover(|s| s.bg(hsla(b.ui.tab_hover)))
            .tooltip(menu::tooltip("The bench's menu: rename, colour, links, hide, close…"))
            .on_mouse_down(MouseButton::Left, self.on(|this, e: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu(cx);
                this.open_menu(e.position, entries, cx);
            }))
            .on_mouse_down(MouseButton::Right, self.on(|this, e: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu(cx);
                this.open_menu(e.position, entries, cx);
            }))
            .child(svg().path(icon("bench")).size(px(14.0 * s)).text_color(hsla(b.ui.ink(accent))))
            .child(b.name.clone())
            .when_some(b.badge.clone(), |el, badge| {
                el.child(div().px_1().rounded_sm().bg(hsla_alpha(accent, 0.25)).child(badge))
            })
            .into_any_element()
    }
}

impl Render for TabBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drawn += 1;
        let started = std::time::Instant::now();
        let width = window.viewport_size().width;
        let element = match &self.model {
            Some(m) => self.render_bar(m, width, cx.entity().downgrade(), cx).into_any_element(),
            None => div().into_any_element(),
        };
        if crate::terminal_view::perf::enabled() {
            use std::sync::atomic::Ordering;
            crate::terminal_view::perf::BAR_RENDERS.fetch_add(1, Ordering::Relaxed);
            crate::terminal_view::perf::BAR_US.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        }
        element
    }
}

impl BenchWindow {

    /// A terminal tab's panes, laid out by its split tree (`node` is the
    /// part of `panes.layout` at `path`).
    fn render_node(&self, node: &PaneNode, path: Vec<bool>, panes: &Panes, accent: Rgb, cx: &mut Context<Self>) -> AnyElement {
        match node {
            PaneNode::Leaf(id) => {
                let pane = *id;
                let split = panes.views.len() > 1;
                let active = pane == panes.active;
                let mut cell = div()
                    .relative()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        let entries = this.pane_menu(pane, cx);
                        this.open_menu(e.position, entries, cx);
                    }));
                if let Some((_, Some(view))) = panes.views.iter().find(|(id, _)| *id == pane) {
                    cell = cell.child(view.clone());
                }
                if split && active {
                    // Four thin strips over the pane: a bordered box the size of the
                    // pane would run the quad shader over every pixel of it.
                    let edge = || div().absolute().bg(hsla(accent));
                    cell = cell
                        .child(edge().top_0().left_0().w_full().h(px(1.0)))
                        .child(edge().bottom_0().left_0().w_full().h(px(1.0)))
                        .child(edge().top_0().left_0().h_full().w(px(1.0)))
                        .child(edge().top_0().right_0().h_full().w(px(1.0)));
                }
                cell.into_any_element()
            }
            PaneNode::Split { dir, ratio, first, second } => {
                let row = *dir == SplitDir::Right;
                let dir = *dir;
                let (mut first_path, mut second_path) = (path.clone(), path.clone());
                first_path.push(false);
                second_path.push(true);
                let first_el = div()
                    .flex_none()
                    .min_w_0()
                    .min_h_0()
                    .map(|d| if row { d.h_full().w(relative(*ratio)) } else { d.w_full().h(relative(*ratio)) })
                    .child(self.render_node(first, first_path, panes, accent, cx));
                let (tab_id, drag_layout, drag_path) = (panes.tab, panes.layout.clone(), path.clone());
                // A thin line inside a wider grab area.
                let divider = div()
                    .id(SharedString::from(format!("divider-{path:?}")))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .map(|d| if row { d.w(px(5.0)).h_full().flex_row().cursor_col_resize() } else { d.h(px(5.0)).w_full().flex_col().cursor_row_resize() })
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.drag = Some(SplitDrag { tab: tab_id, path: drag_path.clone(), dir, layout: drag_layout.clone() });
                        cx.stop_propagation();
                    }))
                    .child(div().bg(hsla(self.shown.ui.divider)).map(|d| if row { d.w(px(1.0)).h_full() } else { d.h(px(1.0)).w_full() }));
                let second_el = div().flex_1().min_w_0().min_h_0().child(self.render_node(second, second_path, panes, accent, cx));
                let bounds = self.split_bounds.clone();
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .map(|d| if row { d.flex_row() } else { d.flex_col() })
                    .child(
                        canvas(move |b, _, _| { bounds.borrow_mut().insert(path.clone(), b); }, |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(first_el)
                    .child(divider)
                    .child(second_el)
                    .into_any_element()
            }
        }
    }

    fn on_drag_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let Some(b) = self.split_bounds.borrow().get(&drag.path).copied() else { return };
        let ratio = match drag.dir {
            SplitDir::Right => f32::from(event.position.x - b.origin.x) / f32::from(b.size.width),
            SplitDir::Down => f32::from(event.position.y - b.origin.y) / f32::from(b.size.height),
        };
        if drag.layout.set_ratio(&drag.path, ratio) {
            let msg = ClientMsg::SetLayout { tab: drag.tab, layout: drag.layout.clone() };
            self.send(msg, cx);
        }
    }
}

#[path = "bookmarks.rs"]
mod bookmarks;

impl MenuHost for BenchWindow {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.menu
    }

    fn menu_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remote_menu = None;
        // A choice may have opened a text field, which keeps the keyboard.
        if self.editing.is_none() {
            self.refocus(window, cx);
        }
    }
}

impl Render for BenchWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drawn += 1;
        let perf = crate::terminal_view::perf::enabled();
        if perf {
            crate::terminal_view::perf::bench_frame_started();
        }
        // Drawn from what the window keeps, never from `AppState`: reading
        // it here would draw the window again at every change to it.
        let ui = self.shown.ui.clone();
        let errors = self.shown.errors.clone();
        let Some((accent, content)) = self.shown.bench.clone() else {
            return div().size_full().bg(hsla(ui.background)).into_any_element();
        };
        let content = match content {
            Content::Project(Some(view)) => div().size_full().child(view).into_any_element(),
            Content::Ai(Some(view)) => div().size_full().child(view).into_any_element(),
            Content::Panes(panes) => self.render_node(&panes.layout, Vec::new(), &panes, accent, cx),
            Content::Project(None) | Content::Ai(None) | Content::None => div().into_any_element(),
        };
        let menu = self.menu.as_ref().map(|m| menu::render(m, &ui, cx));
        let bookmarks = self.bookmarks_open.then(|| self.render_bookmarks(&ui, accent, cx));

        div()
            .key_context("BenchWindow")
            .size_full()
            .flex()
            .flex_col()
            // The terminal background, drawn first and full-window, so the renderer
            // turns it into the frame's clear colour.
            .bg(hsla(self.shown.background))
            .child(self.ctrl_wheel_zoom(cx))
            .capture_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                if this.menu.is_some() && e.keystroke.key == "escape" {
                    this.menu = None;
                    this.refocus(window, cx);
                    cx.stop_propagation();
                } else if this.bookmarks_open && e.keystroke.key == "escape" {
                    this.close_bookmarks(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(cx.listener(Self::on_drag_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
            .on_action(cx.listener(|this, _: &NewTab, _, cx| this.new_tab(cx)))
            // Terminals zoom themselves; project and AI tabs zoom here.
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom_tab(1.0, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom_tab(-1.0, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.zoom_tab(0.0, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(tab) = this.active_tab(cx).map(|t| t.id) {
                    this.close_tab(tab, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ClosePane, window, cx| {
                // In a tab without panes, closing "the pane" closes the tab.
                match this.active_tab(cx).map(|t| (t.id, t.active_pane, t.panes.is_empty())) {
                    Some((tab, _, true)) => this.close_tab(tab, window, cx),
                    Some((_, pane, false)) => this.close_pane(pane, window, cx),
                    None => {}
                }
            }))
            .on_action(cx.listener(|this, _: &NewProjectTab, window, cx| this.new_project_tab(None, window, cx)))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.cycle_tab(1, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, _, cx| this.cycle_tab(-1, cx)))
            .on_action(cx.listener(|this, a: &ActivateTab, _, cx| {
                if let Some(tab) = this.bench(cx).and_then(|b| b.tabs.get(a.0)).map(|t| t.id) {
                    this.send(ClientMsg::FocusTab { tab }, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SplitRight, _, cx| this.split(SplitDir::Right, cx)))
            .on_action(cx.listener(|this, _: &SplitDown, _, cx| this.split(SplitDir::Down, cx)))
            .on_action(cx.listener(|this, _: &FocusLeft, _, cx| this.move_focus(Direction::Left, cx)))
            .on_action(cx.listener(|this, _: &FocusRight, _, cx| this.move_focus(Direction::Right, cx)))
            .on_action(cx.listener(|this, _: &FocusUp, _, cx| this.move_focus(Direction::Up, cx)))
            .on_action(cx.listener(|this, _: &FocusDown, _, cx| this.move_focus(Direction::Down, cx)))
            .on_action(cx.listener(|this, _: &CloseBench, window, cx| this.close_bench(window, cx)))
            .on_action(cx.listener(|this, _: &HideBench, window, cx| this.hide(window, cx)))
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                if let Some(tab) = this.active_tab(cx).map(|t| t.id) {
                    this.start_edit(EditTarget::TabName(tab), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RenameBench, window, cx| this.start_edit(EditTarget::BenchName, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSetup, window, cx| this.toggle_setup(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleBookmarks, _, cx| this.toggle_bookmarks(cx)))
            .on_action(cx.listener(|this, _: &ShowShed, _, cx| this.state.update(cx, |state, cx| state.show_shed(cx))))
            .on_action(cx.listener(|this, a: &FocusBench, _, cx| {
                if let Some(handle) = this.state.read(cx).window_at(a.0, cx) {
                    handle.update(cx, |_, window, _| window.activate()).ok();
                }
            }))
            .child(div().h(px(3.0)).w_full().flex_none().bg(hsla(accent)))
            .child(self.bar.clone())
            .child(
                // The panel lies over the panes rather than beside them, so opening it
                // does not resize every terminal in the tab.
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(content)
                    .when_some(self.setup.as_ref().map(|(p, _)| p.clone()), |el, panel| {
                        el.child(div().absolute().top_0().right_0().h_full().child(panel))
                    })
                    // What the field in the tab bar is for, and how to finish.
                    .when_some(self.editing.as_ref().map(|(t, _, _)| t.describe()), |el, what| {
                        el.child(
                            div()
                                .absolute()
                                .top(px(4.0))
                                .left(px(8.0))
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_size(px(12.0))
                                .bg(hsla(ui.tab_active))
                                .border_1()
                                .border_color(hsla(ui.accent))
                                .text_color(hsla(ui.text))
                                .child(format!("{what} · Enter saves · Esc cancels · clicking elsewhere saves")),
                        )
                    }),
            )
            .when(!errors.is_empty(), |el| {
                el.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .bg(hsla_alpha(ui.danger, 0.2))
                        .text_size(px(12.0))
                        .text_color(hsla(ui.text))
                        .child(format!("shed.toml: {}", errors.join("; "))),
                )
            })
            .children(bookmarks)
            .children(menu)
            // Painted last: how long the frame took (`SHED_PERF`).
            .when(perf, |d| d.child(canvas(|_, _, _| {}, |_, _, _, _| crate::terminal_view::perf::bench_frame_painted()).absolute().size_0()))
            .into_any_element()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use shed_client::keeper_conn::KeeperConn;

    fn bench(name: &str, titles: &[&str]) -> Bench {
        let tabs: Vec<Tab> = titles
            .iter()
            .map(|title| {
                let mut pane = shed_model::Pane::new(PaneId::new(), "/tmp".into());
                pane.title = Some(title.to_string());
                Tab::with_pane(pane)
            })
            .collect();
        Bench {
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

    /// A change to one bench draws that bench's window only (and of it, the
    /// tab bar only when the bar shows the change); a change no window shows
    /// (a terminal's size, a title under the shown one) draws none.
    #[gpui::test]
    fn a_change_draws_only_its_bench_window(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let state = cx.new(|_| AppState::new(KeeperConn::detached(), crate::settings::Settings::default()));
        let (api, web) = (bench("api", &["~/api", "cargo test"]), bench("web", &["~/web"]));
        let (api_id, web_id) = (api.id, web.id);
        state.update(cx, |s, cx| {
            s.tree = shed_model::Shed { benches: vec![api, web], ..Default::default() };
            cx.notify();
        });
        let open = |id: BenchId, cx: &mut gpui::TestAppContext| {
            let tracker = PlaceTracker::new(Some(id), KeeperConn::detached(), gpui::WindowBounds::Windowed(gpui::Bounds::default()), None);
            let st = state.clone();
            cx.add_window(|window, cx| BenchWindow::new(id, st, tracker, window, cx))
        };
        let (a, w) = (open(api_id, cx), open(web_id, cx));
        cx.run_until_parked();
        // Each window's draws, and its tab bar's.
        let drawn = |cx: &mut gpui::TestAppContext| {
            [a, w].map(|h| h.update(cx, |v, _, cx| (v.drawn, v.bar.read(cx).drawn)).unwrap())
        };
        let change = |cx: &mut gpui::TestAppContext, f: &dyn Fn(&mut shed_model::Shed)| {
            state.update(cx, |s, cx| {
                f(&mut s.tree);
                cx.notify();
            });
            cx.run_until_parked();
        };
        let [(a0, ab0), (w0, wb0)] = drawn(cx);
        assert!(a0 > 0 && ab0 > 0 && w0 > 0 && wb0 > 0, "both drawn at first");

        // A terminal's size, and the title of a pane that is not the tab's
        // shown one: in no window.
        change(cx, &|tree| {
            let tab = &mut tree.benches[0].tabs[1];
            tab.panes[0].cols = 132;
            let mut hidden = shed_model::Pane::new(PaneId::new(), "/tmp".into());
            hidden.title = Some("htop".into());
            tab.panes.push(hidden);
        });
        let before = drawn(cx);
        change(cx, &|tree| tree.benches[0].tabs[1].panes[1].title = Some("top".into()));
        assert_eq!(drawn(cx), before, "nothing shown changed");

        // A title api's tab bar shows: api's window and bar, not web's.
        let [(a1, ab1), (w1, wb1)] = drawn(cx);
        change(cx, &|tree| tree.benches[0].tabs[1].panes[0].title = Some("cargo build".into()));
        let [(a2, ab2), (w2, wb2)] = drawn(cx);
        assert!(a2 > a1 && ab2 > ab1, "api's window drew its bar again");
        assert_eq!((w2, wb2), (w1, wb1), "web's window drew nothing");

        // Web renamed: web's window only.
        change(cx, &|tree| tree.benches[1].name = "website".into());
        let [(a3, ab3), (w3, wb3)] = drawn(cx);
        assert_eq!((a3, ab3), (a2, ab2));
        assert!(w3 > w2 && wb3 > wb2);
    }

    /// An AI tab at work turns its mark with the app's beat: the tab bar is
    /// drawn again a few times a second, the other bench's window not at
    /// all; once Claude is done, nothing is drawn by itself any more.
    #[gpui::test]
    fn a_working_ai_tab_animates_its_tab_bar_alone(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let state = cx.new(|_| AppState::new(KeeperConn::detached(), crate::settings::Settings::default()));
        let (mut api, web) = (bench("api", &["~/api"]), bench("web", &["~/web"]));
        let ai = Tab::ai("/tmp".into());
        let ai_id = ai.id;
        api.tabs.push(ai);
        let (api_id, web_id) = (api.id, web.id);
        state.update(cx, |s, cx| {
            s.tree = shed_model::Shed { benches: vec![api, web], ..Default::default() };
            cx.notify();
        });
        let open = |id: BenchId, cx: &mut gpui::TestAppContext| {
            let tracker = PlaceTracker::new(Some(id), KeeperConn::detached(), gpui::WindowBounds::Windowed(gpui::Bounds::default()), None);
            let st = state.clone();
            cx.add_window(|window, cx| BenchWindow::new(id, st, tracker, window, cx))
        };
        let (a, w) = (open(api_id, cx), open(web_id, cx));
        cx.run_until_parked();
        let drawn = |cx: &mut gpui::TestAppContext| [a, w].map(|h| h.update(cx, |v, _, cx| v.bar.read(cx).drawn).unwrap());
        let set = |activity, cx: &mut gpui::TestAppContext| {
            state.update(cx, |s, cx| {
                if let Some(shed_model::TabKind::Ai(ai)) = s.tree.tab_mut(ai_id).map(|t| &mut t.kind) {
                    ai.activity = activity;
                }
                cx.notify();
            });
            cx.run_until_parked();
        };
        let beats = |n: u32, cx: &mut gpui::TestAppContext| {
            for _ in 0..n {
                cx.executor().advance_clock(crate::pulse::STEP);
                cx.run_until_parked();
            }
        };
        // Idle: no beat, nothing drawn.
        let before = drawn(cx);
        beats(4, cx);
        assert_eq!(drawn(cx), before);
        assert!(!cx.update(|cx| crate::pulse::running(cx)));

        set(shed_model::AiActivity::Working, cx);
        let [a0, w0] = drawn(cx);
        beats(4, cx);
        let [a1, w1] = drawn(cx);
        assert!(a1 >= a0 + 4, "api's bar drawn at each beat ({a0} → {a1})");
        assert_eq!(w1, w0, "web's window is left alone");

        // Waiting on background work: an hourglass, still.
        set(shed_model::AiActivity::Background(2), cx);
        assert!(!cx.update(|cx| crate::pulse::running(cx)), "the beat stops with nothing to animate");
        let still = drawn(cx);
        beats(4, cx);
        assert_eq!(drawn(cx), still);
    }

    /// A tab's New Tab to the Left and Right offer every kind of tab the New
    /// Tab button's menu does; one chosen there opens next to that tab.
    #[gpui::test]
    fn new_tabs_of_every_kind_open_beside_a_tab(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let (tx, sent) = std::sync::mpsc::channel();
        let conn = KeeperConn::with_closer(tx, None, std::sync::Arc::new(|| {}));
        let state = cx.new(|_| AppState::new(conn, crate::settings::Settings::default()));
        let api = bench("api", &["~/api", "~/api/src"]);
        let (api_id, tab) = (api.id, api.tabs[0].id);
        state.update(cx, |s, cx| {
            s.tree = shed_model::Shed { benches: vec![api], ..Default::default() };
            cx.notify();
        });
        let tracker = PlaceTracker::new(Some(api_id), KeeperConn::detached(), gpui::WindowBounds::Windowed(gpui::Bounds::default()), None);
        let st = state.clone();
        let w = cx.add_window(|window, cx| BenchWindow::new(api_id, st, tracker, window, cx));
        cx.run_until_parked();
        let labels = |entries: &[MenuEntry<BenchWindow>]| -> Vec<String> {
            entries
                .iter()
                .filter_map(|e| match e {
                    MenuEntry::Item { label, .. } | MenuEntry::More { label, .. } => Some(label.to_string()),
                    _ => None,
                })
                .collect()
        };
        let kinds = labels(&BenchWindow::new_tab_menu(None));
        assert_eq!(kinds, ["New Terminal Tab", "New Project Tab…", "New AI Tab", "New Remote Tab"]);
        w.update(cx, |v, window, cx| {
            let menu = v.tab_menu(tab, cx);
            let sub = |label: &str| {
                menu.iter()
                    .find_map(|e| match e {
                        MenuEntry::More { label: l, entries, .. } if l.as_ref() == label => Some(entries.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("no submenu {label}"))
            };
            let (left, right) = (sub("New Tab to the Left")(v, cx), sub("New Tab to the Right")(v, cx));
            assert_eq!(labels(&left), kinds);
            assert_eq!(labels(&right), kinds);
            // An AI tab in the bench's folder, to the right of the tab.
            let MenuEntry::More { entries: folders, .. } = &right[2] else { panic!("New AI Tab is a submenu") };
            let folders = folders(v, cx);
            let MenuEntry::Item { action, .. } = &folders[0] else { panic!("a folder") };
            action(v, window, cx);
        })
        .unwrap();
        let msg = sent.try_iter().find_map(|f| match f {
            shed_proto::Frame::Client(m @ ClientMsg::NewTabNextToOfKind { .. }) => Some(m),
            _ => None,
        });
        assert_eq!(msg, Some(ClientMsg::NewTabNextToOfKind { tab, after: true, kind: NewTabKind::Ai { folder: "/".into() } }));
    }

    /// The bookmarks list opens under its button, says why a bookmark
    /// cannot be gone to (staying open), and closes again.
    #[gpui::test]
    fn the_bookmarks_list_opens_and_says_what_it_cannot_open(cx: &mut gpui::TestAppContext) {
        use shed_model::{Bookmark, MessageRole};
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let state = cx.new(|_| AppState::new(KeeperConn::detached(), crate::settings::Settings::default()));
        let mut b = bench("api", &["~/api"]);
        let terminal = b.tabs[0].id;
        let (gone, chat) = (
            Bookmark::file("/no/such/folder/main.rs".into(), Some(3)),
            Bookmark::message(terminal, None, None, MessageRole::User, "Fix it"),
        );
        b.bookmarks = vec![gone.clone(), Bookmark::folder(std::env::temp_dir()), chat.clone()];
        let id = b.id;
        state.update(cx, |s, cx| {
            s.tree = shed_model::Shed { benches: vec![b], ..Default::default() };
            cx.notify();
        });
        let tracker = PlaceTracker::new(Some(id), KeeperConn::detached(), gpui::WindowBounds::Windowed(gpui::Bounds::default()), None);
        let st = state.clone();
        let h = cx.add_window(|window, cx| BenchWindow::new(id, st, tracker, window, cx));
        cx.run_until_parked();
        h.update(cx, |v, _, cx| v.toggle_bookmarks(cx)).unwrap();
        cx.run_until_parked();
        let (open, button) = h.update(cx, |v, _, cx| (v.bookmarks_open, v.bar.read(cx).layout.borrow().bookmarks_button)).unwrap();
        assert!(open && button.is_some(), "open, under its button");
        h.update(cx, |v, _, cx| v.go_to_bookmark(gone.id, cx)).unwrap();
        let problem = h.update(cx, |v, _, _| (v.bookmarks_open, v.bookmark_problem.clone())).unwrap();
        assert_eq!(problem, (true, Some("/no/such/folder/main.rs is not there any more".into())));
        h.update(cx, |v, _, cx| v.go_to_bookmark(chat.id, cx)).unwrap();
        assert_eq!(h.update(cx, |v, _, _| v.bookmark_problem.clone()).unwrap().as_deref(), Some("Its AI tab is closed"));
        cx.run_until_parked();
        h.update(cx, |v, _, cx| v.toggle_bookmarks(cx)).unwrap();
        assert!(!h.update(cx, |v, _, _| v.bookmarks_open).unwrap());
    }

    /// Two rows: groups [a b] [c] on the first, [d] (wrapped) on the second.
    fn laid_out() -> (BarLayout, [TabId; 4]) {
        let ids = [TabId::new(), TabId::new(), TabId::new(), TabId::new()];
        let tab = |x: f32, y: f32| Bounds::new(gpui::point(px(x), px(y)), gpui::size(px(80.0), px(26.0)));
        let tabs = vec![(ids[0], 0, tab(4.0, 4.0)), (ids[1], 0, tab(86.0, 4.0)), (ids[2], 1, tab(184.0, 4.0)), (ids[3], 2, tab(4.0, 32.0))];
        (BarLayout { tabs, ..Default::default() }, ids)
    }

    #[test]
    fn where_a_dragged_tab_goes() {
        let (l, [a, b, c, d]) = laid_out();
        let at = |x: f32, y: f32| l.drop_at(gpui::point(px(x), px(y)), 1.0);
        assert_eq!(at(1.0, 10.0), Some(DropAt::NewRow(a)));
        assert_eq!(at(30.0, 10.0), Some(DropAt::Tab(a, false)));
        assert_eq!(at(70.0, 10.0), Some(DropAt::Tab(a, true)));
        assert_eq!(at(85.0, 10.0), Some(DropAt::Tab(a, true)));
        // The gap between the groups: a new group before c's.
        assert_eq!(at(175.0, 10.0), Some(DropAt::NewGroup(c)));
        // Past a row's end: after its last tab; the second row by height.
        assert_eq!(at(600.0, 10.0), Some(DropAt::Tab(c, true)));
        assert_eq!(at(5.0, 40.0), Some(DropAt::NewRow(d)));
        assert_eq!(at(60.0, 50.0), Some(DropAt::Tab(d, true)));
        assert_eq!(at(100.0, 3.0), Some(DropAt::Tab(b, false)));
    }

    #[test]
    fn groups_by_where_the_pointer_is() {
        let (l, _) = laid_out();
        let group = |x: f32, y: f32| l.group_at(gpui::point(px(x), px(y)));
        assert_eq!(group(85.0, 10.0), Some(0));
        assert_eq!(group(175.0, 10.0), Some(1));
        assert_eq!(group(600.0, 10.0), Some(1));
        assert_eq!(group(600.0, 40.0), Some(2));
        // One line, between the groups on the first row; none where d starts a row.
        let lines = l.separators(1.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].center().x, px(175.0));
    }
}

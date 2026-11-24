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
    tag_color,
};
use shed_model::config::NewPaneDir;
use shed_proto::{BenchRef, ClientMsg};

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

/// What the inline text field is editing.
#[derive(Clone, Copy, PartialEq)]
enum EditTarget {
    TabName(TabId),
    BenchName,
    AddTag(TabId),
    Badge,
    /// A colour of one's own: `#rrggbb` or a colour's name.
    Color,
    /// A folder to open as a project tab, typed where there is no picker.
    ProjectFolder,
    /// A server to open a remote tab on (`host` or `user@host`).
    RemoteHost,
    /// A new link for the bench: `name url`.
    AddLink,
    /// The bench's link at this index.
    EditLink(usize),
    /// The name of the bench's link at this index.
    RenameLink(usize),
}

impl EditTarget {
    fn describe(&self) -> &'static str {
        match self {
            EditTarget::TabName(_) => "Renaming the tab",
            EditTarget::BenchName => "Renaming the bench",
            EditTarget::AddTag(_) => "Adding a tag",
            EditTarget::Badge => "Setting the badge",
            EditTarget::Color => "Choosing a colour (#rrggbb or a name)",
            EditTarget::ProjectFolder => "Opening a folder as a project tab",
            EditTarget::RemoteHost => "Opening a remote tab: host, or user@host, as ssh takes it",
            EditTarget::AddLink | EditTarget::EditLink(_) => "A link: name, then the address (app localhost:3000)",
            EditTarget::RenameLink(_) => "The link's name",
        }
    }
}

/// A tab's one icon, the same on its bench's tab bar and in Shed: what it
/// is (terminal, program running, project, AI, remote), or that it waits
/// for you or is working; and its colour. An AI tab connected to the
/// browser shows a globe, so it is clear which one is.
pub fn tab_icon(tab: &Tab, accent: Rgb, ui: &UiColors) -> (&'static str, Rgb) {
    let waiting = tab.panes.iter().any(|p| p.waiting.is_some() || p.held.is_some());
    if waiting {
        ("tool-waiting", ui.ink(accent))
    } else if let Some(ai) = tab.ai_info() {
        let glyph = if ai.browser { "globe" } else { "ai" };
        // (Sessions going on in the background count too.)
        match ai.overall_activity() {
            shed_model::AiActivity::Waiting => ("tool-waiting", ui.ink(accent)),
            shed_model::AiActivity::Working => (glyph, ui.ink(accent)),
            shed_model::AiActivity::Idle => (glyph, if ai.browser { ui.text } else { ui.text_dim }),
        }
    } else if tab.project_info().is_some() {
        ("project", ui.text_dim)
    } else if tab.remote_info().is_some() {
        ("server", ui.text_dim)
    } else if tab.panes.iter().any(|p| p.tool.is_some()) {
        ("tool", ui.text_dim)
    } else {
        ("terminal", ui.text_dim)
    }
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

/// About how wide the tab bar's parts would be, at scale `s`: the tabs on
/// one row (by their titles, tags and zoom, in their groups); the usage
/// limits; and the other controls (Run Startup, Setup, the bench's name, its
/// links), each. Text is not measured before it is laid out, so this goes by
/// characters.
fn bar_widths(bench: &BarState, s: f32, limits: bool, lifecycle: bool, setup: bool) -> (f32, f32, Vec<f32>) {
    let ch = 7.4 * s;
    let tab = |t: &BarTab| {
        let tags: f32 = t.tags.iter().map(|g| g.label.chars().count() as f32 * 6.2 * s + 6.0 * s).sum();
        2.0 * TAB_PAD_X * s + 20.0 * s + t.title.chars().count() as f32 * ch + tags + if t.zoom != 0.0 { 44.0 * s } else { 0.0 }
    };
    // The bar's edges, the new-tab button.
    let mut tabs = 8.0 * s + 34.0 * s;
    for (i, t) in bench.tabs.iter().enumerate() {
        if i > 0 && t.divider != TabDivider::None {
            tabs += GROUP_GAP * s;
        }
        tabs += tab(t) + 2.0 * s;
    }
    let limits = if limits { 176.0 * s } else { 0.0 };
    let mut items = Vec::new();
    if lifecycle {
        items.push(32.0 * s);
    }
    if setup {
        items.push(32.0 * s);
    }
    items.push(bench.name.chars().count() as f32 * 8.0 * s + 54.0 * s);
    items.extend(bench.links.iter().map(|l| l.name.chars().count().min(22) as f32 * 7.0 * s + 40.0 * s));
    (tabs, limits, items)
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
    /// While a tab is dragged over another: that tab, and whether it would
    /// go after it (the right half) rather than before.
    drop_hint: Option<(TabId, bool)>,
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
    fn of(bench: BenchId, state: &AppState) -> Self {
        let settings = &state.settings;
        let bench = state.tree.bench(bench).map(|b| {
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
        Self { ui: settings.ui.clone(), background: settings.palette.background, errors: settings.errors.clone(), bench }
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
        let bar = cx.new(|_| TabBar { window: me, model: None, drawn: 0 });
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
        });
        self.bar.update(cx, |bar, cx| {
            if bar.model != model {
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

    /// Asks for a folder and opens it as a project tab; where the desktop
    /// has no folder picker, the folder is typed instead.
    fn new_project_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = crate::state::pick_folder(cx);
        cx.spawn_in(window, async move |this, cx| {
            let picked = picked.await;
            this.update_in(cx, |this, window, cx| match picked {
                Ok(Some(root)) => this.send(ClientMsg::NewProjectTab { bench: this.bench, root }, cx),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("no folder picker: {e}");
                    this.start_edit(EditTarget::ProjectFolder, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Where a new AI tab can work: the bench's projects, its folder, or one to pick.
    fn ai_folder_menu(&mut self, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some(bench) = self.bench(cx) else { return Vec::new() };
        let mut folders: Vec<std::path::PathBuf> = bench.tabs.iter().filter_map(|t| Some(t.project_info()?.root.clone())).collect();
        if !folders.contains(&bench.cwd) {
            folders.push(bench.cwd.clone());
        }
        let mut entries: Vec<MenuEntry<Self>> = folders
            .into_iter()
            .map(|folder| {
                let label = shed_save::home_relative(&folder);
                item(label, move |this, _, cx| this.send(ClientMsg::NewAiTab { bench: this.bench, folder: folder.clone() }, cx)).icon("folder")
            })
            .collect();
        entries.push(item("Other Folder…", |_, window, cx| {
            let picked = crate::state::pick_folder(cx);
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(Some(folder)) = picked.await {
                    this.update(cx, |this, cx| this.send(ClientMsg::NewAiTab { bench: this.bench, folder }, cx)).ok();
                }
            })
            .detach();
        }));
        entries
    }

    /// Servers for a new remote tab: those remote tabs used (latest first,
    /// marked so), those in `~/.ssh/config` (not Git hosts), or one to type.
    fn remote_host_menu(&mut self, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let recent = shed_client::remote::recent_hosts(&self.state.read(cx).tree);
        let mut entries: Vec<MenuEntry<Self>> = shed_client::remote::known_hosts(&recent)
            .into_iter()
            .map(|host| {
                let used = recent.contains(&host);
                more(host.clone(), move |this: &mut Self, cx| this.remote_shell_menu(&host, cx)).icon(if used { "history" } else { "server" })
            })
            .collect();
        entries.push(item("Other Host…", |this, window, cx| this.start_edit(EditTarget::RemoteHost, window, cx)));
        entries
    }

    /// A server's menu: a new shell, or one it keeps already (from another
    /// PC, or closed here), asked for over ssh while the menu is open.
    fn remote_shell_menu(&mut self, host: &str, cx: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
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
                    let entries = this.remote_shell_entries(&h, cx);
                    if let Some(menu) = &mut this.menu {
                        menu.entries = entries;
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        self.remote_shell_entries(host, cx)
    }

    fn remote_shell_entries(&self, host: &str, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let h = host.to_string();
        let mut entries = vec![
            item("New Shell", move |this, _, cx| this.send(ClientMsg::NewRemoteTab { bench: this.bench, host: h.clone(), join: None }, cx)).icon("plus"),
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
                            this.send(ClientMsg::NewRemoteTab { bench: this.bench, host: h.clone(), join: Some(id.clone()) }, cx)
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
            EditTarget::ProjectFolder => self.bench(cx).map(|b| shed_save::home_relative(&b.cwd)),
            EditTarget::RemoteHost | EditTarget::AddLink => Some(String::new()),
            EditTarget::EditLink(i) => self.bench(cx).and_then(|b| b.links.get(i)).map(crate::links::unparse),
            EditTarget::RenameLink(i) => self.bench(cx).and_then(|b| b.links.get(i)).map(|l| l.name.clone()),
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
            EditTarget::ProjectFolder => ClientMsg::NewProjectTab { bench: self.bench, root: text.trim().into() },
            EditTarget::RemoteHost if text.trim().is_empty() => return,
            EditTarget::RemoteHost => ClientMsg::NewRemoteTab { bench: self.bench, host: text.trim().to_string(), join: None },
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
        // Dividers group the tab bar: a space before this tab, or a new row.
        let divider = move |label: &'static str, d: TabDivider| {
            item(label, move |this: &mut Self, _, cx| this.send(ClientMsg::SetTabDivider { tab: tab_id, divider: d }, cx))
        };
        let first = self.bench(cx).is_some_and(|b| b.tabs.first().map(|t| t.id) == Some(tab_id));
        if !first {
            entries.push(more("Divider Before", move |this: &mut Self, cx| {
                let now = this.tab(tab_id, cx).map(|t| t.divider).unwrap_or_default();
                [
                    (TabDivider::Space, "Space: a New Group", "Space: a New Group  ✓"),
                    (TabDivider::Row, "New Row", "New Row  ✓"),
                    (TabDivider::None, "None", "None  ✓"),
                ]
                .into_iter()
                .map(|(d, label, checked)| divider(if d == now { checked } else { label }, d))
                .collect()
            }));
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
            item("New Project Tab…", |this, window, cx| this.new_project_tab(window, cx)).icon("project"),
            more("New AI Tab", |this: &mut Self, cx| this.ai_folder_menu(cx)).icon("ai"),
            more("New Remote Tab", |this: &mut Self, cx| this.remote_host_menu(cx)).icon("server"),
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
    icon: (&'static str, Rgb),
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
    drop_hint: Option<(TabId, bool)>,
    setup_open: bool,
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
}

impl TabBar {
    /// A listener that runs on the window, as `cx.listener` would there.
    fn on<E: ?Sized>(&self, f: impl Fn(&mut BenchWindow, &E, &mut Window, &mut Context<BenchWindow>) + 'static) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = self.window.clone();
        move |event, window, cx| {
            view.update(cx, |this, cx| f(this, event, window, cx)).ok();
        }
    }

    fn render_bar(&self, m: &BarModel, width: Pixels, cx: &App) -> impl IntoElement + use<> {
        let b = &m.bench;
        let s = b.scale;
        let ui = &b.ui;
        let accent = b.accent;
        let tab_count = b.tabs.len();
        // Tabs wrap onto more rows when they do not fit.
        let pad = px(4.0 * s);
        let tab_h = px(TAB_HEIGHT * s);
        let row_gap = px(2.0 * s);
        let mut tabs = div().flex_1().min_w_0().flex().flex_wrap().gap_x(px(GROUP_GAP * s)).gap_y(row_gap).p(pad);
        // Tabs in groups, split where a tab has a divider before it: a group
        // goes onto the next row whole, and wraps inside only when it alone
        // is wider than the bar. Groups are set apart by space alone.
        let group = move || div().flex().flex_wrap().min_w_0().gap(px(2.0 * s));
        let mut current = group();
        let base = b.base;

        for (i, tab) in b.tabs.iter().enumerate() {
            if i > 0 && tab.divider != TabDivider::None {
                tabs = tabs.child(std::mem::replace(&mut current, group()));
                if tab.divider == TabDivider::Row {
                    // A full-width nothing: what follows starts a row.
                    tabs = tabs.child(div().w_full().h(px(0.0)));
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
                    let entries = this.tab_menu(tab_id, cx);
                    this.open_menu(e.position, entries, cx);
                }))
                .on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
                // Dropped on the left half it goes before this tab, on the
                // right half after it: either way into this tab's group.
                .on_drag_move::<DraggedTab>(self.on(move |this, e: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                    let over = e.bounds.contains(&e.event.position);
                    let hint = if over {
                        Some((tab_id, e.event.position.x > e.bounds.center().x))
                    } else if this.drop_hint.is_some_and(|(t, _)| t == tab_id) {
                        None
                    } else {
                        return;
                    };
                    if this.drop_hint != hint {
                        this.drop_hint = hint;
                        this.redraw(cx);
                    }
                }))
                .on_drop(self.on(move |this, d: &DraggedTab, _, cx| {
                    let after = this.drop_hint.is_some_and(|(t, after)| t == tab_id && after);
                    this.drop_hint = None;
                    if d.tab != tab_id {
                        this.send(ClientMsg::MoveTabNextTo { tab: d.tab, to: tab_id, after }, cx);
                    }
                    this.redraw(cx);
                }));
            if cx.has_active_drag()
                && let Some((t, after)) = m.drop_hint
                && t == tab_id
            {
                el = if after { el.border_r_2() } else { el.border_l_2() }.border_color(hsla(accent));
            }
            // The shown tab: a soft fill of the bench's colour, stronger text.
            el = match (active, tint) {
                (true, Some(t)) => el.bg(hsla_alpha(t, 0.35)).text_color(hsla(ui.text)).font_weight(FontWeight::MEDIUM),
                (false, Some(t)) => el.bg(hsla_alpha(t, 0.16)).hover(|s| s.bg(hsla_alpha(t, 0.26))),
                (true, None) => el.bg(hsla_alpha(accent, 0.2)).text_color(hsla(ui.text)).font_weight(FontWeight::MEDIUM),
                (false, None) => el.hover(|s| s.bg(hsla_alpha(ui.text, 0.06)).text_color(hsla(ui.text))),
            };
            // One icon, as in Shed: what the tab is, or that it waits or works.
            let (glyph, tint) = tab.icon;
            el = el.child(svg().path(icon(glyph)).size(px(14.0 * s)).flex_none().text_color(hsla(tint)));
            if tab.private {
                el = el.child(svg().path(icon("private")).size(px(12.0 * s)).flex_none().text_color(hsla(ui.text_dim)));
            }
            match m.editor_for(EditTarget::TabName(tab_id)) {
                Some(editor) => el = el.child(editor),
                None => el = el.child(div().truncate().child(title)),
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

        // The new-tab button stands after the groups, in none, close to the last.
        let tabs = tabs.child(current).child(
            div()
                .id("new-tab")
                .ml(px((4.0 - GROUP_GAP) * s))
                .h(tab_h)
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
                    let entries = vec![
                        item("New Terminal Tab", |this, _, cx| this.new_tab(cx)).icon("terminal"),
                        item("New Project Tab…", |this, window, cx| this.new_project_tab(window, cx)).icon("project"),
                        more("New AI Tab", |this: &mut BenchWindow, cx| this.ai_folder_menu(cx)).icon("ai"),
                        more("New Remote Tab", |this: &mut BenchWindow, cx| this.remote_host_menu(cx)).icon("server"),
                    ];
                    this.open_menu(e.position, entries, cx);
                }))
                .child(svg().path(icon("tab-new")).size(px(16.0 * s)).text_color(hsla(ui.text_dim))),
        )
        .child(
            // Dropping a tab on the empty part of the bar moves it to the end.
            div()
                .id("tab-bar-end")
                .flex_1()
                // Never wider than what is left of the row: it never wraps
                // onto a row of its own.
                .min_w(px(0.0))
                .h(tab_h)
                .on_drop(self.on(move |this, d: &DraggedTab, _, cx| {
                    this.drop_hint = None;
                    this.send(ClientMsg::MoveTab { tab: d.tab, bench: this.bench, index: tab_count }, cx);
                    this.redraw(cx);
                })),
        );
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
        // Besides the limits: Run Startup, Setup, the bench's name, its links.
        let others: Vec<AnyElement> = lifecycle
            .map(|l| l.h(tab_h).into_any_element())
            .into_iter()
            .chain(setup.map(|d| d.h(tab_h).into_any_element()))
            .chain(std::iter::once(div().h(tab_h).flex().items_center().child(self.render_bench_label(m)).into_any_element()))
            .chain(self.render_links(b).into_iter().map(|l| div().h(tab_h).flex().items_center().child(l).into_any_element()))
            .collect();
        // The controls on the right: one row beside the tabs when all fits on
        // one; when the tabs need more rows, a column beside them (the limits
        // on top, the rest under them, wrapping), so the tabs get the width.
        // Either way centred against the tabs.
        let (tabs_w, limits_w, items) = bar_widths(b, s, limits.is_some(), lifecycle_shown, terminal || setup_open);
        let gap = 4.0 * s;
        let items_w = items.iter().sum::<f32>() + gap * items.len().saturating_sub(1) as f32;
        let one_row = limits_w + if limits_w > 0.0 { gap } else { 0.0 } + items_w + 2.0 * f32::from(pad);
        let room = f32::from(width);
        let controls = if tabs_w + one_row <= room {
            div().flex().flex_none().items_center().gap_x(px(gap)).p(pad).children(limits).children(others)
        } else {
            let widest = items.iter().copied().fold(limits_w, f32::max);
            let column = widest.max(items_w.min(room * 0.25)).ceil() + 2.0;
            div()
                .flex()
                .flex_none()
                .flex_col()
                .items_end()
                .gap_y(row_gap)
                .p(pad)
                .max_w(px(column) + pad * 2.0)
                .children(limits)
                .child(div().flex().flex_wrap().justify_end().items_center().gap_x(px(gap)).gap_y(row_gap).children(others))
        };
        div()
            .id("tab-bar")
            .flex()
            .flex_none()
            .items_center()
            // Set apart from the panes: a ground of the bench's colour, a line under it.
            .bg(hsla_alpha(accent, 0.09))
            .border_b_1()
            .border_color(hsla_alpha(accent, 0.35))
            .text_size(px(13.5 * s))
            .text_color(hsla(ui.text_dim))
            .child(tabs)
            .child(controls)
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
            Some(m) => self.render_bar(m, width, cx).into_any_element(),
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
            .on_action(cx.listener(|this, _: &NewProjectTab, window, cx| this.new_project_tab(window, cx)))
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
}

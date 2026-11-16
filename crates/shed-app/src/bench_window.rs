//! A bench's window: the bench-colour stripe, the tab bar, and the active
//! tab's panes laid out by its split tree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, PromptLevel, SharedString, Subscription, Window, canvas,
    div, prelude::*, px, relative, svg,
};
use shed_model::{TabDivider, 
    Bench, BenchColor, BenchId, Direction, PaneId, PaneNode, Rgb, SplitDir, Tab, TabId, Tag,
    tag_color,
};
use shed_model::config::NewPaneDir;
use shed_proto::{BenchRef, ClientMsg};

use crate::actions::*;
use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu::{self, Menu, MenuEntry, MenuHost, danger, more};
use crate::setup_panel::{PanelEvent, SetupPanel};
use crate::state::{AppState, Focus};
use crate::theme::{UiColors, hsla, hsla_alpha};
use crate::window_place::{PlaceTracker, Placed};

const TAB_BAR_HEIGHT: f32 = 40.0;

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
/// for you or is working; and its colour.
pub fn tab_icon(tab: &Tab, accent: Rgb, ui: &UiColors) -> (&'static str, Rgb) {
    let waiting = tab.panes.iter().any(|p| p.waiting.is_some() || p.held.is_some());
    if waiting {
        ("tool-waiting", ui.ink(accent))
    } else if let Some(ai) = tab.ai_info() {
        match ai.activity {
            shed_model::AiActivity::Waiting => ("tool-waiting", ui.ink(accent)),
            shed_model::AiActivity::Working => ("ai", ui.ink(accent)),
            shed_model::AiActivity::Idle => ("ai", ui.text_dim),
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
    ui: UiColors,
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

enum RemoteShells {
    Looking(Vec<crate::remote::RemoteShell>),
    Found(Vec<crate::remote::RemoteShell>),
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
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let following = PlaceTracker::follow(window, cx);
        Self {
            bench,
            state,
            ui: UiColors::default(),
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
        }
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
        cx.notify();
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
        cx.notify();
    }

    /// Re-applies focus after an overlay (a text field, a menu) closes.
    fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((focus, handle)) = self.state.read(cx).active_focus(self.bench, cx) else { return };
        window.focus(&handle, cx);
        self.focused = Some(focus);
        cx.notify();
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

    /// Servers for a new remote tab: those in `~/.ssh/config`, or one to type.
    fn remote_host_menu(&mut self, _: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let mut entries: Vec<MenuEntry<Self>> = crate::remote::known_hosts()
            .into_iter()
            .map(|host| more(host.clone(), move |this: &mut Self, cx| this.remote_shell_menu(&host, cx)).icon("server"))
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
            let found = cx.background_executor().spawn(async move { crate::remote::list_shells(&asked) }).await;
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
                        item(crate::remote::describe(shell), move |this, _, cx| {
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
        let root = crate::project_view::project_root(&cwd);
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
        cx.notify();
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

    fn editor_for(&self, target: EditTarget) -> Option<Entity<LineEditor>> {
        self.editing.as_ref().filter(|(t, _, _)| *t == target).map(|(_, e, _)| e.clone())
    }

    fn render_tab_bar(&self, bench: &Bench, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.bar_scale(cx);
        let ui = &self.ui;
        let accent = bench.color.accent();
        let tab_count = bench.tabs.len();
        let row = px(TAB_BAR_HEIGHT * s);
        // Tabs wrap onto more rows when they do not fit; the buttons stay on
        // the right of the first.
        let pad = px(4.0 * s);
        let tab_h = row - pad * 2.0;
        let mut tabs = div().flex_1().min_w_0().flex().flex_wrap().gap_x(px(10.0 * s)).gap_y(pad).p(pad);
        // Tabs in groups, split where a tab has a divider before it: a group
        // goes onto the next row whole, and wraps inside only when it alone
        // is wider than the bar. Each group sits on a tray of its own.
        let group = move || {
            div()
                .flex()
                .flex_wrap()
                .min_w_0()
                .gap(px(2.0 * s))
                // On a tray, one group or several: the bar looks the same
                // with dividers or without.
                .p(px(2.0 * s))
                .rounded_lg()
                .bg(hsla_alpha(accent, 0.07))
                .border_1()
                .border_color(hsla_alpha(accent, 0.22))
        };
        let mut current = group();
        let base = self.state.read(cx).settings.font.size;

        for (i, tab) in bench.tabs.iter().enumerate() {
            if i > 0 && tab.divider != TabDivider::None {
                tabs = tabs.child(std::mem::replace(&mut current, group()));
                if tab.divider == TabDivider::Row {
                    // A full-width nothing: what follows starts a row.
                    tabs = tabs.child(div().w_full().h(px(0.0)));
                }
            }
            let active = tab.id == bench.active_tab;
            let tab_id = tab.id;
            let title = tab.display_title();
            let tint = tab.all_tags().find(|t| t.tint_tab).map(|t| t.color);
            let mut el = div()
                .id(("tab", i))
                .group("tab")
                .flex()
                .items_center()
                .gap_1p5()
                .h(tab_h - px(4.0 * s) - px(2.0))
                .px_3()
                .max_w(px(260.0 * s))
                .rounded_md()
                .cursor_pointer()
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_edit(EditTarget::TabName(tab_id), window, cx);
                    } else {
                        this.send(ClientMsg::FocusTab { tab: tab_id }, cx);
                    }
                }))
                .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| {
                    this.close_tab(tab_id, window, cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let entries = this.tab_menu(tab_id, cx);
                    this.open_menu(e.position, entries, cx);
                }))
                .on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
                // Dropped on the left half it goes before this tab, on the
                // right half after it: either way into this tab's group.
                .on_drag_move::<DraggedTab>(cx.listener(move |this, e: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
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
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                    let after = this.drop_hint.is_some_and(|(t, after)| t == tab_id && after);
                    this.drop_hint = None;
                    if d.tab != tab_id {
                        this.send(ClientMsg::MoveTabNextTo { tab: d.tab, to: tab_id, after }, cx);
                    }
                    cx.notify();
                }));
            if cx.has_active_drag()
                && let Some((t, after)) = self.drop_hint
                && t == tab_id
            {
                el = if after { el.border_r_2() } else { el.border_l_2() }.border_color(hsla(accent));
            }
            el = match (active, tint) {
                (true, Some(t)) => el.bg(hsla_alpha(t, 0.35)).text_color(hsla(ui.text)).font_weight(FontWeight::MEDIUM),
                (false, Some(t)) => el.bg(hsla_alpha(t, 0.16)).hover(|s| s.bg(hsla_alpha(t, 0.26))),
                (true, None) => el.bg(hsla(ui.tab_active)).text_color(hsla(ui.text)).font_weight(FontWeight::MEDIUM),
                (false, None) => el.hover(|s| s.bg(hsla_alpha(ui.text, 0.06))),
            };
            if active {
                // The shown tab: lifted, with the bench's colour under it.
                el = el.border_b_2().border_color(hsla(accent));
            }
            // One icon, as in Shed: what the tab is, or that it waits or works.
            let (glyph, tint) = tab_icon(tab, accent, ui);
            el = el.child(svg().path(icon(glyph)).size(px(14.0 * s)).flex_none().text_color(hsla(tint)));
            if tab.private {
                el = el.child(svg().path(icon("private")).size(px(12.0 * s)).flex_none().text_color(hsla(ui.text_dim)));
            }
            match self.editor_for(EditTarget::TabName(tab_id)) {
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
                        .tooltip(menu::tooltip("Zoom: click for the usual size"))
                        .child(format!("{pct}%"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.send(ClientMsg::SetZoom { tab: tab_id, zoom: 0.0 }, cx);
                        })),
                );
            }
            for tag in tab.all_tags() {
                el = el.child(
                    div()
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .text_size(px(11.0 * s))
                        .bg(hsla_alpha(tag.color, 0.25))
                        .text_color(hsla(self.ui.ink(tag.color)))
                        .child(tag.label.clone()),
                );
            }
            if let Some(editor) = self.editor_for(EditTarget::AddTag(tab_id)) {
                el = el.child(editor);
            }
            current = current.child(el);
        }

        // The new-tab button stands after the trays, in no group.
        let tabs = tabs.child(current).child(
            div()
                .id("new-tab")
                .h(tab_h)
                .rounded_md()
                .flex()
                .items_center()
                .px_2()
                .cursor_pointer()
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .tooltip(menu::tooltip("New tab (right-click for a project tab)"))
                .on_click(cx.listener(|this, _, _, cx| this.new_tab(cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let entries = vec![
                        item("New Terminal Tab", |this, _, cx| this.new_tab(cx)).icon("terminal"),
                        item("New Project Tab…", |this, window, cx| this.new_project_tab(window, cx)).icon("project"),
                        more("New AI Tab", |this: &mut Self, cx| this.ai_folder_menu(cx)).icon("ai"),
                        more("New Remote Tab", |this: &mut Self, cx| this.remote_host_menu(cx)).icon("server"),
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
                .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                    this.drop_hint = None;
                    this.send(ClientMsg::MoveTab { tab: d.tab, bench: this.bench, index: tab_count }, cx);
                })),
        );
        // Startup and teardown on demand: a bench opens quietly, and runs its
        // teardown when closed only if its startup ran.
        let has_startup = bench.tabs.iter().flat_map(|t| &t.panes).any(|p| !p.setup.startup.is_empty());
        let has_teardown = bench.tabs.iter().flat_map(|t| &t.panes).any(|p| !p.setup.teardown.is_empty());
        // A small icon like the bar's others, in the bench's colour; what it
        // does in its tooltip.
        let run = |id: &'static str, glyph: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .px_2()
                .cursor_pointer()
                .hover(|st| st.bg(hsla(ui.tab_hover)))
                .tooltip(menu::tooltip(tip))
                .child(svg().path(icon(glyph)).size(px(16.0 * s)).text_color(hsla(ui.ink(accent))))
        };
        let lifecycle = if bench.closing {
            None
        } else if has_startup && !bench.started {
            Some(run("run-startup", "go", "Run Startup: each pane's startup now; closing then runs the teardown").on_click(cx.listener(|this, _, _, cx| {
                this.send(ClientMsg::RunStartup { bench: this.bench }, cx);
            })))
        } else if bench.started && has_teardown {
            Some(run("run-teardown", "stop", "Run Teardown: stop what runs and run each pane's teardown; the bench stays open").on_click(cx.listener(|this, _, _, cx| {
                this.send(ClientMsg::RunTeardown { bench: this.bench }, cx);
            })))
        } else {
            None
        };
        // Claude's usage limits, on benches that talk to it.
        let limits = bench
            .tabs
            .iter()
            .any(|t| t.ai_info().is_some())
            .then(|| cx.try_global::<crate::ai_view::AiLimits>().map(|l| l.0))
            .flatten()
            .and_then(|l| crate::ai_view::limit_bars(l, ui, &self.state.read(cx).settings.palette, s));
        // Setup is for terminal tabs: startup, teardown and aliases.
        let terminal = bench.active_tab().is_some_and(|t| t.project_info().is_none() && t.ai_info().is_none());
        let controls = div()
            .flex()
            .flex_none()
            .h(row)
            .children(limits.map(|l| l.px_2()))
            .children(lifecycle)
            .child(self.render_links(bench, cx))
        .when(terminal || self.setup.is_some(), |d| d.child(
            div()
                .id("setup-button")
                .flex()
                .items_center()
                .px_2()
                .cursor_pointer()
                .when(self.setup.is_some(), |d| d.bg(hsla(ui.tab_active)))
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .tooltip(menu::tooltip("Setup: startup, teardown, aliases"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_setup(window, cx)))
                .child(svg().path(icon("rack")).size(px(16.0 * s)).text_color(hsla(if self.setup.is_some() { accent } else { ui.text_dim }))),
        ))
            .child(self.render_bench_label(bench, cx));
        div()
            .id("tab-bar")
            .flex()
            .flex_none()
            .items_start()
            .min_h(row)
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
    /// How large the tab bar is drawn (`[window] tab_bar_scale`).
    fn bar_scale(&self, cx: &App) -> f32 {
        self.state.read(cx).settings.config.window.tab_bar_scale.clamp(0.7, 2.0) * crate::theme::ui_scale()
    }

    fn render_links(&self, bench: &Bench, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = bench.color.accent();
        let links = bench.links.clone();
        div().flex().flex_none().items_center().gap_1().px_1().children(bench.links.iter().enumerate().map(|(i, link)| {
            let url = link.url.clone();
            let links = links.clone();
            crate::links::chip(format!("link-{i}"), link, accent, &self.ui)
                .tooltip(menu::tooltip("Open the link (right-click for more)"))
                .on_click(cx.listener(move |_, _, _, cx| crate::links::open(url.clone(), false, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let entries = crate::links::menu(
                        links.clone(),
                        i,
                        |this: &mut Self, links, cx| this.send(ClientMsg::SetBenchLinks { bench: this.bench, links }, cx),
                        move |this: &mut Self, window, cx| this.start_edit(EditTarget::RenameLink(i), window, cx),
                        move |this: &mut Self, window, cx| this.start_edit(EditTarget::EditLink(i), window, cx),
                    );
                    this.open_menu(e.position, entries, cx);
                }))
        }))
    }

    fn render_bench_label(&self, bench: &Bench, cx: &mut Context<Self>) -> AnyElement {
        let s = self.bar_scale(cx);
        let in_label = |t: &EditTarget| !matches!(t, EditTarget::TabName(_) | EditTarget::AddTag(_));
        let editor = self.editing.as_ref().filter(|(t, _, _)| in_label(t)).map(|(_, e, _)| e.clone());
        if let Some(editor) = editor {
            return div().flex().items_center().px_2().child(editor).into_any_element();
        }
        let accent = bench.color.accent();
        div()
            .id("bench-label")
            .flex()
            .items_center()
            .gap_1p5()
            .px_3()
            .cursor_pointer()
            .text_color(hsla(self.ui.ink(accent)))
            .hover(|s| s.bg(hsla(self.ui.tab_hover)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu(cx);
                this.open_menu(e.position, entries, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu(cx);
                this.open_menu(e.position, entries, cx);
            }))
            .child(svg().path(icon("bench")).size(px(14.0 * s)).text_color(hsla(self.ui.ink(accent))))
            .child(bench.name.clone())
            .when_some(bench.badge.clone(), |el, badge| {
                el.child(div().px_1().rounded_sm().bg(hsla_alpha(accent, 0.25)).child(badge))
            })
            .into_any_element()
    }

    fn render_node(&self, node: &PaneNode, path: Vec<bool>, tab: &Tab, accent: Rgb, cx: &mut Context<Self>) -> AnyElement {
        match node {
            PaneNode::Leaf(id) => {
                let pane = *id;
                let split = tab.panes.len() > 1;
                let active = pane == tab.active_pane;
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
                if let Some(view) = self.state.read(cx).panes.get(&pane) {
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
                    .child(self.render_node(first, first_path, tab, accent, cx));
                let (tab_id, layout, drag_path) = (tab.id, tab.layout.clone(), path.clone());
                // A thin line inside a wider grab area.
                let divider = div()
                    .id(SharedString::from(format!("divider-{path:?}")))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .map(|d| if row { d.w(px(5.0)).h_full().flex_row().cursor_col_resize() } else { d.h(px(5.0)).w_full().flex_col().cursor_row_resize() })
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.drag = Some(SplitDrag { tab: tab_id, path: drag_path.clone(), dir, layout: layout.clone() });
                        cx.stop_propagation();
                    }))
                    .child(div().bg(hsla(self.ui.divider)).map(|d| if row { d.w(px(1.0)).h_full() } else { d.h(px(1.0)).w_full() }));
                let second_el = div().flex_1().min_w_0().min_h_0().child(self.render_node(second, second_path, tab, accent, cx));
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::terminal_view::perf::enabled() {
            crate::terminal_view::perf::WINDOW_RENDERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.ui = self.state.read(cx).settings.ui.clone();
        let errors = self.state.read(cx).settings.errors.clone();
        let Some(bench) = self.bench(cx).cloned() else {
            return div().size_full().bg(hsla(self.ui.background)).into_any_element();
        };
        let accent = bench.color.accent();
        let content = match bench.active_tab() {
            Some(tab) if tab.project_info().is_some() => match self.state.read(cx).projects.get(&tab.id) {
                Some(view) => div().size_full().child(view.clone()).into_any_element(),
                None => div().into_any_element(),
            },
            Some(tab) if tab.ai_info().is_some() => match self.state.read(cx).ai_views.get(&tab.id) {
                Some(view) => div().size_full().child(view.clone()).into_any_element(),
                None => div().into_any_element(),
            },
            Some(tab) => self.render_node(&tab.layout, Vec::new(), tab, accent, cx),
            None => div().into_any_element(),
        };
        let ui = self.ui.clone();
        let menu = self.menu.as_ref().map(|m| menu::render(m, &ui, cx));

        div()
            .key_context("BenchWindow")
            .size_full()
            .flex()
            .flex_col()
            // The terminal background, drawn first and full-window, so the renderer
            // turns it into the frame's clear colour.
            .bg(hsla(self.state.read(cx).settings.palette.background))
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
            .child(self.render_tab_bar(&bench, cx))
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
                                .bg(hsla(self.ui.tab_active))
                                .border_1()
                                .border_color(hsla(self.ui.accent))
                                .text_color(hsla(self.ui.text))
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
                        .bg(hsla_alpha(self.ui.danger, 0.2))
                        .text_size(px(12.0))
                        .text_color(hsla(self.ui.text))
                        .child(format!("shed.toml: {}", errors.join("; "))),
                )
            })
            .children(menu)
            .into_any_element()
    }
}


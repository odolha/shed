//! The Shed window: the pegboard, with the Shed icon. It lives as long as
//! Shed does; closing it closes every bench and Shed with them. Its header
//! switches between this PC's workspace and others reached through a nexus
//! this desktop is signed in to (and signs in).

use gpui::{Context, Entity, Focusable, PromptLevel, SharedString, Subscription, Window, div, prelude::*, px, svg};
use shed_client::nexus_client::WorkspaceInfo;
use shed_model::config::Appearance;
use shed_proto::{ClientMsg, ShowTarget};

use crate::actions::NewBench;
use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu::{self, Menu, MenuEntry, MenuHost, item};
use crate::state::Workspace;
use crate::pegboard::Pegboard;
use crate::state::AppState;
use crate::theme::{hsla, hsla_alpha, is_dark};
use crate::window_place::{PlaceTracker, Placed};

pub struct ShedWindow {
    state: Entity<AppState>,
    board: Entity<Pegboard>,
    /// Shed is closing: benches are being closed, then it quits.
    quitting: bool,
    place: PlaceTracker,
    /// The settings, over the right side, while open.
    settings: Option<(Entity<crate::settings_panel::SettingsPanel>, Subscription)>,
    /// How the link to the nexus is doing, read every few seconds while a
    /// nexus is set (`None`: no link runs).
    link: Option<shed_client::link_status::LinkStatus>,
    /// The workspace switcher's menu, while open.
    menu: Option<Menu<Self>>,
    /// The nexus's workspaces, as last listed (`None`: not yet), and the
    /// listing while it runs.
    workspaces: Option<Result<Vec<WorkspaceInfo>, String>>,
    listing: Option<gpui::Task<()>>,
    /// The sign-in dialog, while open.
    sign_in: Option<SignInDialog>,
    /// What the window shows of `AppState`, as of its last change.
    shown: Shown,
    _following: Subscription,
}

/// What the header and banners show of `AppState`. Kept here and compared,
/// so that the window draws again only when this changes: rendering never
/// reads `AppState`, which changes with every title a program sets.
#[derive(Clone, PartialEq)]
struct Shown {
    ui: crate::theme::UiColors,
    running: usize,
    appearance: Option<Appearance>,
    compact: bool,
    notice: Option<String>,
    workspace: Workspace,
    switching: Option<String>,
    nexus_url: String,
    nexus_on: bool,
}

impl Shown {
    fn of(state: &AppState) -> Self {
        let config = &state.settings.config;
        Self {
            ui: state.settings.ui.clone(),
            running: state.tree.benches.len(),
            appearance: config.appearance(),
            compact: config.window.compact_pegboard,
            notice: state.notice.clone(),
            workspace: state.workspace.clone(),
            switching: state.switching.clone(),
            nexus_url: config.nexus.url.clone(),
            nexus_on: config.nexus.active(),
        }
    }
}

/// Signing this desktop in to a nexus: its address, user, password and a
/// code from the authenticator.
struct SignInDialog {
    url: Entity<LineEditor>,
    user: Entity<LineEditor>,
    password: Entity<LineEditor>,
    code: Entity<LineEditor>,
    error: Option<String>,
    busy: bool,
    _subs: Vec<Subscription>,
}

impl Placed for ShedWindow {
    fn tracker(&mut self) -> &mut PlaceTracker {
        &mut self.place
    }
}

impl ShedWindow {
    pub fn new(state: Entity<AppState>, place: PlaceTracker, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |this, cx| this.request_quit(window, cx)).ok();
            // The window goes when Shed does, once every bench is closed.
            false
        });
        cx.observe(&state, |this: &mut Self, state, cx| {
            let shown = Shown::of(state.read(cx));
            if shown != this.shown {
                this.shown = shown;
                cx.notify();
            }
        })
        .detach();
        // `theme = "auto"` follows the desktop as it switches.
        cx.observe_window_appearance(window, |this, window, cx| {
            let dark = is_dark(window.appearance());
            this.state.update(cx, |state, cx| state.set_dark(dark, cx));
        })
        .detach();
        let board = cx.new(|cx| Pegboard::new(state.clone(), window, cx));
        // The link's status file, while a nexus is set: cheap to read.
        cx.spawn_in(window, async move |this, cx| loop {
            let linked = this.update(cx, |this, cx| !this.state.read(cx).settings.config.nexus.url.trim().is_empty());
            let Ok(linked) = linked else { break };
            let status = if linked { cx.background_executor().spawn(async { shed_client::link_status::LinkStatus::current() }).await } else { None };
            if this
                .update(cx, |this, cx| {
                    if this.link != status {
                        this.link = status;
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
        })
        .detach();
        // SHED_PERF_FRAMES=<n>: redraw the first bench n times a second, as
        // hovering in it does, to measure what a frame of this window costs.
        if let Some(fps) = std::env::var("SHED_PERF_FRAMES").ok().and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0) {
            let board = board.downgrade();
            cx.spawn_in(window, async move |_, cx| loop {
                cx.background_executor().timer(std::time::Duration::from_millis(1000 / fps)).await;
                let Ok(card) = board.update(cx, |board, _| board.first_card()) else { break };
                if let Some(card) = card {
                    card.update(cx, |_, cx| cx.notify());
                }
            })
            .detach();
        }
        let following = PlaceTracker::follow(window, cx);
        let shown = Shown::of(state.read(cx));
        Self {
            shown,
            state,
            board,
            quitting: false,
            place,
            settings: None,
            link: None,
            menu: None,
            workspaces: None,
            listing: None,
            sign_in: None,
            _following: following,
        }
    }

    /// Lists the nexus's workspaces in the background; the switcher's menu,
    /// if open, is made again with them.
    fn list_workspaces(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = shed_client::nexus_client::SignIn::load() else {
            self.workspaces = None;
            return;
        };
        let ca = shed_client::nexus_client::configured_ca_file();
        self.listing = Some(cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move { shed_client::nexus_client::workspaces(&sign_in, ca.as_deref()).map_err(|e| format!("{e:#}")) })
                .await;
            this.update(cx, |this, cx| {
                this.listing = None;
                this.workspaces = Some(listed);
                if this.menu.is_some() {
                    let entries = this.switcher_entries(cx);
                    if let Some(menu) = &mut this.menu {
                        menu.reset(entries);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The switcher's menu: this PC, the nexus's workspaces (offline ones
    /// not to be chosen), and signing in or out.
    fn switcher_entries(&self, cx: &gpui::App) -> Vec<MenuEntry<Self>> {
        let state = self.state.read(cx);
        let current = state.workspace.clone();
        let mark = |on: bool| if on { "check" } else { "bench" };
        let mut entries = vec![item("This PC", |this: &mut Self, _, cx| this.switch(Workspace::Local, cx)).icon(mark(current == Workspace::Local))];
        let sign_in = shed_client::nexus_client::SignIn::load();
        // This PC, if paired, is in the nexus's list too: it is "This PC" here.
        let me = shed_link::pairing::Secret::load().map(|s| s.workspace);
        match (&sign_in, &self.workspaces) {
            (None, _) => {}
            (Some(_), None) => entries.push(menu::disabled("Workspaces", "listing…").icon("server")),
            (Some(_), Some(Err(e))) => {
                entries.push(menu::disabled("Workspaces", "cannot list").icon("alert"));
                entries.push(menu::disabled(e.chars().take(60).collect::<String>(), ""));
            }
            (Some(_), Some(Ok(list))) => {
                let mut list: Vec<&WorkspaceInfo> = list.iter().filter(|w| Some(&w.id) != me.as_ref()).collect();
                list.sort_by_key(|w| w.name.to_lowercase());
                if list.is_empty() {
                    entries.push(menu::disabled("No other workspaces", ""));
                }
                for w in list {
                    let target = Workspace::Remote { id: w.id.clone(), name: w.name.clone() };
                    let on = current == target;
                    if w.online || on {
                        entries.push(item(w.name.clone(), move |this: &mut Self, _, cx| this.switch(target.clone(), cx)).icon(if on { "check" } else { "server" }));
                    } else {
                        entries.push(menu::disabled(w.name.clone(), "offline").icon("server"));
                    }
                }
            }
        }
        entries.push(MenuEntry::Separator);
        match sign_in {
            Some(s) => {
                entries.push(menu::disabled(format!("Signed in as {}", s.username), short_url(&s.url)));
                // A desktop's sign-in ends some days after it was made.
                let now = shed_link::pairing::now();
                let ends = if s.ended(now) {
                    Some("Sign-in ended".to_string())
                } else {
                    s.ends_soon(now).map(|d| format!("Sign-in ends in {d} day{}", if d == 1 { "" } else { "s" }))
                };
                if let Some(ends) = ends {
                    entries.push(menu::disabled(ends, "").icon("alert"));
                    entries.push(item("Sign In Again…", |this: &mut Self, window, cx| this.open_sign_in(window, cx)).icon("globe"));
                }
                entries.push(item("Sign Out", |this: &mut Self, _, cx| this.sign_out(cx)).icon("power"));
            }
            None => entries.push(item("Sign In to Nexus…", |this: &mut Self, window, cx| this.open_sign_in(window, cx)).icon("globe")),
        }
        // The panel's pinned bench launchers can switch with the workspace.
        entries.push(MenuEntry::Separator);
        let follows = state.settings.config.desktop.taskbar_follows_workspace;
        match shed_desktop::taskbar::Panel::detect() {
            Some(panel) => entries.push(
                item("Taskbar Follows Workspace", move |this: &mut Self, _, cx| {
                    this.state.update(cx, |state, cx| state.set_taskbar_follows(!follows, cx));
                })
                .icon(if follows { "check" } else { "pegboard" })
                .tip(format!(
                    "Switching workspace switches the bench launchers pinned in the {} panel: each workspace keeps its own pins. Other pins are left as they are.",
                    panel.name()
                )),
            ),
            None => entries.push(
                menu::disabled("Taskbar Follows Workspace", "")
                    .icon("pegboard")
                    .tip("Only on Cinnamon and GNOME: other desktops' panels are not changed."),
            ),
        }
        entries
    }

    fn switch(&mut self, target: Workspace, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| state.switch_to(target, cx));
    }

    fn open_switcher(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        if self.listing.is_none() {
            self.list_workspaces(cx);
        }
        let entries = self.switcher_entries(cx);
        self.open_menu(position, entries, cx);
    }

    fn sign_out(&mut self, cx: &mut Context<Self>) {
        // Showing another workspace through it: back to this PC first.
        self.state.update(cx, |state, cx| state.switch_to(Workspace::Local, cx));
        self.workspaces = None;
        cx.notify();
        // The nexus revokes this desktop first; it is forgotten here either way.
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async { shed_link::pairing::sign_out_blocking() }).await;
            let notice = match done {
                Ok(shed_link::pairing::SignedOut::Revoked) | Ok(shed_link::pairing::SignedOut::NotSignedIn) => None,
                Ok(other) => Some(other.message()),
                Err(e) => Some(format!("Cannot sign out: {e:#}")),
            };
            state.update(cx, |state, cx| {
                if notice.is_some() {
                    state.notice = notice;
                }
                cx.notify();
            });
            this.update(cx, |this, cx| {
                this.workspaces = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = shed_link::pairing::default_nexus().unwrap_or_default();
        let url = cx.new(|cx| LineEditor::new(url, cx).without_hint());
        let user = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
        let password = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint().secret());
        let code = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
        let first = if url.read(cx).text().is_empty() { &url } else { &user };
        window.focus(&first.read(cx).focus_handle(cx), cx);
        // Enter in any field signs in; Esc closes.
        let subs = [&url, &user, &password, &code]
            .into_iter()
            .map(|field| {
                cx.subscribe_in(field, window, |this, _, e: &EditorEvent, window, cx| match e {
                    EditorEvent::Submit(_) => this.submit_sign_in(window, cx),
                    EditorEvent::Cancel => {
                        this.sign_in = None;
                        cx.notify();
                    }
                })
            })
            .collect();
        self.sign_in = Some(SignInDialog { url, user, password, code, error: None, busy: false, _subs: subs });
        cx.notify();
    }

    fn submit_sign_in(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &mut self.sign_in else { return };
        if d.busy {
            return;
        }
        let (url, user, password, code) =
            (d.url.read(cx).text().trim().to_string(), d.user.read(cx).text().trim().to_string(), d.password.read(cx).text().to_string(), d.code.read(cx).text().trim().to_string());
        if url.is_empty() || user.is_empty() || password.is_empty() || code.is_empty() {
            d.error = Some("Fill in every field.".into());
            cx.notify();
            return;
        }
        d.busy = true;
        d.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let signed = cx
                .background_executor()
                .spawn(async move { shed_link::pairing::sign_in_blocking(&url, &user, &password, &code).map_err(|e| format!("{e:#}")) })
                .await;
            this.update(cx, |this, cx| {
                match signed {
                    Ok(_) => {
                        this.sign_in = None;
                        this.workspaces = None;
                        this.list_workspaces(cx);
                    }
                    Err(e) => {
                        if let Some(d) = &mut this.sign_in {
                            d.busy = false;
                            d.error = Some(e);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.take().is_none() {
            let panel = cx.new(|cx| crate::settings_panel::SettingsPanel::new(self.state.clone(), cx));
            let sub = cx.subscribe(&panel, |this, _, event: &crate::settings_panel::SettingsEvent, cx| {
                let crate::settings_panel::SettingsEvent::Close = event;
                this.settings = None;
                cx.notify();
            });
            self.settings = Some((panel, sub));
        }
        cx.notify();
    }

    /// Closing Shed closes every bench; asks first if any is running.
    pub fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        // Every window's place, so all open the same next time.
        self.place.record(window);
        self.state.update(cx, |state, cx| state.record_places(cx));
        let running = self.state.read(cx).local_running();
        if running == 0 {
            self.quit(cx);
            return;
        }
        let benches = if running == 1 { "its running bench".to_string() } else { format!("its {running} running benches") };
        // Asked from the taskbar, the window may be minimized or behind others.
        window.activate();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Close Shed and {benches}?"),
            Some(
                "Every bench is saved, then the programs running in it stop and its teardown \
                 commands run. Open benches again from Shed to bring them back.\n\n\
                 To keep a bench running, leave Shed open; hiding a bench closes only its window.",
            ),
            &["Close Shed", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(0) = answer.await {
                this.update(cx, |this, cx| this.quit(cx)).ok();
            }
        })
        .detach();
    }

    fn quit(&mut self, cx: &mut Context<Self>) {
        self.quitting = true;
        // The keeper closes every bench and stops; losing it quits this process.
        self.state.update(cx, |state, cx| state.quit_shed(cx));
        cx.notify();
    }
}

impl Render for ShedWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let perf = crate::terminal_view::perf::enabled();
        if perf {
            crate::terminal_view::perf::shed_frame_started();
        }
        let shown = self.shown.clone();
        let ui = shown.ui.clone();
        let running = shown.running;
        let current = shown.appearance;
        let appearances = [
            (Appearance::Auto, "theme-auto", "Follow the desktop's light or dark"),
            (Appearance::Light, "theme-light", "Light"),
            (Appearance::Dark, "theme-dark", "Dark"),
        ];
        let switch = div()
            .flex()
            .items_center()
            .p_0p5()
            .rounded_md()
            .border_1()
            .border_color(hsla(ui.divider))
            .children(appearances.map(|(appearance, glyph, tip)| {
                let on = current == Some(appearance);
                let tint = if on { ui.accent } else { ui.text_dim };
                div()
                    .id(SharedString::from(format!("appearance-{glyph}")))
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(on, |d| d.bg(hsla_alpha(ui.accent, 0.2)))
                    .when(!on, |d| d.hover(|s| s.bg(hsla(ui.tab_hover))))
                    .tooltip(crate::menu::tooltip(tip))
                    .child(svg().path(icon(glyph)).size(crate::theme::us(15.0)).text_color(hsla(tint)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.set_appearance(appearance, cx));
                    }))
            }));
        // Cards with previews, or every bench on a line.
        let compact = shown.compact;
        let views = [(false, "view-cards", "Cards with previews"), (true, "view-list", "Compact: a line per bench")];
        let view_switch = div()
            .flex()
            .items_center()
            .p_0p5()
            .rounded_md()
            .border_1()
            .border_color(hsla(ui.divider))
            .children(views.map(|(value, glyph, tip)| {
                let on = compact == value;
                let tint = if on { ui.accent } else { ui.text_dim };
                div()
                    .id(SharedString::from(format!("view-{glyph}")))
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(on, |d| d.bg(hsla_alpha(ui.accent, 0.2)))
                    .when(!on, |d| d.hover(|s| s.bg(hsla(ui.tab_hover))))
                    .tooltip(crate::menu::tooltip(tip))
                    .child(svg().path(icon(glyph)).size(crate::theme::us(15.0)).text_color(hsla(tint)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.set_compact_pegboard(value, cx));
                    }))
            }));
        let banner = self.quitting.then(|| {
            let text = if running == 0 {
                "Closing Shed…".to_string()
            } else {
                format!("Closing Shed: {running} bench{} still closing…", if running == 1 { "" } else { "es" })
            };
            div()
                .flex_none()
                .px_4()
                .py_1()
                .text_size(crate::theme::us(12.0))
                .text_color(hsla(ui.text))
                .bg(hsla(ui.tab_hover))
                .child(text)
        });
        // What the keeper last refused, until dismissed.
        let notice = shown.notice.clone().map(|text| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .mx_4()
                .mt_2()
                .px_3()
                .py_1()
                .rounded_md()
                .text_size(crate::theme::us(12.0))
                .text_color(hsla(ui.text))
                .bg(hsla_alpha(ui.danger, 0.18))
                .child(div().flex_1().child(text))
                .child(
                    div()
                        .id("dismiss-notice")
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla(ui.tab_hover)))
                        .tooltip(crate::menu::tooltip("Dismiss"))
                        .child("×")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.notice = None;
                                cx.notify();
                            });
                        })),
                )
        });
        div()
            .key_context("ShedWindow")
            .size_full()
            .flex()
            .flex_col()
            // Window-sized and drawn first, so the renderer clears to it for free.
            .bg(hsla(ui.background))
            .text_color(hsla(ui.text))
            .on_action(cx.listener(|this, _: &NewBench, _, cx| {
                this.state.read(cx).conn.send_msg(ClientMsg::Show(ShowTarget::NewBench { name: None, cwd: None }));
            }))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .pt_3()
                    .child(svg().path(icon("shed")).size(crate::theme::us(18.0)).text_color(hsla(ui.accent)))
                    .child(div().text_size(crate::theme::us(16.0)).child("Shed"))
                    .child(div().flex_1())
                    .children(self.switcher(cx))
                    .children(self.link_chip(cx))
                    .child(
                        div()
                            .id("open-settings")
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(crate::theme::us(12.0))
                            .text_color(hsla(ui.text_dim))
                            .when(self.settings.is_some(), |d| d.bg(hsla_alpha(ui.accent, 0.2)))
                            .hover(|s| s.bg(hsla(ui.tab_hover)))
                            .tooltip(crate::menu::tooltip("Shed's settings: theme, font, keys, nexus… (shed.toml)"))
                            .child(svg().path(icon("settings")).size(crate::theme::us(14.0)).text_color(hsla(ui.text_dim)))
                            .child("Settings")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
                    )
                    .child(view_switch)
                    .child(switch),
            )
            .children(banner)
            .children(notice)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(self.board.clone())
                    .when_some(self.settings.as_ref().map(|(p, _)| p.clone()), |d, panel| d.child(div().absolute().top_0().right_0().h_full().child(panel)))
                    .children(self.sign_in_dialog(cx)),
            )
            .children(self.menu.as_ref().map(|m| menu::render(m, &ui, cx)))
            // Painted last: how long the frame took (`SHED_PERF`).
            .when(perf, |d| d.child(gpui::canvas(|_, _, _| {}, |_, _, _, _| crate::terminal_view::perf::shed_frame_painted()).absolute().size_0()))
    }
}

impl MenuHost for ShedWindow {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.menu
    }

    fn menu_closed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
    }
}

/// `https://nexus.example.com/` as `nexus.example.com`.
fn short_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    url.split_once("://").map_or(url, |(_, rest)| rest).to_string()
}

impl ShedWindow {
    /// Which workspace the windows show: "This PC ▾", or another reached
    /// through the nexus. Shown once this desktop is signed in to a nexus
    /// (or showing another workspace); its menu also signs in.
    fn switcher(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let shown = &self.shown;
        let ui = shown.ui.clone();
        let remote = shown.workspace != Workspace::Local;
        let label = match &shown.switching {
            Some(name) => format!("Connecting to {name}…"),
            None => format!("{} ▾", shown.workspace.label()),
        };
        let tip = if remote {
            format!("Showing {}'s benches through the nexus; its terminals and AI tabs work here. Click for other workspaces.", shown.workspace.label())
        } else {
            "The benches of this PC. Click for other workspaces, through a nexus.".into()
        };
        let tint = if remote { ui.ink(ui.accent) } else { ui.text_dim };
        Some(
            div()
                .id("workspace-switcher")
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .text_size(crate::theme::us(12.0))
                .text_color(hsla(tint))
                .when(remote, |d| d.bg(hsla_alpha(ui.accent, 0.15)))
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .tooltip(crate::menu::tooltip_text(tip))
                .child(svg().path(icon(if remote { "server" } else { "bench" })).size(crate::theme::us(14.0)).text_color(hsla(tint)))
                .child(label)
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.open_switcher(e.position, cx))),
        )
    }

    /// The sign-in dialog, over the pegboard while open.
    fn sign_in_dialog(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let d = self.sign_in.as_ref()?;
        let ui = self.shown.ui.clone();
        let field = |label: &'static str, editor: &Entity<LineEditor>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(110.0)).flex_none().text_color(hsla(ui.text_dim)).child(label))
                .child(div().flex_1().child(editor.clone()))
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(hsla(if primary { ui.accent } else { ui.divider }))
                .when(primary, |b| b.bg(hsla_alpha(ui.accent, 0.2)).text_color(hsla(ui.ink(ui.accent))))
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .child(label)
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt_16()
                .bg(hsla_alpha(ui.background, 0.6))
                .occlude()
                .child(
                    div()
                        .id("sign-in")
                        .w(px(460.0))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_4()
                        .rounded_md()
                        .border_1()
                        .border_color(hsla(ui.divider))
                        .bg(hsla(ui.tab_active))
                        .text_size(crate::theme::us(13.0))
                        .child(div().text_size(crate::theme::us(15.0)).child("Sign in to a nexus"))
                        .child(
                            div()
                                .text_size(crate::theme::us(12.0))
                                .text_color(hsla(ui.text_dim))
                                .child("To show other workspaces here: those whose shed.toml says desktop_clients = true under [nexus]. This desktop becomes a device of the account (revoke it in the web app's Devices)."),
                        )
                        .child(field("Nexus", &d.url))
                        .child(field("User", &d.user))
                        .child(field("Password", &d.password))
                        .child(field("Code", &d.code))
                        .children(d.error.clone().map(|e| div().text_size(crate::theme::us(12.0)).text_color(hsla(ui.danger)).child(e)))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(button("sign-in-cancel", "Cancel", false).on_click(cx.listener(|this, _, _, cx| {
                                    this.sign_in = None;
                                    cx.notify();
                                })))
                                .child(
                                    button("sign-in-go", if d.busy { "Signing In…" } else { "Sign In" }, true)
                                        .on_click(cx.listener(|this, _, window, cx| this.submit_sign_in(window, cx))),
                                ),
                        ),
                ),
        )
    }

    /// While a nexus is set: whether this machine is linked to it (reachable
    /// from the web and phone apps), switching the link on or off.
    fn link_chip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let shown = &self.shown;
        if shown.nexus_url.trim().is_empty() {
            return None;
        }
        let ui = shown.ui.clone();
        let on = shown.nexus_on;
        let url = shown.nexus_url.clone();
        let (label, color, tip): (String, _, String) = match (&self.link, on) {
            (_, false) => ("Link off".into(), ui.text_dim, "Not reachable from the web and phone apps; click to link again".into()),
            (Some(l), true) if l.state == "connected" => (
                format!("Linked{}", l.name.as_ref().map(|n| format!(" · {n}")).unwrap_or_default()),
                ui.accent,
                format!("Reachable from the web and phone apps through {url}; click to stop"),
            ),
            (Some(l), true) if l.state == "refused" => (
                "Link refused".into(),
                ui.danger,
                format!("{}; pair again with shed nexus join", l.error.clone().unwrap_or_else(|| "The nexus refused this machine".into())),
            ),
            (Some(l), true) => (
                "Linking…".into(),
                ui.text_dim,
                l.error.clone().map(|e| format!("Not connected yet: {e}")).unwrap_or_else(|| format!("Connecting to {url}")),
            ),
            (None, true) => ("Not linked".into(), ui.text_dim, "No link runs: pair this machine with shed nexus join".into()),
        };
        Some(
            div()
                .id("nexus-link")
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .text_size(crate::theme::us(12.0))
                .text_color(hsla(if color == ui.accent { ui.ink(color) } else { color }))
                .when(on && color == ui.accent, |d| d.bg(hsla_alpha(ui.accent, 0.15)))
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .tooltip(crate::menu::tooltip_text(tip))
                .child(svg().path(icon("globe")).size(crate::theme::us(14.0)).text_color(hsla(if color == ui.accent { ui.ink(color) } else { color })))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| state.set_config_value(Some("nexus"), "enabled", toml_edit::Value::from(!on), cx));
                })),
        )
    }
}

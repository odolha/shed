//! A bench's window: the bench-colour stripe, the tab bar, and the active
//! tab's panes laid out by its split tree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, PromptLevel, SharedString, Subscription, Window, canvas,
    div, prelude::*, px, relative, svg,
};
use shed_model::{
    Bench, BenchColor, BenchId, Direction, PaneId, PaneNode, Rgb, SplitDir, Tab, TabId, Tag,
    default_tag_color, tag_color,
};
use shed_model::config::NewPaneDir;
use shed_proto::ClientMsg;

use crate::actions::*;
use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu::{self, Menu, MenuEntry, MenuHost, danger, swatch};
use crate::setup_panel::{PanelEvent, SetupPanel};
use crate::state::AppState;
use crate::theme::{UiColors, hsla, hsla_alpha};

const TAB_BAR_HEIGHT: f32 = 32.0;

/// What the inline text field is editing.
#[derive(Clone, Copy, PartialEq)]
enum EditTarget {
    TabName(TabId),
    BenchName,
    AddTag(TabId),
    Badge,
}

/// `tags` with the one `text` describes added: "prod", or "prod red".
pub fn with_tag(tags: &[Tag], text: &str) -> Option<Vec<Tag>> {
    let mut words = text.split_whitespace();
    let label = words.next()?;
    let color = words.next().and_then(tag_color).unwrap_or_else(|| default_tag_color(label));
    let mut tags: Vec<Tag> = tags.iter().filter(|t| t.label != label).cloned().collect();
    tags.push(Tag { label: label.to_string(), color, tint_tab: false });
    Some(tags)
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
    /// The pane last given keyboard focus, so focus follows the keeper's
    /// active pane without fighting the user's clicks.
    focused: Option<PaneId>,
    title: String,
    editing: Option<(EditTarget, Entity<LineEditor>, Subscription)>,
    menu: Option<Menu<Self>>,
    /// Where each split container was laid out last frame, by path in the tree.
    split_bounds: Rc<RefCell<HashMap<Vec<bool>, Bounds<Pixels>>>>,
    drag: Option<SplitDrag>,
    /// The Setup panel, while open.
    setup: Option<(Entity<SetupPanel>, Subscription)>,
}

impl BenchWindow {
    pub fn new(bench: BenchId, state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let closing = state.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            closing.update(cx, |state, _| state.window_closed(bench)).ok();
            true
        });
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            bench,
            state,
            ui: UiColors::default(),
            focused: None,
            title: String::new(),
            editing: None,
            menu: None,
            split_bounds: Rc::default(),
            drag: None,
            setup: None,
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
    pub fn sync(&mut self, title: String, active: Option<(PaneId, FocusHandle)>, window: &mut Window, cx: &mut Context<Self>) {
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
        let state = self.state.read(cx);
        let Some(pane) = state.tree.bench(self.bench).and_then(|b| b.active_tab()).map(|t| t.active_pane) else { return };
        if let Some(view) = state.panes.get(&pane) {
            let handle = view.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        }
        self.focused = Some(pane);
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

    fn close_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench(cx) else { return };
        let message = format!("Close the bench “{}”?", bench.name);
        let name = bench.name.clone();
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(
                "Closing stops every tool in it and keeps it saved, to load again later. \
                 Deleting also removes the saved bench and its launcher.\n\n\
                 To keep the tools running, close the window instead.",
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

    fn start_edit(&mut self, target: EditTarget, window: &mut Window, cx: &mut Context<Self>) {
        let current = match target {
            EditTarget::TabName(tab) => self.tab(tab, cx).map(|t| t.display_title()),
            EditTarget::BenchName => self.bench(cx).map(|b| b.name.clone()),
            EditTarget::AddTag(_) => Some(String::new()),
            EditTarget::Badge => self.bench(cx).map(|b| b.badge.clone().unwrap_or_default()),
        };
        let Some(current) = current else { return };
        let editor = cx.new(|cx| LineEditor::selected(current, cx));
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
            EditTarget::Badge => {
                let badge: String = text.chars().filter(|c| !c.is_whitespace()).take(2).collect();
                ClientMsg::SetBadge { bench: self.bench, badge: (!badge.is_empty()).then_some(badge) }
            }
            EditTarget::AddTag(tab) => {
                let Some(t) = self.tab(tab, cx) else { return };
                let Some(tags) = with_tag(&t.tags, &text) else { return };
                ClientMsg::SetTags { tab, tags }
            }
        };
        self.send(msg, cx);
    }

    fn tab_menu(&self, tab_id: TabId, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some(tab) = self.tab(tab_id, cx) else { return Vec::new() };
        let mut entries = vec![
            item("Rename Tab…", move |this, window, cx| this.start_edit(EditTarget::TabName(tab_id), window, cx)).icon("pencil"),
            item("Add Tag…", move |this, window, cx| this.start_edit(EditTarget::AddTag(tab_id), window, cx)).icon("tag"),
            item("Setup…", move |this, window, cx| {
                this.send(ClientMsg::FocusTab { tab: tab_id }, cx);
                if this.setup.is_none() {
                    this.toggle_setup(window, cx);
                }
            })
            .icon("rack"),
        ];
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
        // Moving a tab to another bench is the pegboard's: drag it there.
        entries.push(MenuEntry::Separator);
        entries.push(danger("Close Tab", move |this: &mut Self, _, cx| this.send(ClientMsg::CloseTab { tab: tab_id }, cx)).icon("close"));
        entries
    }

    fn pane_menu(&self, pane: PaneId) -> Vec<MenuEntry<Self>> {
        vec![
            item("Copy", |_, window, cx| window.dispatch_action(Box::new(Copy), cx)),
            item("Paste", |_, window, cx| window.dispatch_action(Box::new(Paste), cx)),
            MenuEntry::Separator,
            item("Split Right", move |this, _, cx| this.split_pane(pane, SplitDir::Right, cx)).icon("split-right"),
            item("Split Down", move |this, _, cx| this.split_pane(pane, SplitDir::Down, cx)).icon("split-down"),
            MenuEntry::Separator,
            danger("Close Pane", move |this: &mut Self, _, cx| this.send(ClientMsg::ClosePane { pane }, cx)).icon("close"),
        ]
    }

    fn bench_menu(&self, cx: &Context<Self>) -> Vec<MenuEntry<Self>> {
        let current = self.bench(cx).map(|b| b.color);
        let mut entries = vec![
            item("Rename Bench…", |this, window, cx| this.start_edit(EditTarget::BenchName, window, cx)).icon("pencil"),
            item("Set Badge…", |this, window, cx| this.start_edit(EditTarget::Badge, window, cx)),
            MenuEntry::Separator,
        ];
        for color in BenchColor::NAMED {
            let name = color.name().unwrap_or_default();
            let mark = if current == Some(color) { "✓ " } else { "" };
            entries.push(swatch(format!("{mark}{}{}", name[..1].to_uppercase(), &name[1..]), color.accent(), move |this: &mut Self, _, cx| {
                    this.send(ClientMsg::SetBenchColor { bench: this.bench, color }, cx);
            }));
        }
        entries.push(MenuEntry::Separator);
        entries.push(item("Show Shed", |this, _, cx| this.state.update(cx, |state, cx| state.show_shed(cx))).icon("shed"));
        entries.push(danger("Close Bench…", |this: &mut Self, window, cx| this.close_bench(window, cx)).icon("power"));
        entries
    }

    fn editor_for(&self, target: EditTarget) -> Option<Entity<LineEditor>> {
        self.editing.as_ref().filter(|(t, _, _)| *t == target).map(|(_, e, _)| e.clone())
    }

    fn render_tab_bar(&self, bench: &Bench, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = &self.ui;
        let accent = bench.color.accent();
        let tab_count = bench.tabs.len();
        let mut bar = div()
            .id("tab-bar")
            .flex()
            .flex_none()
            .h(px(TAB_BAR_HEIGHT))
            .bg(hsla(ui.background))
            .text_size(px(13.0))
            .text_color(hsla(ui.text_dim));

        for (i, tab) in bench.tabs.iter().enumerate() {
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
                .h_full()
                .pl_3()
                .pr_1()
                .max_w(px(260.0))
                .border_r_1()
                .border_color(hsla(ui.divider))
                .cursor_pointer()
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_edit(EditTarget::TabName(tab_id), window, cx);
                    } else {
                        this.send(ClientMsg::FocusTab { tab: tab_id }, cx);
                    }
                }))
                .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, _, cx| {
                    this.send(ClientMsg::CloseTab { tab: tab_id }, cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let entries = this.tab_menu(tab_id, cx);
                    this.open_menu(e.position, entries, cx);
                }))
                .on_drag(DraggedTab { tab: tab_id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<DraggedTab>(move |s, _, _, _| s.border_l_2().border_color(hsla(accent)))
                .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                    this.send(ClientMsg::MoveTab { tab: d.tab, bench: this.bench, index: i }, cx);
                }));
            el = match (active, tint) {
                (true, Some(t)) => el.bg(hsla_alpha(t, 0.35)).text_color(hsla(ui.text)),
                (false, Some(t)) => el.bg(hsla_alpha(t, 0.18)).hover(|s| s.bg(hsla_alpha(t, 0.25))),
                (true, None) => el.bg(hsla(ui.tab_active)).text_color(hsla(ui.text)),
                (false, None) => el.hover(|s| s.bg(hsla(ui.tab_hover))),
            };
            if active {
                // The active tab carries the bench colour along its top.
                el = el.border_t_2().border_color(hsla(accent));
            }
            if tab.panes.iter().any(|p| p.waiting.is_some()) {
                // A loaded bench typed a command here; it waits for Enter.
                el = el.child(svg().path(icon("tool-waiting")).size(px(12.0)).flex_none().text_color(hsla(accent)));
            } else if tab.panes.iter().any(|p| p.tool.is_some()) {
                el = el.child(svg().path(icon("tool")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)));
            }
            if tab.private {
                el = el.child(svg().path(icon("private")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)));
            }
            match self.editor_for(EditTarget::TabName(tab_id)) {
                Some(editor) => el = el.child(editor),
                None => el = el.child(div().truncate().child(title)),
            }
            for tag in tab.all_tags() {
                el = el.child(
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
            if let Some(editor) = self.editor_for(EditTarget::AddTag(tab_id)) {
                el = el.child(editor);
            }
            el = el.child(
                div()
                    .id(("close-tab", i))
                    .flex_none()
                    .p_0p5()
                    .rounded_sm()
                    .invisible()
                    .group_hover("tab", |s| s.visible())
                    .hover(|s| s.bg(hsla(ui.divider)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.send(ClientMsg::CloseTab { tab: tab_id }, cx);
                    }))
                    .child(svg().path(icon("close")).size(px(12.0)).text_color(hsla(ui.text_dim))),
            );
            bar = bar.child(el);
        }

        bar.child(
            div()
                .id("new-tab")
                .flex()
                .items_center()
                .px_2()
                .cursor_pointer()
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .on_click(cx.listener(|this, _, _, cx| this.new_tab(cx)))
                .child(svg().path(icon("tab-new")).size(px(16.0)).text_color(hsla(ui.text_dim))),
        )
        .child(
            // Dropping a tab on the empty part of the bar moves it to the end.
            div()
                .id("tab-bar-end")
                .flex_1()
                .h_full()
                .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                    this.send(ClientMsg::MoveTab { tab: d.tab, bench: this.bench, index: tab_count }, cx);
                })),
        )
        .child(
            div()
                .id("shed-button")
                .flex()
                .items_center()
                .px_2()
                .cursor_pointer()
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .tooltip(menu::tooltip("Show Shed: every bench"))
                .on_click(cx.listener(|this, _, _, cx| this.state.update(cx, |state, cx| state.show_shed(cx))))
                .child(svg().path(icon("shed")).size(px(16.0)).text_color(hsla(ui.text_dim))),
        )
        .child(
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
                .child(svg().path(icon("rack")).size(px(16.0)).text_color(hsla(if self.setup.is_some() { accent } else { ui.text_dim }))),
        )
        .child(self.render_bench_label(bench, cx))
    }

    fn render_bench_label(&self, bench: &Bench, cx: &mut Context<Self>) -> AnyElement {
        if let Some(editor) = self.editor_for(EditTarget::BenchName).or_else(|| self.editor_for(EditTarget::Badge)) {
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
            .text_color(hsla(accent))
            .hover(|s| s.bg(hsla(self.ui.tab_hover)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu(cx);
                this.open_menu(e.position, entries, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let entries = this.bench_menu(cx);
                this.open_menu(e.position, entries, cx);
            }))
            .child(svg().path(icon("bench")).size(px(14.0)).text_color(hsla(accent)))
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
                        let entries = this.pane_menu(pane);
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
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| {
                if let Some(tab) = this.active_tab(cx).map(|t| t.id) {
                    this.send(ClientMsg::CloseTab { tab }, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ClosePane, _, cx| {
                if let Some(pane) = this.active_tab(cx).map(|t| t.active_pane) {
                    this.send(ClientMsg::ClosePane { pane }, cx);
                }
            }))
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
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                if let Some(tab) = this.active_tab(cx).map(|t| t.id) {
                    this.start_edit(EditTarget::TabName(tab), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RenameBench, window, cx| this.start_edit(EditTarget::BenchName, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSetup, window, cx| this.toggle_setup(window, cx)))
            .on_action(cx.listener(|this, _: &ShowShed, _, cx| this.state.update(cx, |state, cx| state.show_shed(cx))))
            .on_action(cx.listener(|this, a: &FocusBench, _, cx| {
                if let Some(handle) = this.state.read(cx).window_at(a.0) {
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


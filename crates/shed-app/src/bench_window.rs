//! A bench's window: the bench-colour stripe, the tab bar, and the active
//! tab's panes laid out by its split tree.

use gpui::{
    AnyElement, Context, Entity, Focusable, MouseButton, PromptLevel, Subscription, Window, div, prelude::*,
    px, relative, svg,
};
use gpui::FocusHandle;
use shed_model::{Bench, BenchId, Direction, PaneId, PaneNode, SplitDir, Tab, TabId};
use shed_proto::{ClientMsg, ShowTarget};

use crate::actions::*;
use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::state::AppState;
use crate::theme::{UiColors, hsla, hsla_alpha};

const TAB_BAR_HEIGHT: f32 = 32.0;

enum RenameTarget {
    Tab(TabId),
    Bench,
}

pub struct BenchWindow {
    bench: BenchId,
    state: Entity<AppState>,
    ui: UiColors,
    /// The pane last given keyboard focus, so focus follows the keeper's
    /// active pane without fighting the user's clicks.
    focused: Option<PaneId>,
    title: String,
    renaming: Option<(RenameTarget, Entity<LineEditor>, Subscription)>,
}

impl BenchWindow {
    pub fn new(bench: BenchId, state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let closing = state.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            closing.update(cx, |state, _| state.window_closed(bench)).ok();
            true
        });
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { bench, state, ui: UiColors::default(), focused: None, title: String::new(), renaming: None }
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
        if active_id != self.focused && self.renaming.is_none() {
            if let Some((_, handle)) = &active {
                window.focus(handle, cx);
            }
            self.focused = active_id;
        }
        cx.notify();
    }

    /// Re-applies focus after an overlay (the rename field) closes.
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

    fn cycle_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(bench) = self.bench(cx) else { return };
        let Some(i) = bench.tab_index(bench.active_tab) else { return };
        let n = bench.tabs.len() as isize;
        let tab = bench.tabs[(i as isize + delta).rem_euclid(n) as usize].id;
        self.send(ClientMsg::FocusTab { tab }, cx);
    }

    fn split(&mut self, dir: SplitDir, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab(cx) else { return };
        let pane = tab.active_pane;
        let cwd = tab.active().map(|p| p.cwd.clone());
        self.send(ClientMsg::Split { pane, dir, cwd }, cx);
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
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("Every tool running in it will be stopped. To keep them running, close the window instead."),
            &["Close Bench", "Cancel"],
            cx,
        );
        let (bench, conn) = (self.bench, self.state.read(cx).conn.clone());
        cx.spawn(async move |_, _| {
            if answer.await == Ok(0) {
                conn.send_msg(ClientMsg::CloseBench { bench });
            }
        })
        .detach();
    }

    fn start_rename(&mut self, target: RenameTarget, window: &mut Window, cx: &mut Context<Self>) {
        let current = match &target {
            RenameTarget::Tab(tab) => self.bench(cx).and_then(|b| b.tabs.iter().find(|t| t.id == *tab)).map(|t| t.display_title()),
            RenameTarget::Bench => self.bench(cx).map(|b| b.name.clone()),
        };
        let Some(current) = current else { return };
        let editor = cx.new(|cx| LineEditor::new(current, cx));
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let sub = cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, window, cx| {
            let Some((target, _, _)) = this.renaming.take() else { return };
            if let EditorEvent::Submit(text) = event {
                let msg = match target {
                    RenameTarget::Tab(tab) => ClientMsg::RenameTab { tab, name: Some(text.clone()) },
                    RenameTarget::Bench => ClientMsg::RenameBench { bench: this.bench, name: text.clone() },
                };
                this.send(msg, cx);
            }
            this.refocus(window, cx);
        });
        self.renaming = Some((target, editor, sub));
        cx.notify();
    }

    fn render_tab_bar(&self, bench: &Bench, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = &self.ui;
        let accent = bench.color.accent();
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
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(RenameTarget::Tab(tab_id), window, cx);
                    } else {
                        this.send(ClientMsg::FocusTab { tab: tab_id }, cx);
                    }
                }))
                .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, _, cx| {
                    this.send(ClientMsg::CloseTab { tab: tab_id }, cx);
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
            if tab.panes.iter().any(|p| p.tool.is_some()) {
                el = el.child(svg().path(icon("tool")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)));
            }
            if tab.private {
                el = el.child(svg().path(icon("private")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)));
            }

            let renaming_this = matches!(&self.renaming, Some((RenameTarget::Tab(t), _, _)) if *t == tab_id);
            if let (true, Some((_, editor, _))) = (renaming_this, &self.renaming) {
                el = el.child(editor.clone());
            } else {
                el = el.child(div().truncate().child(tab.display_title()));
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

        let bench_id = bench.id;
        bar.child(
            div()
                .id("new-tab")
                .flex()
                .items_center()
                .px_2()
                .cursor_pointer()
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .on_click(cx.listener(move |this, _, _, cx| this.send(ClientMsg::NewTab { bench: bench_id, cwd: None }, cx)))
                .child(svg().path(icon("tab-new")).size(px(16.0)).text_color(hsla(ui.text_dim))),
        )
        .child(div().flex_1())
        .child(self.render_bench_label(bench, cx))
    }

    fn render_bench_label(&self, bench: &Bench, cx: &mut Context<Self>) -> AnyElement {
        if let Some((RenameTarget::Bench, editor, _)) = &self.renaming {
            return div().flex().items_center().px_2().child(editor.clone()).into_any_element();
        }
        div()
            .id("bench-label")
            .flex()
            .items_center()
            .gap_1p5()
            .px_3()
            .text_color(hsla(bench.color.accent()))
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.start_rename(RenameTarget::Bench, window, cx);
                }
            }))
            .child(svg().path(icon("bench")).size(px(14.0)).text_color(hsla(bench.color.accent())))
            .child(bench.name.clone())
            .when_some(bench.badge.clone(), |el, badge| {
                el.child(div().px_1().rounded_sm().bg(hsla_alpha(bench.color.accent(), 0.25)).child(badge))
            })
            .into_any_element()
    }

    fn render_node(&self, node: &PaneNode, tab: &Tab, accent: shed_model::Rgb, cx: &Context<Self>) -> AnyElement {
        match node {
            PaneNode::Leaf(id) => {
                let split = tab.panes.len() > 1;
                let active = *id == tab.active_pane;
                let border = if split && active { hsla(accent) } else { hsla_alpha(accent, 0.0) };
                let mut cell = div().size_full().min_w_0().min_h_0().overflow_hidden();
                if split {
                    cell = cell.border_1().border_color(border);
                }
                match self.state.read(cx).panes.get(id) {
                    Some(view) => cell.child(view.clone()).into_any_element(),
                    None => cell.into_any_element(),
                }
            }
            PaneNode::Split { dir, ratio, first, second } => {
                let row = *dir == SplitDir::Right;
                let first_el = div()
                    .flex_none()
                    .min_w_0()
                    .min_h_0()
                    .map(|d| if row { d.h_full().w(relative(*ratio)) } else { d.w_full().h(relative(*ratio)) })
                    .child(self.render_node(first, tab, accent, cx));
                let divider = div()
                    .flex_none()
                    .bg(hsla(self.ui.divider))
                    .map(|d| if row { d.w(px(1.0)).h_full() } else { d.h(px(1.0)).w_full() });
                let second_el = div().flex_1().min_w_0().min_h_0().child(self.render_node(second, tab, accent, cx));
                div()
                    .size_full()
                    .flex()
                    .map(|d| if row { d.flex_row() } else { d.flex_col() })
                    .child(first_el)
                    .child(divider)
                    .child(second_el)
                    .into_any_element()
            }
        }
    }
}

impl Render for BenchWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(bench) = self.bench(cx).cloned() else {
            return div().size_full().bg(hsla(self.ui.background)).into_any_element();
        };
        let accent = bench.color.accent();
        let content = match bench.active_tab() {
            Some(tab) => self.render_node(&tab.layout, tab, accent, cx),
            None => div().into_any_element(),
        };

        div()
            .key_context("BenchWindow")
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(self.ui.background))
            .on_action(cx.listener(|this, _: &NewTab, _, cx| {
                this.send(ClientMsg::NewTab { bench: this.bench, cwd: None }, cx);
            }))
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
            .on_action(cx.listener(|this, _: &NewBench, _, cx| {
                this.send(ClientMsg::Show(ShowTarget::NewBench { name: None, cwd: None }), cx);
            }))
            .on_action(cx.listener(|this, _: &CloseBench, window, cx| this.close_bench(window, cx)))
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                if let Some(tab) = this.active_tab(cx).map(|t| t.id) {
                    this.start_rename(RenameTarget::Tab(tab), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RenameBench, window, cx| this.start_rename(RenameTarget::Bench, window, cx)))
            .on_action(cx.listener(|this, a: &FocusBench, _, cx| {
                if let Some(handle) = this.state.read(cx).window_at(a.0) {
                    handle.update(cx, |_, window, _| window.activate()).ok();
                }
            }))
            .child(div().h(px(3.0)).w_full().flex_none().bg(hsla(accent)))
            .child(self.render_tab_bar(&bench, cx))
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }
}

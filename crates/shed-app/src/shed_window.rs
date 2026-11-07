//! The Shed window: the pegboard, with the Shed icon. It lives as long as
//! Shed does; closing it closes every bench and Shed with them.

use gpui::{Context, Entity, PromptLevel, SharedString, Subscription, Window, div, prelude::*, px, svg};
use shed_model::config::Appearance;
use shed_proto::{ClientMsg, ShowTarget};

use crate::actions::NewBench;
use crate::assets::icon;
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
    _following: Subscription,
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
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        // `theme = "auto"` follows the desktop as it switches.
        cx.observe_window_appearance(window, |this, window, cx| {
            let dark = is_dark(window.appearance());
            this.state.update(cx, |state, cx| state.set_dark(dark, cx));
        })
        .detach();
        let board = cx.new(|cx| Pegboard::new(state.clone(), window, cx));
        // SHED_PERF_FRAMES=<n>: redraw n times a second, as hovering does, to
        // measure what a frame of this window costs.
        if let Some(fps) = std::env::var("SHED_PERF_FRAMES").ok().and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0) {
            cx.spawn_in(window, async move |_, cx| loop {
                cx.background_executor().timer(std::time::Duration::from_millis(1000 / fps)).await;
                if cx.update(|window, _| window.refresh()).is_err() {
                    break;
                }
            })
            .detach();
        }
        let following = PlaceTracker::follow(window, cx);
        Self { state, board, quitting: false, place, _following: following }
    }

    /// Closing Shed closes every bench; asks first if any is running.
    pub fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        // Every window's place, so all open the same next time.
        self.place.record(window);
        self.state.update(cx, |state, cx| state.record_places(cx));
        let running = self.state.read(cx).tree.benches.len();
        if running == 0 {
            self.quit(cx);
            return;
        }
        let benches = if running == 1 { "its running bench".to_string() } else { format!("its {running} running benches") };
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
        self.state.read(cx).conn.send_msg(ClientMsg::QuitShed);
        cx.notify();
    }
}

impl Render for ShedWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let ui = state.settings.ui.clone();
        let running = state.tree.benches.len();
        let current = state.settings.config.appearance();
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
                    .child(svg().path(icon(glyph)).size(px(15.0)).text_color(hsla(tint)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.set_appearance(appearance, cx));
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
                .text_size(px(12.0))
                .text_color(hsla(ui.text))
                .bg(hsla(ui.tab_hover))
                .child(text)
        });
        // What the keeper last refused, until dismissed.
        let notice = state.notice.clone().map(|text| {
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
                .text_size(px(12.0))
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
                    .child(svg().path(icon("shed")).size(px(18.0)).text_color(hsla(ui.accent)))
                    .child(div().text_size(px(16.0)).child("Shed"))
                    .child(div().flex_1())
                    .child(switch),
            )
            .children(banner)
            .children(notice)
            .child(div().flex_1().min_h_0().child(self.board.clone()))
    }
}

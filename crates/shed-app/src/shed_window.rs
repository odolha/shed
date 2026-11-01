//! The Shed window: the pegboard, with the Shed icon. It lives as long as
//! Shed does; closing it closes every bench and Shed with them.

use gpui::{Context, Entity, PromptLevel, SharedString, Window, div, prelude::*, px, svg};
use shed_model::config::Appearance;
use shed_proto::{ClientMsg, ShowTarget};

use crate::actions::NewBench;
use crate::assets::icon;
use crate::pegboard::Pegboard;
use crate::state::AppState;
use crate::theme::{hsla, hsla_alpha, is_dark};

pub struct ShedWindow {
    state: Entity<AppState>,
    board: Entity<Pegboard>,
    /// Shed is closing: benches are being closed, then it quits.
    quitting: bool,
}

impl ShedWindow {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        Self { state, board, quitting: false }
    }

    /// Closing Shed closes every bench; asks first if any is running.
    fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
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
                 To keep a bench running, leave Shed open; closing a bench's window only hides it.",
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
        let switch = div().flex().items_center().gap_1().text_size(px(12.0)).children(
            [(Appearance::Auto, "Auto"), (Appearance::Light, "Light"), (Appearance::Dark, "Dark")].map(|(appearance, label)| {
                let on = current == Some(appearance);
                div()
                    .id(SharedString::from(format!("appearance-{label}")))
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(on, |d| d.bg(hsla_alpha(ui.accent_fallback, 0.2)).text_color(hsla(ui.accent_fallback)))
                    .when(!on, |d| d.text_color(hsla(ui.text_dim)).hover(|s| s.bg(hsla(ui.tab_hover))))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.set_appearance(appearance, cx));
                    }))
            }),
        );
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
                    .child(svg().path(icon("shed")).size(px(18.0)).text_color(hsla(ui.accent_fallback)))
                    .child(div().text_size(px(16.0)).child("Shed"))
                    .child(div().flex_1())
                    .child(div().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child("Appearance"))
                    .child(switch),
            )
            .children(banner)
            .child(div().flex_1().min_h_0().child(self.board.clone()))
    }
}

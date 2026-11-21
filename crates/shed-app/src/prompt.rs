//! Confirmation dialogs in Shed's own colours, replacing GPUI's default.

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Global, KeyDownEvent, PromptButton,
    PromptHandle, PromptLevel, PromptResponse, RenderablePromptHandle, Window, div, prelude::*, px,
};
use shed_model::Rgb;

use crate::theme::{UiColors, hsla, hsla_alpha};

/// The chrome colours of the current theme, for views built outside a window's tree.
pub struct CurrentUi(pub UiColors);

impl Global for CurrentUi {}

pub fn install(cx: &mut App) {
    cx.set_prompt_builder(build);
}

fn build(
    level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    actions: &[PromptButton],
    handle: PromptHandle,
    window: &mut Window,
    cx: &mut App,
) -> RenderablePromptHandle {
    let ui = cx.try_global::<CurrentUi>().map(|u| u.0.clone()).unwrap_or_default();
    let view = cx.new(|cx| ShedPrompt {
        message: message.to_string(),
        detail: detail.map(str::to_string),
        actions: actions.iter().map(|a| a.label().to_string()).collect(),
        critical: level == PromptLevel::Critical,
        focus: cx.focus_handle(),
        ui,
    });
    handle.with_view(view, window, cx)
}

struct ShedPrompt {
    message: String,
    detail: Option<String>,
    actions: Vec<String>,
    /// What cannot easily be undone: bigger, and Enter cancels.
    critical: bool,
    focus: FocusHandle,
    ui: UiColors,
}

impl EventEmitter<PromptResponse> for ShedPrompt {}

impl Focusable for ShedPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ShedPrompt {
    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // The last button is the safe one: Cancel.
        let cancel = self.actions.len().saturating_sub(1);
        match event.keystroke.key.as_str() {
            "enter" => cx.emit(PromptResponse(if self.critical { cancel } else { 0 })),
            "escape" => cx.emit(PromptResponse(cancel)),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for ShedPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = &self.ui;
        let danger = ui.danger;
        let last = self.actions.len().saturating_sub(1);
        let critical = self.critical;
        let buttons = self.actions.iter().enumerate().map(|(i, label)| {
            // ("Close Here" only lets go of a remote shell, which keeps running.)
            let destructive = ["Delete", "Close", "Remove", "End"].iter().any(|word| label.starts_with(word)) && label != "Close Here";
            let color = if destructive { danger } else { ui.text };
            div()
                .id(i)
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                // (Critical: Enter takes Cancel, so it looks the default.)
                .border_color(hsla(if critical && i == last { ui.text_dim } else { ui.divider }))
                .text_color(hsla(color))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .child(label.clone())
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(PromptResponse(i));
                    cx.stop_propagation();
                }))
        });
        let dialog = div()
            .w(px(if self.critical { 500.0 } else { 420.0 }))
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .rounded_lg()
            .bg(hsla(ui.tab_active))
            .border_1()
            .border_color(hsla(ui.divider))
            .text_color(hsla(ui.text))
            .text_size(px(14.0))
            .when(self.critical, |d| d.border_color(hsla(danger)))
            .child(div().text_size(px(if self.critical { 17.0 } else { 15.0 })).when(self.critical, |d| d.text_color(hsla(danger))).child(self.message.clone()))
            .children(self.detail.clone().map(|d| div().text_size(px(13.0)).text_color(hsla(if self.critical { ui.text } else { ui.text_dim })).child(d)))
            .child(div().mt_2().flex().justify_end().gap_2().children(buttons));

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .occlude()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla_alpha(Rgb::hex(0x000000), 0.45))
            .child(dialog)
    }
}

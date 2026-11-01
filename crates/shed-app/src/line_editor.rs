//! A one-line text field, for renaming tabs and benches.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Window, div, prelude::*, px,
};

use crate::theme::{UiColors, hsla};

pub enum EditorEvent {
    Submit(String),
    Cancel,
}

pub struct LineEditor {
    text: String,
    /// Cursor position in characters.
    cursor: usize,
    focus: FocusHandle,
}

impl EventEmitter<EditorEvent> for LineEditor {}

impl Focusable for LineEditor {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LineEditor {
    pub fn new(text: String, cx: &mut Context<Self>) -> Self {
        let cursor = text.chars().count();
        Self { text, cursor, focus: cx.focus_handle() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn byte(&self, chars: usize) -> usize {
        self.text.char_indices().nth(chars).map_or(self.text.len(), |(i, _)| i)
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let len = self.text.chars().count();
        match ks.key.as_str() {
            "enter" => cx.emit(EditorEvent::Submit(self.text.trim().to_string())),
            "escape" => cx.emit(EditorEvent::Cancel),
            "backspace" if self.cursor > 0 => {
                let (a, b) = (self.byte(self.cursor - 1), self.byte(self.cursor));
                self.text.replace_range(a..b, "");
                self.cursor -= 1;
            }
            "delete" if self.cursor < len => {
                let (a, b) = (self.byte(self.cursor), self.byte(self.cursor + 1));
                self.text.replace_range(a..b, "");
            }
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => self.cursor = (self.cursor + 1).min(len),
            "home" => self.cursor = 0,
            "end" => self.cursor = len,
            "v" if ks.modifiers.control => {
                let Some(pasted) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
                let pasted = pasted.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
                let at = self.byte(self.cursor);
                self.text.insert_str(at, &pasted);
                self.cursor += pasted.chars().count();
            }
            _ => match ks.key_char.as_deref() {
                Some(text) if !ks.modifiers.control && !ks.modifiers.platform && !text.chars().any(char::is_control) => {
                    let at = self.byte(self.cursor);
                    self.text.insert_str(at, text);
                    self.cursor += text.chars().count();
                }
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for LineEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = UiColors::default();
        let at = self.byte(self.cursor);
        let (before, after) = self.text.split_at(at);
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .flex()
            .items_center()
            .px_1()
            .min_w(px(80.0))
            .bg(hsla(ui.background))
            .border_1()
            .border_color(hsla(ui.accent_fallback))
            .rounded_sm()
            .text_color(hsla(ui.text))
            .child(before.to_string())
            .child(div().w(px(1.5)).h(px(14.0)).bg(hsla(ui.text)))
            .child(after.to_string())
    }
}

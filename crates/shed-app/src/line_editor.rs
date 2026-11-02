//! A one-line text field, for renaming tabs and benches.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Subscription, Window, div, prelude::*, px,
};

use crate::theme::hsla;

pub enum EditorEvent {
    Submit(String),
    Cancel,
}

pub struct LineEditor {
    text: String,
    /// All the text is selected: typing replaces it.
    all_selected: bool,
    /// Cursor position in characters.
    cursor: usize,
    focus: FocusHandle,
    /// Shown dimmed beside the field: how to finish.
    hint: Option<&'static str>,
    _blur: Option<Subscription>,
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
        Self { text, all_selected: false, cursor, focus: cx.focus_handle(), hint: None, _blur: None }
    }

    /// For renaming and the like: clicking or typing elsewhere keeps what was
    /// typed, as Enter does, so the field never stays open behind the user.
    pub fn submit_on_blur(mut self, window: &mut Window, cx: &mut Context<Self>) -> Self {
        self._blur = Some(cx.on_blur(&self.focus, window, |this, _, cx| cx.emit(EditorEvent::Submit(this.text.trim().to_string()))));
        self.hint = Some("Enter saves · Esc cancels");
        self
    }

    /// Without the hint beside the field, where there is no room for it.
    pub fn without_hint(mut self) -> Self {
        self.hint = None;
        self
    }

    /// With the text selected, as for renaming: typing replaces it.
    pub fn selected(text: String, cx: &mut Context<Self>) -> Self {
        Self { all_selected: !text.is_empty(), ..Self::new(text, cx) }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.text.clear();
        self.cursor = 0;
        self.all_selected = false;
        cx.notify();
    }

    /// Typing or deleting over a selection takes it all away first.
    fn replace_selection(&mut self) {
        if std::mem::take(&mut self.all_selected) {
            self.text.clear();
            self.cursor = 0;
        }
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
            "a" if ks.modifiers.control => self.all_selected = !self.text.is_empty(),
            "backspace" | "delete" if self.all_selected => self.replace_selection(),
            "backspace" if self.cursor > 0 => {
                let (a, b) = (self.byte(self.cursor - 1), self.byte(self.cursor));
                self.text.replace_range(a..b, "");
                self.cursor -= 1;
            }
            "delete" if self.cursor < len => {
                let (a, b) = (self.byte(self.cursor), self.byte(self.cursor + 1));
                self.text.replace_range(a..b, "");
            }
            "left" | "home" if std::mem::take(&mut self.all_selected) => self.cursor = 0,
            "right" | "end" if std::mem::take(&mut self.all_selected) => self.cursor = len,
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => self.cursor = (self.cursor + 1).min(len),
            "home" => self.cursor = 0,
            "end" => self.cursor = len,
            "v" if ks.modifiers.control => {
                let Some(pasted) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
                let pasted = pasted.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
                self.replace_selection();
                let at = self.byte(self.cursor);
                self.text.insert_str(at, &pasted);
                self.cursor += pasted.chars().count();
            }
            _ => match ks.key_char.as_deref() {
                Some(text) if !ks.modifiers.control && !ks.modifiers.platform && !text.chars().any(char::is_control) => {
                    self.replace_selection();
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
        let ui = crate::theme::current_ui(cx);
        let at = self.byte(self.cursor);
        let (before, after) = self.text.split_at(at);
        let selection = self.all_selected.then(|| div().rounded_sm().bg(crate::theme::hsla_alpha(ui.accent, 0.3)).child(self.text.clone()));
        let (before, after) = if self.all_selected { (String::new(), String::new()) } else { (before.to_string(), after.to_string()) };
        let field = div()
            .flex()
            .flex_1()
            .items_center()
            .px_1()
            .min_w(px(80.0))
            .bg(hsla(ui.background))
            .border_1()
            .border_color(hsla(ui.accent))
            .rounded_sm()
            .text_color(hsla(ui.text))
            .children(selection)
            .child(before)
            .child(div().w(px(1.5)).h(px(14.0)).bg(hsla(ui.text)))
            .child(after);
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .flex()
            .items_center()
            .gap_2()
            .child(field)
            .children(self.hint.map(|hint| div().flex_none().text_size(px(11.0)).text_color(hsla(ui.text_dim)).child(hint)))
    }
}

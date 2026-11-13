//! Shed's settings, over the right side of the Shed window: the ones most
//! changed, as controls that write `shed.toml` (keeping the rest of the file
//! as written), and the way to the file for everything else.

use gpui::{Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Window, div, prelude::*, px, svg};
use shed_model::config::{Appearance, Config, NewPaneDir};

use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::state::AppState;
use crate::theme::{UiColors, hsla, hsla_alpha};

pub enum SettingsEvent {
    Close,
}

pub struct SettingsPanel {
    state: Entity<AppState>,
    family: Entity<LineEditor>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

impl SettingsPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let family_now = state.read(cx).settings.config.font.family.clone();
        let family = cx.new(|cx| LineEditor::new(family_now, cx).without_hint());
        let subs = vec![
            cx.subscribe(&family, |this, _, event: &EditorEvent, cx| {
                if let EditorEvent::Submit(text) = event
                    && !text.trim().is_empty()
                {
                    this.set(Some("font"), "family", text.trim().into(), cx);
                }
            }),
            cx.observe(&state, |_, _, cx| cx.notify()),
        ];
        Self { state, family, _subs: subs }
    }

    /// Writes one setting; Shed takes it at once.
    fn set(&mut self, table: Option<&str>, key: &str, value: toml_edit::Value, cx: &mut Context<Self>) {
        let (table, key) = (table.map(str::to_string), key.to_string());
        self.state.update(cx, |state, cx| state.set_config_value(table.as_deref(), &key, value, cx));
    }
}

/// A row: what it is, a line saying more, and its control on the right.
fn row(ui: &UiColors, title: &str, about: &str, control: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .py_2()
        .border_b_1()
        .border_color(hsla_alpha(ui.divider, 0.6))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_color(hsla(ui.text)).child(title.to_string()))
                .when(!about.is_empty(), |d| d.child(div().text_size(px(11.5)).text_color(hsla(ui.text_dim)).child(about.to_string()))),
        )
        .child(div().flex_none().child(control))
}

/// Choices side by side, the current one marked.
fn segmented<T: Clone + PartialEq + 'static>(
    id: &str,
    ui: &UiColors,
    current: T,
    choices: Vec<(T, &'static str)>,
    cx: &mut Context<SettingsPanel>,
    on: impl Fn(&mut SettingsPanel, T, &mut Context<SettingsPanel>) + Clone + 'static,
) -> gpui::Div {
    div().flex().p_0p5().gap_0p5().rounded_md().bg(hsla(ui.tab_active)).children(choices.into_iter().enumerate().map(|(n, (value, label))| {
        let selected = value == current;
        let on = on.clone();
        div()
            .id(SharedString::from(format!("{id}-{n}")))
            .px_2()
            .py_0p5()
            .rounded_sm()
            .cursor_pointer()
            .text_size(px(12.0))
            .when(selected, |d| d.bg(hsla_alpha(ui.accent, 0.22)).text_color(hsla(ui.text)))
            .when(!selected, |d| d.text_color(hsla(ui.text_dim)).hover(|s| s.bg(hsla(ui.tab_hover))))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| on(this, value.clone(), cx)))
    }))
}

/// A value with − and + on either side.
fn stepper(id: &str, ui: &UiColors, shown: String, cx: &mut Context<SettingsPanel>, step: impl Fn(&mut SettingsPanel, f32, &mut Context<SettingsPanel>) + Clone + 'static) -> gpui::Div {
    let button = |sign: f32, glyph: &'static str| {
        let step = step.clone();
        div()
            .id(SharedString::from(format!("{id}-{glyph}")))
            .size(px(22.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .bg(hsla(ui.tab_active))
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .child(svg().path(icon(glyph)).size(px(12.0)).text_color(hsla(ui.text)))
            .on_click(cx.listener(move |this, _, _, cx| step(this, sign, cx)))
    };
    div().flex().items_center().gap_1().child(button(-1.0, "minus")).child(div().min_w(px(52.0)).flex().justify_center().text_color(hsla(ui.text)).child(shown)).child(button(1.0, "plus"))
}

/// A switch: on or off.
fn toggle(id: &str, ui: &UiColors, on: bool, cx: &mut Context<SettingsPanel>, flip: impl Fn(&mut SettingsPanel, bool, &mut Context<SettingsPanel>) + 'static) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.to_string()))
        .w(px(34.0))
        .h(px(18.0))
        .p(px(2.0))
        .rounded_full()
        .cursor_pointer()
        .flex()
        .when(on, |d| d.justify_end().bg(hsla(ui.accent)))
        .when(!on, |d| d.justify_start().bg(hsla_alpha(ui.text_dim, 0.35)))
        .child(div().size(px(14.0)).rounded_full().bg(hsla(ui.tab_active)))
        .on_click(cx.listener(move |this, _, _, cx| flip(this, !on, cx)))
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let ui = state.settings.ui.clone();
        let config = state.settings.config.clone();
        let heading = |text: &str| div().mt_4().mb_1().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(hsla(ui.text_dim)).child(text.to_uppercase());

        let appearance = segmented(
            "appearance",
            &ui,
            config.appearance(),
            vec![(Some(Appearance::Auto), "Auto"), (Some(Appearance::Light), "Light"), (Some(Appearance::Dark), "Dark")],
            cx,
            |this, value, cx| {
                if let Some(a) = value {
                    this.state.update(cx, |state, cx| state.set_appearance(a, cx));
                }
            },
        );
        let view = segmented("view", &ui, config.window.compact_pegboard, vec![(false, "Cards"), (true, "Compact")], cx, |this, compact, cx| {
            this.state.update(cx, |state, cx| state.set_compact_pegboard(compact, cx));
        });
        let font_size = stepper("font-size", &ui, format!("{} pt", config.font.size), cx, |this, sign, cx| {
            let now = this.state.read(cx).settings.config.font.size;
            this.set(Some("font"), "size", ((now + sign).clamp(6.0, 40.0) as f64).into(), cx);
        });
        let bar = stepper("tab-bar", &ui, format!("{}%", (config.window.tab_bar_scale * 100.0).round()), cx, |this, sign, cx| {
            let now = this.state.read(cx).settings.config.window.tab_bar_scale;
            let next = ((now + sign * 0.1) * 10.0).round() / 10.0;
            this.set(Some("window"), "tab_bar_scale", (next.clamp(0.7, 2.0) as f64).into(), cx);
        });
        let new_panes = segmented("new-panes", &ui, config.window.new_pane_dir, vec![(NewPaneDir::Current, "Current folder"), (NewPaneDir::Bench, "Bench's folder")], cx, |this, dir, cx| {
            let value = if dir == NewPaneDir::Bench { "bench" } else { "current" };
            this.set(Some("window"), "new_pane_dir", value.into(), cx);
        });
        let restore = toggle("auto-restore", &ui, config.auto_restore, cx, |this, on, cx| this.set(None, "auto_restore", on.into(), cx));
        let badges = toggle("icon-badges", &ui, config.window.icon_badges, cx, |this, on, cx| this.set(Some("window"), "icon_badges", on.into(), cx));
        let fetch = stepper(
            "git-fetch",
            &ui,
            if config.git.fetch_minutes == 0 { "off".into() } else { format!("{} min", config.git.fetch_minutes) },
            cx,
            |this, sign, cx| {
                let now = this.state.read(cx).settings.config.git.fetch_minutes as i64;
                this.set(Some("git"), "fetch_minutes", (now + sign as i64).clamp(0, 120).into(), cx);
            },
        );
        let path = Config::path();
        let open_file = div()
            .id("open-shed-toml")
            .mt_4()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .border_1()
            .border_color(hsla(ui.divider))
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .child(svg().path(icon("file")).size(px(13.0)).text_color(hsla(ui.text_dim)))
            .child(div().text_color(hsla(ui.text)).child("Open shed.toml"))
            .on_click(move |_, _, cx| cx.open_url(&format!("file://{}", path.display())));

        div()
            .id("settings-panel")
            .occlude()
            .w(px(420.0))
            .h_full()
            .flex()
            .flex_col()
            .p_4()
            .bg(hsla(ui.background))
            .border_l_1()
            .border_color(hsla(ui.divider))
            .text_size(px(13.0))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(svg().path(icon("settings")).size(px(16.0)).text_color(hsla(ui.text)))
                    .child(div().flex_1().text_size(px(15.0)).text_color(hsla(ui.text)).child("Settings"))
                    .child(
                        div()
                            .id("settings-close")
                            .p_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(hsla(ui.tab_hover)))
                            .child(svg().path(icon("close")).size(px(14.0)).text_color(hsla(ui.text_dim)))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                    ),
            )
            .child(heading("Look"))
            .child(row(&ui, "Appearance", "Auto follows the desktop's light or dark", appearance))
            .child(row(&ui, "Font size", "Terminals and code; each tab still zooms on its own", font_size))
            .child(row(&ui, "Font", "Its family name; Enter applies it", div().w(px(170.0)).px_1().rounded_sm().border_1().border_color(hsla(ui.divider)).child(self.family.clone())))
            .child(row(&ui, "Tab bar size", "How large a bench window's tab bar is", bar))
            .child(row(&ui, "Shed window", "Cards with previews, or a line per bench", view))
            .child(heading("Benches"))
            .child(row(&ui, "Reopen at start", "Benches open when Shed last closed come back", restore))
            .child(row(&ui, "New tabs and panes open in", "", new_panes))
            .child(row(&ui, "Badges on taskbar icons", "Each bench's badge, or its initials", badges))
            .child(heading("Projects"))
            .child(row(&ui, "Git fetch", "How often project tabs fetch, to know what is behind", fetch))
            .child(open_file)
            .child(div().mt_2().text_size(px(11.5)).text_color(hsla(ui.text_dim)).child("Keys, the rack, tag rules, themes and the rest are in shed.toml; Shed follows the file as it changes."))
    }
}

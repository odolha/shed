//! The Setup panel: a tab's startup, teardown and aliases, shown and edited
//! in place. Changes apply the next time the bench loads or closes; aliases
//! apply at the shells' next prompt.

use gpui::{
    AnyElement, Context, Entity, EventEmitter, Focusable, SharedString, Subscription, Window, div,
    prelude::*, px, svg,
};
use shed_model::config::{AliasLevel, resolve_aliases};
use shed_model::{Aliases, BenchId, Pane, PaneId, PaneSetup, Rgb, Tab};
use shed_proto::{AliasTarget, ClientMsg};
use shed_save::{expand_home, home_relative};

use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::state::AppState;
use crate::theme::{UiColors, hsla, hsla_alpha};

pub enum PanelEvent {
    Close,
    /// An edit ended; the keyboard goes back to the terminal.
    Done,
}

/// What an open text field edits.
#[derive(Clone, PartialEq)]
enum Field {
    StartIn(PaneId),
    /// A startup command of a pane, by position and text; `None` adds one.
    Startup(PaneId, Option<(usize, String)>),
    Teardown(PaneId, Option<(usize, String)>),
    /// `name = command`; `None` adds one at the chosen level.
    Alias(Option<(AliasLevel, String)>),
}

/// Replaces, removes (empty `text`) or adds (`at` is `None`) a command. The
/// list may have changed since the edit began, so the old text is looked up.
fn edit_list(list: &mut Vec<String>, at: Option<(usize, String)>, text: String) {
    let found = at.map(|(i, old)| if list.get(i) == Some(&old) { Some(i) } else { list.iter().position(|c| *c == old) });
    match found {
        Some(Some(i)) if text.is_empty() => {
            list.remove(i);
        }
        Some(Some(i)) => list[i] = text,
        // Gone meanwhile: keep what was typed.
        Some(None) | None if !text.is_empty() => list.push(text),
        _ => {}
    }
}

/// `name = command`, or `alias name='command'` as bash prints it. `Some(None)`
/// for an empty field (remove), `None` for text that is not an alias.
fn parse_alias(text: &str) -> Option<Option<(String, String)>> {
    let text = text.trim();
    if text.is_empty() {
        return Some(None);
    }
    let (name, value) = text.strip_prefix("alias ").unwrap_or(text).split_once('=')?;
    let (name, value) = (name.trim(), value.trim());
    let value = match (value.chars().next(), value.chars().last()) {
        (Some(a @ ('"' | '\'')), Some(b)) if a == b && value.len() >= 2 => value[1..value.len() - 1].trim(),
        _ => value,
    };
    let ok = !name.is_empty() && !name.contains([' ', '\t', '/', '=']) && !value.is_empty();
    ok.then(|| Some((name.to_string(), value.to_string())))
}

pub struct SetupPanel {
    bench: BenchId,
    state: Entity<AppState>,
    editing: Option<(Field, Entity<LineEditor>, [Subscription; 2])>,
    /// Where a new alias goes.
    new_alias_level: AliasLevel,
    /// Whether the shell's own aliases are listed too.
    show_shell: bool,
}

impl EventEmitter<PanelEvent> for SetupPanel {}

impl SetupPanel {
    pub fn new(bench: BenchId, state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { bench, state, editing: None, new_alias_level: AliasLevel::Tab, show_shell: false }
    }

    fn tab<'a>(&self, cx: &'a Context<Self>) -> Option<&'a Tab> {
        self.state.read(cx).tree.bench(self.bench).and_then(|b| b.active_tab())
    }

    /// The tab's panes in layout order.
    fn panes<'a>(&self, cx: &'a Context<Self>) -> Vec<&'a Pane> {
        let Some(tab) = self.tab(cx) else { return Vec::new() };
        tab.layout.leaves().iter().filter_map(|id| tab.pane(*id)).collect()
    }

    fn send(&self, msg: ClientMsg, cx: &Context<Self>) {
        self.state.read(cx).conn.send_msg(msg);
    }

    fn set_setup(&self, pane: PaneId, edit: impl FnOnce(&mut PaneSetup), cx: &Context<Self>) {
        let Some(p) = self.state.read(cx).tree.pane(pane) else { return };
        let mut setup = p.setup.clone();
        edit(&mut setup);
        if setup != p.setup {
            self.send(ClientMsg::SetPaneSetup { pane, setup }, cx);
        }
    }

    fn aliases_at(&self, level: AliasLevel, cx: &Context<Self>) -> Option<(AliasTarget, Aliases)> {
        let state = self.state.read(cx);
        let bench = state.tree.bench(self.bench)?;
        let tab = bench.active_tab()?;
        Some(match level {
            AliasLevel::Global => (AliasTarget::Global, state.tree.aliases.clone()),
            AliasLevel::Bench => (AliasTarget::Bench(bench.id), bench.aliases.clone()),
            AliasLevel::Tab => (AliasTarget::Tab(tab.id), tab.aliases.clone()),
            AliasLevel::Shell => return None,
        })
    }

    fn edit(&mut self, field: Field, initial: String, window: &mut Window, cx: &mut Context<Self>) {
        // Moving straight to another field keeps the one being left.
        if let Some((field, editor, _)) = self.editing.take() {
            let text = editor.read(cx).text().trim().to_string();
            self.commit(field, text, cx);
        }
        let editor = cx.new(|cx| LineEditor::new(initial, cx));
        let focus = editor.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        let submit = cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, _, cx| {
            let Some((field, _, _)) = this.editing.take() else { return };
            if let EditorEvent::Submit(text) = event {
                this.commit(field, text.trim().to_string(), cx);
            }
            cx.emit(PanelEvent::Done);
            cx.notify();
        });
        // Clicking elsewhere keeps what was typed, as Enter would.
        let blur = cx.on_blur(&focus, window, |this, _, cx| {
            let Some((field, editor, _)) = this.editing.take() else { return };
            let text = editor.read(cx).text().trim().to_string();
            this.commit(field, text, cx);
            cx.notify();
        });
        self.editing = Some((field, editor, [submit, blur]));
        cx.notify();
    }

    fn commit(&mut self, field: Field, text: String, cx: &mut Context<Self>) {
        match field {
            Field::StartIn(p) => self.set_setup(p, |s| s.start_in = (!text.is_empty()).then(|| expand_home(&text)), cx),
            Field::Startup(p, at) => self.set_setup(p, |s| edit_list(&mut s.startup, at, text), cx),
            Field::Teardown(p, at) => self.set_setup(p, |s| edit_list(&mut s.teardown, at, text), cx),
            Field::Alias(existing) => {
                let level = existing.as_ref().map_or(self.new_alias_level, |(l, _)| *l);
                let Some((target, mut aliases)) = self.aliases_at(level, cx) else { return };
                // Not an alias: leave things as they were rather than lose one.
                let Some(parsed) = parse_alias(&text) else { return };
                if let Some((_, old)) = &existing {
                    aliases.remove(old);
                }
                if let Some((name, value)) = parsed {
                    aliases.insert(name, value);
                }
                self.send(ClientMsg::SetAliases { level: target, aliases }, cx);
            }
        }
    }

    fn editor_for(&self, field: &Field) -> Option<Entity<LineEditor>> {
        self.editing.as_ref().filter(|(f, _, _)| f == field).map(|(_, e, _)| e.clone())
    }

    /// A value that turns into a text field when clicked.
    fn value(&self, id: SharedString, field: Field, text: String, placeholder: &str, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        if let Some(editor) = self.editor_for(&field) {
            return div().flex_1().child(editor).into_any_element();
        }
        let empty = text.is_empty();
        let shown = if empty { placeholder.to_string() } else { text.clone() };
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .px_1()
            .rounded_sm()
            .truncate()
            .cursor_text()
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .text_color(hsla(if empty { ui.text_dim } else { ui.text }))
            .child(shown)
            .on_click(cx.listener(move |this, _, window, cx| this.edit(field.clone(), text.clone(), window, cx)))
            .into_any_element()
    }

    fn remove_button(&self, id: SharedString, ui: &UiColors, cx: &mut Context<Self>, on: impl Fn(&mut Self, &mut Context<Self>) + 'static) -> AnyElement {
        div()
            .id(id)
            .flex_none()
            .p_0p5()
            .rounded_sm()
            .cursor_pointer()
            .hover(|s| s.bg(hsla(ui.divider)))
            .on_click(cx.listener(move |this, _, _, cx| on(this, cx)))
            .child(svg().path(icon("close")).size(px(11.0)).text_color(hsla(ui.text_dim)))
            .into_any_element()
    }

    fn link(&self, id: SharedString, label: &str, accent: Rgb, cx: &mut Context<Self>, on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> AnyElement {
        div()
            .id(id)
            .text_size(px(12.0))
            .text_color(hsla(accent))
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(label.to_string())
            .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
            .into_any_element()
    }

    fn render_list(&self, pane: PaneId, teardown: bool, items: &[String], ui: &UiColors, accent: Rgb, cx: &mut Context<Self>) -> AnyElement {
        let kind = if teardown { "teardown" } else { "startup" };
        let mut col = div().flex().flex_col().gap_0p5().flex_1().min_w_0();
        for (i, item) in items.iter().enumerate() {
            let at = Some((i, item.clone()));
            let field = if teardown { Field::Teardown(pane, at.clone()) } else { Field::Startup(pane, at.clone()) };
            let value = self.value(format!("{kind}-{pane}-{i}").into(), field, item.clone(), "", ui, cx);
            let remove = self.remove_button(format!("{kind}-rm-{pane}-{i}").into(), ui, cx, move |this, cx| {
                let at = at.clone();
                this.set_setup(pane, |s| edit_list(if teardown { &mut s.teardown } else { &mut s.startup }, at, String::new()), cx)
            });
            col = col.child(div().flex().items_center().gap_1().font_family(self.mono(cx)).child(value).child(remove));
        }
        let new_field = if teardown { Field::Teardown(pane, None) } else { Field::Startup(pane, None) };
        if let Some(editor) = self.editor_for(&new_field) {
            col = col.child(editor);
        } else {
            col = col.child(self.link(format!("{kind}-add-{pane}").into(), "+ add command", accent, cx, move |this, window, cx| {
                this.edit(new_field.clone(), String::new(), window, cx)
            }));
        }
        col.into_any_element()
    }

    fn mono(&self, cx: &Context<Self>) -> SharedString {
        self.state.read(cx).settings.font.family.clone().into()
    }

    fn render_pane(&self, index: usize, pane: &Pane, ui: &UiColors, accent: Rgb, cx: &mut Context<Self>) -> AnyElement {
        let id = pane.id;
        let active = self.tab(cx).is_some_and(|t| t.panes.len() > 1 && t.active_pane == id);
        let row = |label: &str, content: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .child(div().w(px(70.0)).flex_none().text_color(hsla(ui.text_dim)).child(label.to_string()))
                .child(content)
        };
        let running = pane.tool.as_ref().map(|t| t.display());
        let mut head = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_color(hsla(if active { accent } else { ui.text })).child(format!("Pane {}", index + 1)))
            .child(div().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child(format!("now in {}", home_relative(&pane.cwd))));
        if let Some(r) = &running {
            head = head.child(svg().path(icon("tool")).size(px(11.0)).text_color(hsla(ui.text_dim))).child(
                div().text_size(px(12.0)).text_color(hsla(ui.text_dim)).truncate().child(r.clone()),
            );
        }

        let start_in = pane.setup.start_in.as_deref().map(home_relative).unwrap_or_default();
        let start = self.value(format!("start-{index}").into(), Field::StartIn(id), start_in, "where it was last", ui, cx);
        let startup = self.render_list(id, false, &pane.setup.startup, ui, accent, cx);
        let teardown = self.render_list(id, true, &pane.setup.teardown, ui, accent, cx);

        let wait = pane.setup.wait;
        let mut options = div().flex().gap_3().pl(px(78.0));
        if !pane.setup.startup.is_empty() {
            options = options.child(self.link(
                format!("wait-{index}").into(),
                if wait { "☑ wait for Enter" } else { "☐ wait for Enter" },
                accent,
                cx,
                move |this, _, cx| this.set_setup(id, |s| s.wait = !s.wait, cx),
            ));
        }
        if let Some(cmd) = running.filter(|r| !pane.setup.startup.contains(r)) {
            options = options.child(self.link(format!("use-{index}").into(), "use running command", accent, cx, move |this, _, cx| {
                let cmd = cmd.clone();
                this.set_setup(id, |s| s.startup.push(cmd), cx)
            }));
        }
        let startup_hint = if pane.setup.startup.is_empty() {
            Some(div().pl(px(78.0)).text_size(px(11.0)).text_color(hsla(ui.text_dim)).child("Empty: on load, what was running is restarted or typed, by the rack."))
        } else {
            None
        };
        let teardown_hint = if pane.setup.teardown.is_empty() {
            Some(div().pl(px(78.0)).text_size(px(11.0)).text_color(hsla(ui.text_dim)).child("Empty: closing the bench stops the running tool."))
        } else {
            None
        };

        div()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .border_t_1()
            .border_color(hsla(ui.divider))
            .child(head)
            .child(row("Start in", start))
            .child(row("Startup", startup))
            .children(startup_hint)
            .child(options)
            .child(row("Teardown", teardown))
            .children(teardown_hint)
            .into_any_element()
    }

    fn render_aliases(&self, ui: &UiColors, accent: Rgb, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let Some(bench) = state.tree.bench(self.bench) else { return div().into_any_element() };
        let Some(tab) = bench.active_tab() else { return div().into_any_element() };
        let shell = tab.active().map(|p| p.shell_aliases.clone()).unwrap_or_default();
        let rows = resolve_aliases(&shell, &state.tree.aliases, &bench.aliases, &tab.aliases);
        let mono = self.mono(cx);

        let shell_count = rows.iter().filter(|r| r.2 == AliasLevel::Shell).count();
        let shown = rows.into_iter().filter(|r| self.show_shell || r.2 != AliasLevel::Shell);
        let mut list = div().flex().flex_col().gap_0p5();
        for (i, (name, value, level, hidden)) in shown.enumerate() {
            let editable = level != AliasLevel::Shell;
            let level_color = match level {
                AliasLevel::Tab | AliasLevel::Bench => accent,
                AliasLevel::Global => ui.text,
                AliasLevel::Shell => ui.text_dim,
            };
            let text = format!("{name} = {value}");
            let content = if editable {
                self.value(format!("alias-{i}").into(), Field::Alias(Some((level, name.clone()))), text, "", ui, cx)
            } else {
                div().flex_1().min_w_0().truncate().px_1().text_color(hsla(ui.text_dim)).child(text).into_any_element()
            };
            let over = (!hidden.is_empty()).then(|| {
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(hsla(ui.text_dim))
                    .child(format!("over {}", hidden.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ")))
            });
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(48.0)).flex_none().text_size(px(11.0)).text_color(hsla(level_color)).child(level.name()))
                .child(div().flex_1().min_w_0().font_family(mono.clone()).child(content))
                .children(over);
            if editable {
                let name = name.clone();
                row = row.child(self.remove_button(format!("alias-rm-{i}").into(), ui, cx, move |this, cx| {
                    if let Some((target, mut aliases)) = this.aliases_at(level, cx) {
                        aliases.remove(&name);
                        this.send(ClientMsg::SetAliases { level: target, aliases }, cx);
                    }
                }));
            }
            list = list.child(row);
        }
        if shell_count > 0 {
            let label = if self.show_shell {
                "hide the shell's own".to_string()
            } else {
                format!("show the shell's own ({shell_count})")
            };
            list = list.child(self.link("alias-shell".into(), &label, ui.text_dim, cx, |this, _, cx| {
                this.show_shell = !this.show_shell;
                cx.notify();
            }));
        }

        let levels = div().flex().gap_1().children([AliasLevel::Tab, AliasLevel::Bench, AliasLevel::Global].map(|level| {
            let on = level == self.new_alias_level;
            div()
                .id(SharedString::from(format!("level-{}", level.name())))
                .px_1p5()
                .rounded_sm()
                .text_size(px(12.0))
                .cursor_pointer()
                .when(on, |d| d.bg(hsla_alpha(accent, 0.25)).text_color(hsla(accent)))
                .when(!on, |d| d.text_color(hsla(ui.text_dim)))
                .child(level.name())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.new_alias_level = level;
                    cx.notify();
                }))
        }));
        let add = match self.editor_for(&Field::Alias(None)) {
            Some(editor) => div().flex_1().font_family(mono).child(editor).into_any_element(),
            None => self.link("alias-add".into(), "+ add alias (name = command)", accent, cx, |this, window, cx| {
                this.edit(Field::Alias(None), String::new(), window, cx)
            }),
        };

        div()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .border_t_1()
            .border_color(hsla(ui.divider))
            .child(div().text_color(hsla(ui.text)).child("Aliases"))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(hsla(ui.text_dim))
                    .child("Tab over bench over global over your shell's own. Changes apply at the next prompt."),
            )
            .child(list)
            .child(div().flex().items_center().gap_2().child(add).child(levels))
            .into_any_element()
    }
}

impl Render for SetupPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let ui = state.settings.ui.clone();
        let accent = state.tree.bench(self.bench).map(|b| b.color.accent()).unwrap_or(ui.accent_fallback);
        let title = self.tab(cx).map(|t| t.display_title()).unwrap_or_default();
        let panes: Vec<(usize, Pane)> = self.panes(cx).into_iter().cloned().enumerate().collect();

        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .child(svg().path(icon("rack")).size(px(16.0)).text_color(hsla(accent)))
            .child(div().flex_1().truncate().text_color(hsla(ui.text)).child(format!("Setup · {title}")))
            .child(
                div()
                    .id("close-setup")
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PanelEvent::Close)))
                    .child(svg().path(icon("close")).size(px(12.0)).text_color(hsla(ui.text_dim))),
            );
        let intro = div()
            .text_size(px(11.0))
            .text_color(hsla(ui.text_dim))
            .child("Used the next time this bench loads, and when it is closed. Click a value to edit; empty it to remove it.");

        let mut body = div().flex().flex_col();
        for (i, pane) in &panes {
            body = body.child(self.render_pane(*i, pane, &ui, accent, cx));
        }
        body = body.child(self.render_aliases(&ui, accent, cx));

        div()
            .id("setup-panel")
            .occlude()
            .w(px(440.0))
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(hsla(ui.tab_active))
            .border_l_1()
            .border_color(hsla(ui.divider))
            .text_size(px(13.0))
            .overflow_y_scroll()
            .child(header)
            .child(intro)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_parse_from_both_forms() {
        let pair = |n: &str, v: &str| Some(Some((n.to_string(), v.to_string())));
        assert_eq!(parse_alias("gs = git status"), pair("gs", "git status"));
        assert_eq!(parse_alias("alias ll='ls -la'"), pair("ll", "ls -la"));
        assert_eq!(parse_alias("e=\"echo 'hi'\""), pair("e", "echo 'hi'"));
        assert_eq!(parse_alias("x = a=b"), pair("x", "a=b"));
        assert_eq!(parse_alias("  "), Some(None));
        assert_eq!(parse_alias("no equals"), None);
        assert_eq!(parse_alias("two words = x"), None);
        assert_eq!(parse_alias("y = ''"), None);
    }

    #[test]
    fn list_edits_find_the_command_even_if_it_moved() {
        let mut list = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        edit_list(&mut list, Some((1, "b".into())), "B".into());
        assert_eq!(list, ["a", "B", "c"]);
        // "c" moved to index 1 meanwhile.
        list.remove(0);
        edit_list(&mut list, Some((2, "c".into())), String::new());
        assert_eq!(list, ["B"]);
        edit_list(&mut list, None, "d".into());
        edit_list(&mut list, None, String::new());
        assert_eq!(list, ["B", "d"]);
        // Edited away by someone else: what was typed is kept.
        edit_list(&mut list, Some((0, "gone".into())), "e".into());
        assert_eq!(list, ["B", "d", "e"]);
    }
}

//! Right-click menus, for any view that shows them, and tooltips.

use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Context, Pixels, Point, SharedString, Window, anchored, deferred, div, prelude::*, px, svg,
};
use shed_model::Rgb;

use crate::assets::icon;
use crate::theme::{UiColors, current_ui, hsla};

pub type MenuAction<V> = Rc<dyn Fn(&mut V, &mut Window, &mut Context<V>)>;

pub enum MenuEntry<V> {
    Item { label: SharedString, icon: Option<&'static str>, swatch: Option<Rgb>, danger: bool, action: MenuAction<V> },
    Separator,
}

impl<V> MenuEntry<V> {
    /// Shows a UI icon before the label.
    pub fn icon(mut self, name: &'static str) -> Self {
        if let MenuEntry::Item { icon, .. } = &mut self {
            *icon = Some(name);
        }
        self
    }
}

pub fn item<V>(label: impl Into<SharedString>, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), icon: None, swatch: None, danger: false, action: Rc::new(action) }
}

/// An item in red, for what stops or removes things.
pub fn danger<V>(label: impl Into<SharedString>, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), icon: None, swatch: None, danger: true, action: Rc::new(action) }
}

/// An item with a colour dot.
pub fn swatch<V>(label: impl Into<SharedString>, color: Rgb, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), icon: None, swatch: Some(color), danger: false, action: Rc::new(action) }
}

/// A one-line tooltip: `.tooltip(tooltip("Close"))`.
pub fn tooltip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| Tooltip(text.into())).into()
}

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = current_ui(cx);
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(hsla(ui.tab_active))
            .border_1()
            .border_color(hsla(ui.divider))
            .text_size(px(12.0))
            .text_color(hsla(ui.text))
            .child(self.0.clone())
    }
}

pub struct Menu<V> {
    pub position: Point<Pixels>,
    pub entries: Vec<MenuEntry<V>>,
}

/// A view that shows menus, one at a time.
pub trait MenuHost: Sized + 'static {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>>;

    /// After the menu goes, whether something was chosen or not.
    fn menu_closed(&mut self, window: &mut Window, cx: &mut Context<Self>);

    fn open_menu(&mut self, position: Point<Pixels>, entries: Vec<MenuEntry<Self>>, cx: &mut Context<Self>) {
        *self.menu_slot() = Some(Menu { position, entries });
        cx.stop_propagation();
        cx.notify();
    }
}

pub fn render<V: MenuHost>(menu: &Menu<V>, ui: &UiColors, cx: &mut Context<V>) -> AnyElement {
    let mut list = div()
        .id("menu")
        .occlude()
        .min_w(px(200.0))
        .py_1()
        .bg(hsla(ui.tab_active))
        .border_1()
        .border_color(hsla(ui.divider))
        .rounded_md()
        // No drop shadow: a blurred shadow is redrawn over the whole menu on
        // every hover change, which tripled the cost of moving through it.
        .text_size(px(13.0))
        .text_color(hsla(ui.text))
        .on_mouse_down_out(cx.listener(|this: &mut V, _, window, cx| {
            *this.menu_slot() = None;
            this.menu_closed(window, cx);
        }));
    // With icons on some items, every label starts at the same place.
    let icons = menu.entries.iter().any(|e| matches!(e, MenuEntry::Item { icon: Some(_), .. } | MenuEntry::Item { swatch: Some(_), .. }));
    for (i, entry) in menu.entries.iter().enumerate() {
        list = list.child(match entry {
            MenuEntry::Separator => div().my_1().h(px(1.0)).bg(hsla(ui.divider)).into_any_element(),
            MenuEntry::Item { label, icon: glyph, swatch, danger, action } => {
                let action = action.clone();
                let color = if *danger { Rgb::hex(0xF0566A) } else { ui.text };
                let lead = match (glyph, swatch) {
                    (Some(name), _) => Some(svg().path(icon(name)).size(px(14.0)).text_color(hsla(if *danger { color } else { ui.text_dim })).into_any_element()),
                    (None, Some(c)) => Some(div().size(px(10.0)).mx(px(2.0)).rounded_full().bg(hsla(*c)).into_any_element()),
                    (None, None) if icons => Some(div().size(px(14.0)).into_any_element()),
                    (None, None) => None,
                };
                div()
                    .id(("menu-item", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .text_color(hsla(color))
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .cursor_pointer()
                    .children(lead)
                    .child(label.clone())
                    .on_click(cx.listener(move |this: &mut V, _, window, cx| {
                        *this.menu_slot() = None;
                        action(this, window, cx);
                        this.menu_closed(window, cx);
                    }))
                    .into_any_element()
            }
        });
    }
    deferred(anchored().position(menu.position).snap_to_window_with_margin(px(8.0)).child(list)).priority(1).into_any_element()
}

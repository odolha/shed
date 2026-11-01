//! Right-click menus, for any view that shows them.

use std::rc::Rc;

use gpui::{AnyElement, Context, Pixels, Point, SharedString, Window, anchored, deferred, div, prelude::*, px};
use shed_model::Rgb;

use crate::theme::{UiColors, hsla};

pub type MenuAction<V> = Rc<dyn Fn(&mut V, &mut Window, &mut Context<V>)>;

pub enum MenuEntry<V> {
    Item { label: SharedString, swatch: Option<Rgb>, danger: bool, action: MenuAction<V> },
    Separator,
}

pub fn item<V>(label: impl Into<SharedString>, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), swatch: None, danger: false, action: Rc::new(action) }
}

/// An item in red, for what stops or removes things.
pub fn danger<V>(label: impl Into<SharedString>, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), swatch: None, danger: true, action: Rc::new(action) }
}

/// An item with a colour dot.
pub fn swatch<V>(label: impl Into<SharedString>, color: Rgb, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), swatch: Some(color), danger: false, action: Rc::new(action) }
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
    for (i, entry) in menu.entries.iter().enumerate() {
        list = list.child(match entry {
            MenuEntry::Separator => div().my_1().h(px(1.0)).bg(hsla(ui.divider)).into_any_element(),
            MenuEntry::Item { label, swatch, danger, action } => {
                let action = action.clone();
                let color = if *danger { Rgb::hex(0xF0566A) } else { ui.text };
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
                    .when_some(*swatch, |el, c| el.child(div().size(px(10.0)).rounded_full().bg(hsla(c))))
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

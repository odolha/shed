//! Right-click menus, for any view that shows them, and tooltips.

use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Context, Pixels, Point, SharedString, Window, anchored, deferred, div, prelude::*, px, svg,
};
use shed_model::Rgb;

use crate::assets::icon;
use crate::theme::{UiColors, current_ui, hsla};

pub type MenuAction<V> = Rc<dyn Fn(&mut V, &mut Window, &mut Context<V>)>;
pub type MenuEntries<V> = Rc<dyn Fn(&mut V, &mut Context<V>) -> Vec<MenuEntry<V>>>;

pub enum MenuEntry<V> {
    /// `dot`: a colour shown before the label (a tag's), in the icon's place.
    Item { label: SharedString, icon: Option<&'static str>, danger: bool, action: MenuAction<V>, dot: Option<Rgb> },
    /// Colours to pick from, as a grid of dots.
    Palette(Vec<Swatch<V>>),
    /// Opens another menu in its place.
    More { label: SharedString, icon: Option<&'static str>, entries: MenuEntries<V> },
    Separator,
}

pub struct Swatch<V> {
    pub color: Rgb,
    /// Its name, shown on hover.
    pub name: &'static str,
    pub current: bool,
    pub action: MenuAction<V>,
}

pub fn swatch_of<V>(color: Rgb, name: &'static str, current: bool, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> Swatch<V> {
    Swatch { color, name, current, action: Rc::new(action) }
}

impl<V> MenuEntry<V> {
    /// Shows a UI icon before the label.
    pub fn icon(mut self, name: &'static str) -> Self {
        if let MenuEntry::Item { icon, .. } | MenuEntry::More { icon, .. } = &mut self {
            *icon = Some(name);
        }
        self
    }

    /// Shows a dot of this colour before the label.
    pub fn dot(mut self, color: Rgb) -> Self {
        if let MenuEntry::Item { dot, .. } = &mut self {
            *dot = Some(color);
        }
        self
    }
}

pub fn item<V>(label: impl Into<SharedString>, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), icon: None, danger: false, action: Rc::new(action), dot: None }
}

/// An item that opens another menu, made when it is chosen.
pub fn more<V>(label: impl Into<SharedString>, entries: impl Fn(&mut V, &mut Context<V>) -> Vec<MenuEntry<V>> + 'static) -> MenuEntry<V> {
    MenuEntry::More { label: label.into(), icon: None, entries: Rc::new(entries) }
}

/// An item in red, for what stops or removes things.
pub fn danger<V>(label: impl Into<SharedString>, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), icon: None, danger: true, action: Rc::new(action), dot: None }
}

/// A one-line tooltip: `.tooltip(tooltip("Close"))`.
pub fn tooltip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| Tooltip(text.into())).into()
}

/// A tooltip of text known only when drawing (a title cut short).
pub fn tooltip_text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip(text.clone())).into()
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
            .text_size(crate::theme::us(12.0))
            .text_color(hsla(ui.text))
            .child(self.0.clone())
    }
}

pub struct Menu<V> {
    pub position: Point<Pixels>,
    pub entries: Vec<MenuEntry<V>>,
    /// The submenu shown beside the item at this index (opened by hovering it).
    pub sub: Option<(usize, Vec<MenuEntry<V>>)>,
}

/// A view that shows menus, one at a time.
pub trait MenuHost: Sized + 'static {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>>;

    /// After the menu goes, whether something was chosen or not.
    fn menu_closed(&mut self, window: &mut Window, cx: &mut Context<Self>);

    fn open_menu(&mut self, position: Point<Pixels>, entries: Vec<MenuEntry<Self>>, cx: &mut Context<Self>) {
        *self.menu_slot() = Some(Menu { position, entries, sub: None });
        cx.stop_propagation();
        cx.notify();
    }
}

/// Rows have fixed heights, so a submenu can be put beside its item.
const ROW: f32 = 26.0;
const SEPARATOR: f32 = 9.0;
const LIST_PAD: f32 = 4.0;
const SWATCH: f32 = 20.0;
const SWATCH_GAP: f32 = 6.0;
const SWATCHES_PER_ROW: usize = 8;

/// How far down its list the entry at `at` starts.
fn entry_top<V>(entries: &[MenuEntry<V>], at: usize) -> Pixels {
    let us = |n: f32| crate::theme::us(n);
    let mut top = px(LIST_PAD);
    for e in &entries[..at.min(entries.len())] {
        top += match e {
            MenuEntry::Separator => px(SEPARATOR),
            MenuEntry::Palette(s) => {
                let rows = s.len().div_ceil(SWATCHES_PER_ROW).max(1) as f32;
                px(rows * SWATCH + (rows - 1.0) * SWATCH_GAP + 8.0)
            }
            _ => us(ROW),
        };
    }
    top
}

pub fn render<V: MenuHost>(menu: &Menu<V>, ui: &UiColors, cx: &mut Context<V>) -> AnyElement {
    let main = list(&menu.entries, menu.position, true, menu.sub.as_ref().map(|(i, _)| *i), ui, cx);
    let mut both = div()
        .flex()
        .items_start()
        .on_mouse_down_out(cx.listener(|this: &mut V, _, window, cx| {
            *this.menu_slot() = None;
            this.menu_closed(window, cx);
        }))
        .child(main);
    if let Some((at, entries)) = &menu.sub {
        // Its first item level with the item that opened it.
        let top = entry_top(&menu.entries, *at) - px(LIST_PAD);
        both = both.child(div().pt(top).ml(px(-2.0)).child(list(entries, menu.position, false, None, ui, cx)));
    }
    deferred(anchored().position(menu.position).snap_to_window_with_margin(px(8.0)).child(both)).priority(1).into_any_element()
}

/// One list of a menu: the menu itself (`top`), or a submenu beside it.
fn list<V: MenuHost>(entries: &[MenuEntry<V>], position: Point<Pixels>, top: bool, open: Option<usize>, ui: &UiColors, cx: &mut Context<V>) -> gpui::Stateful<gpui::Div> {
    let base = if top { 0 } else { 10_000 };
    let mut list = div()
        .id(if top { "menu" } else { "submenu" })
        .occlude()
        .min_w(px(200.0))
        .py(px(LIST_PAD))
        .bg(hsla(ui.tab_active))
        .border_1()
        .border_color(hsla(ui.divider))
        .rounded_md()
        // No drop shadow: a blurred shadow is redrawn over the whole menu on
        // every hover change, which tripled the cost of moving through it.
        .text_size(crate::theme::us(13.0))
        .text_color(hsla(ui.text));
    // With icons on some items, every label starts at the same place.
    let icons = entries.iter().any(|e| matches!(e, MenuEntry::Item { icon: Some(_), .. } | MenuEntry::Item { dot: Some(_), .. } | MenuEntry::More { icon: Some(_), .. }));
    let lead = |glyph: &Option<&'static str>, color: Rgb| match glyph {
        Some(name) => Some(svg().path(icon(name)).size(crate::theme::us(14.0)).text_color(hsla(color)).into_any_element()),
        None if icons => Some(div().size(px(14.0)).into_any_element()),
        None => None,
    };
    let row = |id: usize| div().id(("menu-item", base + id)).h(crate::theme::us(ROW)).flex().items_center().gap_2().px_3().cursor_pointer();
    for (i, entry) in entries.iter().enumerate() {
        list = list.child(match entry {
            MenuEntry::Separator => div().h(px(SEPARATOR)).flex().items_center().child(div().w_full().h(px(1.0)).bg(hsla(ui.divider))).into_any_element(),
            MenuEntry::Palette(swatches) => div()
                .flex()
                .flex_wrap()
                .gap(px(SWATCH_GAP))
                .px_3()
                .py_1()
                .max_w(px(SWATCHES_PER_ROW as f32 * (SWATCH + SWATCH_GAP) + 24.0))
                .children(swatches.iter().enumerate().map(|(j, s)| {
                    let action = s.action.clone();
                    let ring = if s.current { ui.text } else { ui.tab_active };
                    div()
                        .id(("swatch", base + i * 1000 + j))
                        .size(px(SWATCH))
                        .rounded_full()
                        .border_2()
                        .border_color(hsla(ring))
                        .bg(hsla(s.color))
                        .cursor_pointer()
                        .hover(|st| st.border_color(hsla(ui.text_dim)))
                        .tooltip(tooltip(s.name))
                        .on_click(cx.listener(move |this: &mut V, _, window, cx| {
                            *this.menu_slot() = None;
                            action(this, window, cx);
                            this.menu_closed(window, cx);
                        }))
                }))
                .into_any_element(),
            MenuEntry::More { label, icon: glyph, entries } => {
                let (on_hover, on_click) = (entries.clone(), entries.clone());
                let shown = open == Some(i);
                row(i)
                    .when(shown, |d| d.bg(hsla(ui.tab_hover)))
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .children(lead(glyph, ui.text_dim))
                    .child(div().flex_1().child(label.clone()))
                    .child(div().text_color(hsla(ui.text_dim)).child("›"))
                    // In the menu itself it opens beside on hover; deeper, in its place.
                    .when(top, |d| {
                        d.on_hover(cx.listener(move |this: &mut V, hovered: &bool, _, cx| {
                            if !*hovered {
                                return;
                            }
                            let entries = on_hover(this, cx);
                            if let Some(menu) = this.menu_slot()
                                && menu.sub.as_ref().is_none_or(|(at, _)| *at != i)
                            {
                                menu.sub = Some((i, entries));
                                cx.notify();
                            }
                        }))
                    })
                    .on_click(cx.listener(move |this: &mut V, _, _, cx| {
                        let entries = on_click(this, cx);
                        if top {
                            if let Some(menu) = this.menu_slot() {
                                menu.sub = Some((i, entries));
                            }
                        } else {
                            *this.menu_slot() = Some(Menu { position, entries, sub: None });
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
            MenuEntry::Item { label, icon: glyph, danger, action, dot } => {
                let action = action.clone();
                let color = if *danger { ui.danger } else { ui.text };
                let lead = match dot {
                    Some(c) => Some(div().size(px(14.0)).flex().items_center().justify_center().child(div().size(px(10.0)).rounded_full().bg(hsla(*c))).into_any_element()),
                    None => lead(glyph, if *danger { color } else { ui.text_dim }),
                };
                row(i)
                    .text_color(hsla(color))
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .children(lead)
                    .child(label.clone())
                    // Moving onto another item of the menu closes a submenu.
                    .when(top, |d| {
                        d.on_hover(cx.listener(|this: &mut V, hovered: &bool, _, cx| {
                            if *hovered
                                && let Some(menu) = this.menu_slot()
                                && menu.sub.take().is_some()
                            {
                                cx.notify();
                            }
                        }))
                    })
                    .on_click(cx.listener(move |this: &mut V, _, window, cx| {
                        *this.menu_slot() = None;
                        action(this, window, cx);
                        this.menu_closed(window, cx);
                    }))
                    .into_any_element()
            }
        });
    }
    list
}

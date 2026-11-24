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
    /// `tip`: a tooltip on hover.
    Item { label: SharedString, icon: Option<&'static str>, danger: bool, action: MenuAction<V>, dot: Option<Rgb>, tip: Option<SharedString> },
    /// Colours to pick from, as a grid of dots.
    Palette(Vec<Swatch<V>>),
    /// Opens another menu in its place.
    More { label: SharedString, icon: Option<&'static str>, entries: MenuEntries<V> },
    /// Shown dimmed, not to be chosen, with why beside it ("offline").
    Disabled { label: SharedString, icon: Option<&'static str>, note: SharedString, tip: Option<SharedString> },
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
        if let MenuEntry::Item { icon, .. } | MenuEntry::More { icon, .. } | MenuEntry::Disabled { icon, .. } = &mut self {
            *icon = Some(name);
        }
        self
    }

    /// A tooltip saying more, on hover.
    pub fn tip(mut self, text: impl Into<SharedString>) -> Self {
        if let MenuEntry::Item { tip, .. } | MenuEntry::Disabled { tip, .. } = &mut self {
            *tip = Some(text.into());
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
    MenuEntry::Item { label: label.into(), icon: None, danger: false, action: Rc::new(action), dot: None, tip: None }
}

/// An item that opens another menu, made when it is chosen.
pub fn more<V>(label: impl Into<SharedString>, entries: impl Fn(&mut V, &mut Context<V>) -> Vec<MenuEntry<V>> + 'static) -> MenuEntry<V> {
    MenuEntry::More { label: label.into(), icon: None, entries: Rc::new(entries) }
}

/// An item that cannot be chosen now, dimmed, with why (`note`) beside it.
pub fn disabled<V>(label: impl Into<SharedString>, note: impl Into<SharedString>) -> MenuEntry<V> {
    MenuEntry::Disabled { label: label.into(), icon: None, note: note.into(), tip: None }
}

/// An item in red, for what stops or removes things.
pub fn danger<V>(label: impl Into<SharedString>, action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> MenuEntry<V> {
    MenuEntry::Item { label: label.into(), icon: None, danger: true, action: Rc::new(action), dot: None, tip: None }
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
    /// The submenus open, outermost first: each beside the item at this
    /// index of the list before it (opened by hovering it), as deep as menus go.
    pub subs: Vec<(usize, Vec<MenuEntry<V>>)>,
    /// Where the menu and its submenus were drawn, and the window's size: a
    /// submenu goes where there is room, and clicks in one do not close it.
    drawn: Rc<Drawn>,
}

#[derive(Default)]
struct Drawn {
    main: std::cell::Cell<Option<gpui::Bounds<Pixels>>>,
    /// Each open submenu's, by depth (0: the first beside the menu).
    subs: std::cell::RefCell<Vec<Option<gpui::Bounds<Pixels>>>>,
    window: std::cell::Cell<gpui::Size<Pixels>>,
}

impl Drawn {
    fn sub(&self, depth: usize) -> Option<gpui::Bounds<Pixels>> {
        self.subs.borrow().get(depth).copied().flatten()
    }

    fn set_sub(&self, depth: usize, bounds: gpui::Bounds<Pixels>) {
        let mut subs = self.subs.borrow_mut();
        if subs.len() <= depth {
            subs.resize(depth + 1, None);
        }
        subs[depth] = Some(bounds);
    }

    /// Whether `at` is inside an open submenu.
    fn in_a_sub(&self, at: Point<Pixels>) -> bool {
        self.subs.borrow().iter().flatten().any(|b| b.contains(&at))
    }
}

impl<V> Menu<V> {
    pub fn new(position: Point<Pixels>, entries: Vec<MenuEntry<V>>) -> Self {
        Menu { position, entries, subs: Vec::new(), drawn: Rc::default() }
    }

    /// Opens `entries` as the submenu at `depth` (beside item `at` of the
    /// list one level up), closing any deeper one; nothing if it is open.
    fn open_sub(&mut self, depth: usize, at: usize, entries: Vec<MenuEntry<V>>) -> bool {
        if self.subs.get(depth).is_some_and(|(i, _)| *i == at) && self.subs.len() == depth + 1 {
            return false;
        }
        self.subs.truncate(depth);
        self.subs.push((at, entries));
        true
    }
}

/// A view that shows menus, one at a time.
pub trait MenuHost: Sized + 'static {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>>;

    /// After the menu goes, whether something was chosen or not.
    fn menu_closed(&mut self, window: &mut Window, cx: &mut Context<Self>);

    fn open_menu(&mut self, position: Point<Pixels>, entries: Vec<MenuEntry<Self>>, cx: &mut Context<Self>) {
        *self.menu_slot() = Some(Menu::new(position, entries));
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
    let drawn = menu.drawn.clone();
    // Records where a list was drawn: the menu (`None`) or a submenu by depth.
    let measure = |level: Option<usize>| {
        let drawn = drawn.clone();
        gpui::canvas(
            move |bounds, window, _| {
                match level {
                    None => drawn.main.set(Some(bounds)),
                    Some(depth) => drawn.set_sub(depth, bounds),
                }
                drawn.window.set(window.viewport_size());
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    };
    let closer = drawn.clone();
    // The menu stays where it opened, kept inside the window.
    let main = list(&menu.entries, 0, menu.subs.first().map(|(i, _)| *i), ui, cx)
        .relative()
        .child(measure(None))
        .on_mouse_down_out(cx.listener(move |this: &mut V, e: &gpui::MouseDownEvent, window, cx| {
            // In a submenu is not outside.
            if closer.in_a_sub(e.position) {
                return;
            }
            *this.menu_slot() = None;
            this.menu_closed(window, cx);
        }));
    let mut out = div().child(deferred(anchored().position(menu.position).snap_to_window_with_margin(px(8.0)).child(main)).priority(1));
    drawn.subs.borrow_mut().truncate(menu.subs.len());
    for (depth, (at, entries)) in menu.subs.iter().enumerate() {
        // Beside the item that opened it, its first item level with it: on
        // the right where there is room, else on the left; the lists before
        // it do not move.
        let (parent, parent_entries) = match depth {
            0 => (drawn.main.get().unwrap_or(gpui::Bounds::new(menu.position, gpui::size(px(200.0), px(0.0)))), &menu.entries),
            d => (drawn.sub(d - 1).unwrap_or(gpui::Bounds::new(menu.position, gpui::size(px(200.0), px(0.0)))), &menu.subs[d - 1].1),
        };
        let width = drawn.sub(depth).map_or(px(220.0), |b| b.size.width);
        let room = drawn.window.get().width;
        let x = if parent.right() + width + px(8.0) <= room || room == px(0.0) { parent.right() - px(2.0) } else { (parent.left() - width + px(2.0)).max(px(0.0)) };
        // (Measured inside the list's 1px border.)
        let y = parent.top() - px(1.0) + entry_top(parent_entries, *at) - px(LIST_PAD);
        let open = menu.subs.get(depth + 1).map(|(i, _)| *i);
        let sub = list(entries, depth + 1, open, ui, cx).relative().child(measure(Some(depth)));
        out = out.child(deferred(anchored().position(gpui::point(x, y)).snap_to_window_with_margin(px(8.0)).child(sub)).priority(2 + depth));
    }
    out.into_any_element()
}

/// One list of a menu: the menu itself (`depth` 0), or a submenu beside the
/// list before it (`depth` 1 and on); `open`: its item whose submenu is open.
fn list<V: MenuHost>(entries: &[MenuEntry<V>], depth: usize, open: Option<usize>, ui: &UiColors, cx: &mut Context<V>) -> gpui::Stateful<gpui::Div> {
    let base = depth * 10_000;
    let mut list = div()
        .id(("menu-list", depth))
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
    let icons = entries.iter().any(|e| {
        matches!(e, MenuEntry::Item { icon: Some(_), .. } | MenuEntry::Item { dot: Some(_), .. } | MenuEntry::More { icon: Some(_), .. } | MenuEntry::Disabled { icon: Some(_), .. })
    });
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
                    // Opens beside on hover (or click), at any depth.
                    .on_hover(cx.listener(move |this: &mut V, hovered: &bool, _, cx| {
                        if !*hovered {
                            return;
                        }
                        let open = this.menu_slot().as_ref().is_some_and(|m| m.subs.get(depth).is_some_and(|(at, _)| *at == i));
                        if open {
                            return;
                        }
                        let entries = on_hover(this, cx);
                        if let Some(menu) = this.menu_slot()
                            && menu.open_sub(depth, i, entries)
                        {
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this: &mut V, _, _, cx| {
                        let entries = on_click(this, cx);
                        if let Some(menu) = this.menu_slot() {
                            menu.subs.truncate(depth);
                            menu.subs.push((i, entries));
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
            MenuEntry::Disabled { label, icon: glyph, note, tip } => div()
                .id(("menu-item", base + i))
                .h(crate::theme::us(ROW))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .text_color(hsla(ui.text_dim))
                .children(lead(glyph, ui.text_dim))
                .child(div().flex_1().child(label.clone()))
                .child(div().text_size(crate::theme::us(11.0)).child(note.clone()))
                .when_some(tip.clone(), |d, t| d.tooltip(tooltip_text(t)))
                .into_any_element(),
            MenuEntry::Item { label, icon: glyph, danger, action, dot, tip } => {
                let action = action.clone();
                let color = if *danger { ui.danger } else { ui.text };
                let lead = match dot {
                    Some(c) => Some(div().size(px(14.0)).flex().items_center().justify_center().child(div().size(px(10.0)).rounded_full().bg(hsla(*c))).into_any_element()),
                    None => lead(glyph, if *danger { color } else { ui.text_dim }),
                };
                let selector = format!("menu-item-{label}");
                row(i)
                    .debug_selector(move || selector)
                    .text_color(hsla(color))
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .children(lead)
                    .child(label.clone())
                    .when_some(tip.clone(), |d, t| d.tooltip(tooltip_text(t)))
                    // Moving onto another item of a list closes the submenus beside it.
                    .on_hover(cx.listener(move |this: &mut V, hovered: &bool, _, cx| {
                        if *hovered
                            && let Some(menu) = this.menu_slot()
                            && menu.subs.len() > depth
                        {
                            menu.subs.truncate(depth);
                            cx.notify();
                        }
                    }))
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Host {
        menu: Option<Menu<Host>>,
    }

    impl MenuHost for Host {
        fn menu_slot(&mut self) -> &mut Option<Menu<Self>> {
            &mut self.menu
        }
        fn menu_closed(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let ui = UiColors::default();
            div().size_full().children(self.menu.as_ref().map(|m| render(m, &ui, cx)))
        }
    }

    fn entries() -> Vec<MenuEntry<Host>> {
        vec![
            item("One", |_: &mut Host, _, _| {}),
            more("More", |_: &mut Host, _| vec![item("A longer item in the submenu", |_: &mut Host, _, _| {})]),
        ]
    }

    #[gpui::test]
    fn a_submenu_goes_where_there_is_room_and_the_menu_stays(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::prompt::CurrentUi(UiColors::default())));
        let (host, cx) = cx.add_window_view(|_, _| Host { menu: None });
        cx.simulate_resize(gpui::size(px(800.0), px(600.0)));
        // Opened near the right edge.
        host.update(cx, |h, _| h.menu = Some(Menu::new(gpui::point(px(560.0), px(40.0)), entries())));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let before = host.update(cx, |h, _| h.menu.as_ref().unwrap().drawn.main.get()).expect("drawn");
        host.update(cx, |h, _| {
            let menu = h.menu.as_mut().unwrap();
            menu.subs = vec![(1, vec![item("A longer item in the submenu", |_: &mut Host, _, _| {})])];
        });
        for _ in 0..2 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        let (main, sub) = host.update(cx, |h, _| {
            let d = &h.menu.as_ref().unwrap().drawn;
            (d.main.get().unwrap(), d.sub(0).unwrap())
        });
        assert_eq!(main.origin, before.origin, "the menu stays where it was");
        assert!(sub.right() <= main.left() + px(4.0), "no room on the right: on the left ({sub:?} vs {main:?})");
        // With room on the right, it goes there.
        host.update(cx, |h, _| {
            let mut m = Menu::new(gpui::point(px(40.0), px(40.0)), entries());
            m.subs = vec![(1, vec![item("A longer item in the submenu", |_: &mut Host, _, _| {})])];
            h.menu = Some(m);
        });
        for _ in 0..3 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        let (main, sub) = host.update(cx, |h, _| {
            let d = &h.menu.as_ref().unwrap().drawn;
            (d.main.get().unwrap(), d.sub(0).unwrap())
        });
        assert!(sub.left() >= main.right() - px(4.0), "room on the right: there ({sub:?} vs {main:?})");
        assert_eq!(sub.top(), main.top() + entry_top(&entries(), 1) - px(LIST_PAD), "level with its item");
    }

    #[gpui::test]
    fn submenus_open_beside_each_other_at_any_depth(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::prompt::CurrentUi(UiColors::default())));
        let (host, cx) = cx.add_window_view(|_, _| Host { menu: None });
        cx.simulate_resize(gpui::size(px(1200.0), px(600.0)));
        let second = || {
            vec![
                item("Terminal", |_: &mut Host, _, _| {}),
                more("AI", |_: &mut Host, _| vec![item("~/projects/docs", |_: &mut Host, _, _| {})]),
            ]
        };
        host.update(cx, |h, _| {
            let mut m = Menu::new(gpui::point(px(40.0), px(40.0)), entries());
            m.open_sub(0, 1, second());
            m.open_sub(1, 1, vec![item("~/projects/docs", |_: &mut Host, _, _| {})]);
            h.menu = Some(m);
        });
        for _ in 0..4 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        let (first, third) = host.update(cx, |h, _| {
            let d = &h.menu.as_ref().unwrap().drawn;
            (d.sub(0).expect("second level drawn"), d.sub(1).expect("third level drawn"))
        });
        assert!(third.left() >= first.right() - px(4.0), "beside the second level ({third:?} vs {first:?})");
        assert_eq!(third.top(), first.top() + entry_top(&second(), 1) - px(LIST_PAD), "level with its item");
        // Opening another branch of the menu closes the deeper ones.
        host.update(cx, |h, _| {
            let m = h.menu.as_mut().unwrap();
            assert!(m.open_sub(0, 0, second()));
            assert_eq!(m.subs.len(), 1);
            assert!(!m.open_sub(0, 0, second()), "already open");
        });
    }
}
//! Right-click menus, for any view that shows them, and tooltips. A menu
//! is one panel; a submenu is shown in its place (drill-down), not beside it.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Bounds, Context, FocusHandle, KeyDownEvent, Pixels, Point, ScrollHandle, SharedString, Size, Window,
    anchored, deferred, div, point, prelude::*, px, svg,
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
    /// Shows another level of the menu in its place.
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

/// A menu: one panel, which stays where it opened. Choosing an item with
/// more behind it (`More`) shows that level in the panel's place, with
/// **‹ Back** and the way there (each step clickable) above it.
///
/// Keys (the menu has the keyboard while open): ↑ ↓ light a row, Enter
/// chooses it (→ too, on a `More`), ← goes back, Esc closes, and typing
/// narrows the level by label (Backspace takes a letter off, then goes back).
/// The mouse over a row only lights it.
pub struct Menu<V> {
    pub position: Point<Pixels>,
    /// The top level.
    pub entries: Vec<MenuEntry<V>>,
    /// The levels gone into, outermost first.
    path: Vec<Level<V>>,
    /// The lit row: its index in the level shown.
    lit: Option<usize>,
    /// What was typed to narrow the level shown.
    filter: String,
    scroll: ScrollHandle,
    bar: crate::scrollbar::State,
    drawn: Rc<Drawn>,
}

struct Level<V> {
    label: SharedString,
    entries: Vec<MenuEntry<V>>,
    /// The item one level up that opened it.
    from: usize,
}

#[derive(Default)]
struct Drawn {
    /// Where the panel was drawn last (inside its 1px border).
    bounds: Cell<Option<Bounds<Pixels>>>,
    /// Where it settled when it first showed, inside the window: it stays there.
    at: Cell<Option<Point<Pixels>>>,
    /// The widest it has been, so it does not narrow from level to level.
    widest: Cell<Pixels>,
    window: Cell<Size<Pixels>>,
    /// The menu has the keyboard while open, and gives it back to `before`.
    focus: OnceCell<FocusHandle>,
    focus_taken: Cell<bool>,
    before: RefCell<Option<FocusHandle>>,
}

impl<V> Menu<V> {
    pub fn new(position: Point<Pixels>, entries: Vec<MenuEntry<V>>) -> Self {
        Menu { position, entries, path: Vec::new(), lit: None, filter: String::new(), scroll: ScrollHandle::new(), bar: Default::default(), drawn: Rc::default() }
    }

    /// Shows `entries` as the top level (what the menu offers changed while open).
    pub fn reset(&mut self, entries: Vec<MenuEntry<V>>) {
        self.entries = entries;
        self.path.clear();
        self.lit = None;
        self.filter.clear();
        self.to_top();
    }

    /// How many levels in it is (0: the top).
    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// The level shown.
    fn level(&self) -> &[MenuEntry<V>] {
        self.path.last().map_or(&self.entries, |l| &l.entries)
    }

    /// The level's entries shown (their indices): all, or the ones whose label has what was typed.
    fn shown(&self) -> Vec<usize> {
        let level = self.level();
        if self.filter.is_empty() {
            return (0..level.len()).collect();
        }
        let typed = self.filter.to_lowercase();
        (0..level.len()).filter(|&i| label_of(&level[i]).is_some_and(|l| l.to_lowercase().contains(&typed))).collect()
    }

    fn choosable(&self, i: usize) -> bool {
        matches!(self.level().get(i), Some(MenuEntry::Item { .. } | MenuEntry::More { .. }))
    }

    /// Lights the next (`down`) or previous row that can be chosen, round the ends.
    fn step(&mut self, down: bool) {
        let rows: Vec<usize> = self.shown().into_iter().filter(|&i| self.choosable(i)).collect();
        if rows.is_empty() {
            self.lit = None;
            return;
        }
        let at = self.lit.and_then(|l| rows.iter().position(|&r| r == l));
        let next = match (at, down) {
            (None, true) => 0,
            (None, false) => rows.len() - 1,
            (Some(a), true) => (a + 1) % rows.len(),
            (Some(a), false) => (a + rows.len() - 1) % rows.len(),
        };
        self.lit = Some(rows[next]);
        self.scroll_to_lit();
    }

    /// Lights the first row that can be chosen.
    fn light_first(&mut self) {
        self.lit = None;
        self.step(true);
    }

    fn scroll_to_lit(&self) {
        if let Some(pos) = self.lit.and_then(|l| self.shown().iter().position(|&i| i == l)) {
            self.scroll.scroll_to_item(pos);
        }
    }

    fn to_top(&self) {
        self.scroll.set_offset(point(px(0.0), px(0.0)));
    }

    /// One level up, its item that was chosen lit; nothing at the top.
    fn back(&mut self) {
        if let Some(level) = self.path.pop() {
            self.filter.clear();
            self.to_top();
            self.lit = Some(level.from);
            self.scroll_to_lit();
        }
    }

    /// Back to the level `depth` levels in.
    fn back_to(&mut self, depth: usize) {
        while self.path.len() > depth {
            self.back();
        }
    }

    /// What was typed changed: the first match is lit.
    fn narrowed(&mut self) {
        self.to_top();
        self.light_first();
    }

    /// Gives the keyboard back to what had it, if the menu still has it.
    fn give_back_focus(&self, window: &mut Window, cx: &mut App) {
        let before = self.drawn.before.borrow_mut().take();
        if let (Some(mine), Some(before)) = (self.drawn.focus.get(), before)
            && mine.is_focused(window)
        {
            window.focus(&before, cx);
        }
    }
}

fn label_of<V>(entry: &MenuEntry<V>) -> Option<&SharedString> {
    match entry {
        MenuEntry::Item { label, .. } | MenuEntry::More { label, .. } | MenuEntry::Disabled { label, .. } => Some(label),
        MenuEntry::Palette(_) | MenuEntry::Separator => None,
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

    /// Closes the menu without choosing anything.
    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        close_then(self, None, window, cx);
    }
}

/// Closes the menu, giving the keyboard back, then runs `action`.
fn close_then<V: MenuHost>(this: &mut V, action: Option<MenuAction<V>>, window: &mut Window, cx: &mut Context<V>) {
    if let Some(menu) = this.menu_slot().take() {
        menu.give_back_focus(window, cx);
    }
    if let Some(action) = action {
        action(this, window, cx);
    }
    this.menu_closed(window, cx);
    cx.notify();
}

/// Chooses row `i` of the level shown: an item runs (the menu closes), a
/// `More` shows its level (`keyed`: chosen by keys, so its first row is lit).
fn choose<V: MenuHost>(this: &mut V, i: usize, keyed: bool, window: &mut Window, cx: &mut Context<V>) {
    let (label, make) = match this.menu_slot().as_ref().and_then(|m| m.level().get(i)) {
        Some(MenuEntry::Item { action, .. }) => {
            let action = action.clone();
            return close_then(this, Some(action), window, cx);
        }
        Some(MenuEntry::More { label, entries, .. }) => (label.clone(), entries.clone()),
        _ => return,
    };
    let entries = make(this, cx);
    if let Some(menu) = this.menu_slot() {
        menu.path.push(Level { label, entries, from: i });
        menu.filter.clear();
        menu.to_top();
        if keyed {
            menu.light_first();
        } else {
            menu.lit = None;
        }
    }
    cx.notify();
}

fn on_key<V: MenuHost>(this: &mut V, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<V>) {
    let ks = &e.keystroke;
    let m = ks.modifiers;
    // Shortcuts are the window's.
    if m.control || m.alt || m.platform || m.function {
        return;
    }
    let key = ks.key.as_str();
    let typed = ks
        .key_char
        .clone()
        .or_else(|| match key {
            "space" => Some(" ".to_string()),
            k if k.chars().count() == 1 => Some(k.to_string()),
            _ => None,
        })
        .filter(|c| !c.chars().any(char::is_control));
    let Some(menu) = this.menu_slot() else { return };
    // Nothing else hears keys while the menu is open.
    cx.stop_propagation();
    match key {
        "escape" => return close_then(this, None, window, cx),
        "up" | "down" => menu.step(key == "down"),
        "home" | "end" => {
            menu.lit = None;
            menu.step(key == "home");
        }
        "enter" | "right" => {
            let lit = menu.lit.filter(|&i| menu.choosable(i));
            if let Some(i) = lit
                && (key == "enter" || matches!(menu.level().get(i), Some(MenuEntry::More { .. })))
            {
                return choose(this, i, true, window, cx);
            }
        }
        "left" => menu.back(),
        "backspace" => {
            if menu.filter.pop().is_some() {
                menu.narrowed();
            } else {
                menu.back();
            }
        }
        _ => match typed {
            Some(c) if !(c == " " && menu.filter.is_empty()) => {
                menu.filter.push_str(&c);
                menu.narrowed();
            }
            _ => return,
        },
    }
    cx.notify();
}

/// Rows have fixed heights.
const ROW: f32 = 26.0;
const SEPARATOR: f32 = 9.0;
const LIST_PAD: f32 = 4.0;
const SWATCH: f32 = 20.0;
const SWATCH_GAP: f32 = 6.0;
const SWATCHES_PER_ROW: usize = 8;
const MIN_WIDTH: f32 = 200.0;
const MAX_WIDTH: f32 = 480.0;
/// Longer levels (a long list of folders) scroll.
const MAX_HEIGHT: f32 = 560.0;

/// Lights row `i` while the mouse is over it.
fn light<V: MenuHost>(i: usize) -> impl Fn(&mut V, &bool, &mut Window, &mut Context<V>) + 'static {
    move |this, hovered, _, cx| {
        let Some(menu) = this.menu_slot() else { return };
        if *hovered && menu.lit != Some(i) {
            menu.lit = Some(i);
            cx.notify();
        } else if !*hovered && menu.lit == Some(i) {
            menu.lit = None;
            cx.notify();
        }
    }
}

pub fn render<V: MenuHost>(menu: &Menu<V>, ui: &UiColors, cx: &mut Context<V>) -> AnyElement {
    let us = crate::theme::us;
    let drawn = menu.drawn.clone();
    let focus = drawn.focus.get_or_init(|| cx.focus_handle()).clone();
    let window = drawn.window.get();
    let at = drawn.at.get();
    // As tall as the window has room for below where it settled.
    let room = match at {
        _ if window.height <= px(0.0) => us(MAX_HEIGHT),
        Some(at) => window.height - at.y - px(8.0),
        None => window.height - px(16.0),
    };
    let max_h = room.min(us(MAX_HEIGHT)).max(us(ROW * 3.0));
    // Records where the panel is, and takes the keyboard once it is there.
    let measure = gpui::canvas(
        move |bounds, window, cx| {
            drawn.bounds.set(Some(bounds));
            drawn.window.set(window.viewport_size());
            if drawn.at.get().is_none() {
                drawn.at.set(Some(bounds.origin - point(px(1.0), px(1.0))));
            }
            drawn.widest.set(drawn.widest.get().max(bounds.size.width + px(2.0)));
            if !drawn.focus_taken.replace(true) {
                let weak = Rc::downgrade(&drawn);
                window.defer(cx, move |window, cx| {
                    if let Some(drawn) = weak.upgrade()
                        && let Some(focus) = drawn.focus.get()
                    {
                        *drawn.before.borrow_mut() = window.focused(cx);
                        window.focus(focus, cx);
                    }
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    let rows = div().id("menu-rows").track_scroll(&menu.scroll).overflow_y_scroll().min_h_0().py(px(LIST_PAD)).children(rows(menu, ui, cx));
    let rows = crate::scrollbar::with_bar(rows, crate::scrollbar::Source::Handle(menu.scroll.clone()), &menu.bar, ui);
    let panel = div()
        .id("menu")
        .occlude()
        .track_focus(&focus)
        .on_key_down(cx.listener(on_key::<V>))
        .relative()
        .flex()
        .flex_col()
        .min_w(menu.drawn.widest.get().max(us(MIN_WIDTH)))
        .max_w(us(MAX_WIDTH))
        .max_h(max_h)
        .bg(hsla(ui.tab_active))
        .border_1()
        .border_color(hsla(ui.divider))
        .rounded_md()
        // No drop shadow: a blurred shadow is redrawn over the whole menu on
        // every hover change, which tripled the cost of moving through it.
        .text_size(us(13.0))
        .text_color(hsla(ui.text))
        .children(header(menu, ui, cx))
        .child(rows)
        .child(measure)
        .on_mouse_down_out(cx.listener(|this: &mut V, _: &gpui::MouseDownEvent, window, cx| close_then(this, None, window, cx)));
    // It stays where it first settled (kept inside the window).
    div().child(deferred(anchored().position(at.unwrap_or(menu.position)).snap_to_window_with_margin(px(8.0)).child(panel)).priority(1)).into_any_element()
}

/// Above a level gone into: ‹ Back, and the way there; and what was typed.
fn header<V: MenuHost>(menu: &Menu<V>, ui: &UiColors, cx: &mut Context<V>) -> Option<gpui::Div> {
    let us = crate::theme::us;
    let depth = menu.path.len();
    if depth == 0 && menu.filter.is_empty() {
        return None;
    }
    let mut header = div().flex_none().flex().flex_col().py(px(2.0)).border_b_1().border_color(hsla(ui.divider));
    if depth > 0 {
        let mut trail = div().flex_1().min_w_0().flex().items_center().gap_1().overflow_hidden().text_color(hsla(ui.text_dim));
        for (d, level) in menu.path.iter().enumerate() {
            if d > 0 {
                trail = trail.child(div().flex_none().child("›"));
            }
            let selector = format!("menu-crumb-{}", level.label);
            let crumb = div().id(("menu-crumb", d)).debug_selector(move || selector).min_w_0().px_1().rounded_sm().truncate().child(level.label.clone());
            trail = trail.child(if d + 1 == depth {
                crumb.text_color(hsla(ui.text))
            } else {
                crumb.cursor_pointer().hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text))).on_click(cx.listener(move |this: &mut V, _, _, cx| {
                    if let Some(menu) = this.menu_slot() {
                        menu.back_to(d + 1);
                        cx.notify();
                    }
                }))
            });
        }
        let back = div()
            .id("menu-back")
            .debug_selector(|| "menu-back".to_string())
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(us(22.0))
            .rounded_sm()
            .cursor_pointer()
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .child(div().text_color(hsla(ui.text_dim)).child("‹"))
            .child("Back")
            .on_click(cx.listener(|this: &mut V, _, _, cx| {
                if let Some(menu) = this.menu_slot() {
                    menu.back();
                    cx.notify();
                }
            }));
        header = header.child(
            div().h(us(ROW)).flex().items_center().gap_1().px_1().child(back).child(div().flex_none().w(px(1.0)).h(us(14.0)).bg(hsla(ui.divider))).child(trail),
        );
    }
    if !menu.filter.is_empty() {
        header = header.child(
            div()
                .mx_2()
                .my_1()
                .px_2()
                .h(us(22.0))
                .flex()
                .items_center()
                .gap_2()
                .rounded_sm()
                .border_1()
                .border_color(hsla(ui.accent))
                .bg(hsla(ui.background))
                .child(svg().path(icon("search")).flex_none().size(us(12.0)).text_color(hsla(ui.text_dim)))
                .child(div().min_w_0().truncate().child(menu.filter.clone()))
                .child(div().flex_none().w(px(1.0)).h(us(14.0)).bg(hsla(ui.text))),
        );
    }
    Some(header)
}

/// The rows of the level shown.
fn rows<V: MenuHost>(menu: &Menu<V>, ui: &UiColors, cx: &mut Context<V>) -> Vec<AnyElement> {
    let us = crate::theme::us;
    let level = menu.level();
    let shown = menu.shown();
    // Ids apart from level to level, so a row's hover and tooltip do not carry over.
    let base = menu.path.len() * 10_000;
    // With icons on some items, every label starts at the same place.
    let icons = shown.iter().any(|&i| {
        matches!(&level[i], MenuEntry::Item { icon: Some(_), .. } | MenuEntry::Item { dot: Some(_), .. } | MenuEntry::More { icon: Some(_), .. } | MenuEntry::Disabled { icon: Some(_), .. })
    });
    let lead = |glyph: &Option<&'static str>, color: Rgb| match glyph {
        Some(name) => Some(svg().path(icon(name)).flex_none().size(us(14.0)).text_color(hsla(color)).into_any_element()),
        None if icons => Some(div().flex_none().size(px(14.0)).into_any_element()),
        None => None,
    };
    let row = |i: usize| {
        div()
            .id(("menu-item", base + i))
            .h(us(ROW))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .cursor_pointer()
            .when(menu.lit == Some(i), |d| d.bg(hsla(ui.tab_hover)))
            .on_hover(cx.listener(light::<V>(i)))
            .on_click(cx.listener(move |this: &mut V, _, window, cx| choose(this, i, false, window, cx)))
    };
    let label = |text: &SharedString| div().flex_1().min_w_0().truncate().child(text.clone());
    let mut out = Vec::with_capacity(shown.len());
    for &i in &shown {
        out.push(match &level[i] {
            MenuEntry::Separator => div().h(px(SEPARATOR)).flex_none().flex().items_center().child(div().w_full().h(px(1.0)).bg(hsla(ui.divider))).into_any_element(),
            MenuEntry::Palette(swatches) => div()
                .flex()
                .flex_none()
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
                        .on_click(cx.listener(move |this: &mut V, _, window, cx| close_then(this, Some(action.clone()), window, cx)))
                }))
                .into_any_element(),
            MenuEntry::More { label: text, icon: glyph, .. } => {
                let selector = format!("menu-more-{text}");
                row(i)
                    .debug_selector(move || selector)
                    .children(lead(glyph, ui.text_dim))
                    .child(label(text))
                    .child(div().flex_none().text_color(hsla(ui.text_dim)).child("›"))
                    .into_any_element()
            }
            MenuEntry::Disabled { label: text, icon: glyph, note, tip } => div()
                .id(("menu-item", base + i))
                .h(us(ROW))
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .text_color(hsla(ui.text_dim))
                .children(lead(glyph, ui.text_dim))
                .child(label(text))
                .child(div().flex_none().text_size(us(11.0)).child(note.clone()))
                .when_some(tip.clone(), |d, t| d.tooltip(tooltip_text(t)))
                .into_any_element(),
            MenuEntry::Item { label: text, icon: glyph, danger, dot, tip, .. } => {
                let color = if *danger { ui.danger } else { ui.text };
                let lead = match dot {
                    Some(c) => Some(div().flex_none().size(px(14.0)).flex().items_center().justify_center().child(div().size(px(10.0)).rounded_full().bg(hsla(*c))).into_any_element()),
                    None => lead(glyph, if *danger { color } else { ui.text_dim }),
                };
                let selector = format!("menu-item-{text}");
                row(i)
                    .debug_selector(move || selector)
                    .text_color(hsla(color))
                    .children(lead)
                    .child(label(text))
                    .when_some(tip.clone(), |d, t| d.tooltip(tooltip_text(t)))
                    .into_any_element()
            }
        });
    }
    if shown.is_empty() && !menu.filter.is_empty() {
        out.push(div().h(us(ROW)).flex_none().flex().items_center().px_3().text_color(hsla(ui.text_dim)).child("No match").into_any_element());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, VisualTestContext};

    struct Host {
        menu: Option<Menu<Host>>,
        focus: FocusHandle,
        chosen: Vec<&'static str>,
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
            div().size_full().track_focus(&self.focus).children(self.menu.as_ref().map(|m| render(m, &ui, cx)))
        }
    }

    fn chose(name: &'static str) -> impl Fn(&mut Host, &mut Window, &mut Context<Host>) + 'static {
        move |h, _, _| h.chosen.push(name)
    }

    /// Top › New Tab to the Right › AI › folders.
    fn entries() -> Vec<MenuEntry<Host>> {
        vec![
            item("Rename Tab…", chose("rename")),
            MenuEntry::Separator,
            more("New Tab to the Right", |_: &mut Host, _| {
                vec![
                    item("New Terminal Tab", chose("terminal")),
                    disabled("New Remote Tab", "offline"),
                    more("AI", |_: &mut Host, _| {
                        (0..30).map(|n| item(format!("~/projects/folder-{n}"), chose("folder"))).chain([item("~/docs", chose("docs"))]).collect()
                    }),
                ]
            }),
            more("Add Tag", |_: &mut Host, _| vec![item("work", chose("work"))]),
            danger("Close Tab…", chose("close")),
        ]
    }

    fn open(cx: &mut gpui::TestAppContext, at: Point<Pixels>) -> (gpui::Entity<Host>, &mut VisualTestContext) {
        cx.update(|cx| cx.set_global(crate::prompt::CurrentUi(UiColors::default())));
        let (host, cx) = cx.add_window_view(|_, cx| Host { menu: None, focus: cx.focus_handle(), chosen: Vec::new() });
        cx.simulate_resize(gpui::size(px(1000.0), px(700.0)));
        host.update_in(cx, |h, window, cx| {
            window.focus(&h.focus, cx);
            h.menu = Some(Menu::new(at, entries()));
        });
        settle(cx);
        (host, cx)
    }

    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let at = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} not drawn")).center();
        cx.simulate_click(at, Modifiers::none());
        settle(cx);
    }

    fn keys(cx: &mut VisualTestContext, keys: &str) {
        cx.simulate_keystrokes(keys);
        settle(cx);
    }

    fn depth(host: &gpui::Entity<Host>, cx: &mut VisualTestContext) -> Option<usize> {
        host.update(cx, |h, _| h.menu.as_ref().map(|m| m.depth()))
    }

    fn panel(host: &gpui::Entity<Host>, cx: &mut VisualTestContext) -> Bounds<Pixels> {
        host.update(cx, |h, _| h.menu.as_ref().and_then(|m| m.drawn.bounds.get())).expect("drawn")
    }

    #[gpui::test]
    fn levels_open_in_place_by_click_and_keys(cx: &mut gpui::TestAppContext) {
        let (host, cx) = open(cx, point(px(40.0), px(40.0)));
        let first = panel(&host, cx);
        // A click goes in: the level shows in the panel's place, nothing beside it.
        click(cx, "menu-more-New Tab to the Right");
        assert_eq!(depth(&host, cx), Some(1));
        assert!(cx.debug_bounds("menu-item-Rename Tab…").is_none(), "the top level is gone");
        assert!(cx.debug_bounds("menu-item-New Terminal Tab").is_some());
        assert_eq!(panel(&host, cx).origin, first.origin, "in the same place");
        click(cx, "menu-back");
        assert_eq!(depth(&host, cx), Some(0));
        // Keys: ↓ skips the separator; → goes in, ← comes back, Enter goes in too.
        keys(cx, "down down");
        keys(cx, "right");
        assert_eq!(depth(&host, cx), Some(1));
        keys(cx, "left");
        assert_eq!(depth(&host, cx), Some(0));
        let lit = host.update(cx, |h, _| h.menu.as_ref().unwrap().lit);
        assert_eq!(lit, Some(2), "back on the item it came from");
        keys(cx, "enter");
        // In: Terminal is lit; ↓ skips the disabled Remote; Enter goes into AI.
        keys(cx, "down enter");
        assert_eq!(depth(&host, cx), Some(2));
        keys(cx, "enter");
        assert_eq!(host.update(cx, |h, _| (h.menu.is_none(), h.chosen.clone())), (true, vec!["folder"]));
        // The keyboard went back where it was.
        let back = host.update_in(cx, |h, window, _| h.focus.is_focused(window));
        assert!(back, "focus given back");
    }

    #[gpui::test]
    fn the_way_there_goes_back_and_escape_closes_from_any_depth(cx: &mut gpui::TestAppContext) {
        let (host, cx) = open(cx, point(px(40.0), px(40.0)));
        keys(cx, "down down enter down enter");
        assert_eq!(depth(&host, cx), Some(2));
        let trail = host.update(cx, |h, _| h.menu.as_ref().unwrap().path.iter().map(|l| l.label.to_string()).collect::<Vec<_>>());
        assert_eq!(trail, ["New Tab to the Right", "AI"]);
        // A step of the way jumps back to it.
        click(cx, "menu-crumb-New Tab to the Right");
        assert_eq!(depth(&host, cx), Some(1));
        // Three levels in (a third from a test-made level), Escape closes it all.
        host.update(cx, |h, _| {
            let m = h.menu.as_mut().unwrap();
            m.path.push(Level { label: "Deeper".into(), entries: vec![item("x", chose("x"))], from: 0 });
            m.path.push(Level { label: "Deepest".into(), entries: vec![item("y", chose("y"))], from: 0 });
        });
        settle(cx);
        assert_eq!(depth(&host, cx), Some(3));
        keys(cx, "escape");
        assert!(host.update(cx, |h, _| h.menu.is_none() && h.chosen.is_empty()));
        assert!(host.update_in(cx, |h, window, _| h.focus.is_focused(window)), "focus given back");
    }

    #[gpui::test]
    fn typing_narrows_the_level(cx: &mut gpui::TestAppContext) {
        let (host, cx) = open(cx, point(px(40.0), px(40.0)));
        keys(cx, "down down enter down enter");
        assert_eq!(depth(&host, cx), Some(2));
        keys(cx, "d o c");
        let shown = host.update(cx, |h, _| {
            let m = h.menu.as_ref().unwrap();
            m.shown().iter().filter_map(|&i| label_of(&m.level()[i]).map(|l| l.to_string())).collect::<Vec<_>>()
        });
        assert_eq!(shown, ["~/docs"]);
        assert!(cx.debug_bounds("menu-item-~/projects/folder-0").is_none());
        // Backspace edits what was typed first, then goes back.
        keys(cx, "backspace backspace backspace");
        assert_eq!(depth(&host, cx), Some(2));
        assert_eq!(host.update(cx, |h, _| h.menu.as_ref().unwrap().shown().len()), 31);
        keys(cx, "backspace");
        assert_eq!(depth(&host, cx), Some(1));
        // Enter chooses the first match.
        keys(cx, "t e r m enter");
        assert_eq!(host.update(cx, |h, _| h.chosen.clone()), ["terminal"]);
    }

    #[gpui::test]
    fn the_panel_keeps_its_place_and_width(cx: &mut gpui::TestAppContext) {
        // Opened near the bottom right: it settles inside the window and stays.
        let (host, cx) = open(cx, point(px(900.0), px(600.0)));
        let top = panel(&host, cx);
        assert!(top.right() <= px(1000.0) && top.bottom() <= px(700.0), "inside the window: {top:?}");
        keys(cx, "down down enter");
        // Against the window's right edge, a wider level grows to the left.
        let second = panel(&host, cx);
        assert_eq!(second.top(), top.top(), "at the same height");
        assert!(second.right() <= px(1000.0), "inside the window: {second:?}");
        assert!(second.size.width >= top.size.width, "no narrower ({second:?} vs {top:?})");
        // The long list of folders scrolls inside the window.
        keys(cx, "down enter");
        let third = panel(&host, cx);
        assert_eq!(third.top(), top.top());
        assert!(third.size.width >= second.size.width);
        assert!(third.bottom() <= px(700.0), "fits the window: {third:?}");
        // Back up, the top level keeps the widest width seen.
        keys(cx, "left left");
        assert!(panel(&host, cx).size.width >= third.size.width);
        // ↑ goes round to the last folder, which is scrolled to.
        keys(cx, "enter down enter");
        let hidden = cx.debug_bounds("menu-item-~/docs").expect("laid out");
        assert!(hidden.top() >= panel(&host, cx).bottom(), "below the end at first: {hidden:?}");
        keys(cx, "up");
        let lit = cx.debug_bounds("menu-item-~/docs").expect("drawn");
        assert!(lit.bottom() <= panel(&host, cx).bottom() + px(1.0), "scrolled into view: {lit:?}");
    }
}

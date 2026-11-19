//! A bench's links: web pages that go with its work, shown as chips on its
//! tab bar and in Shed, opened through the browser extension when it is
//! there (bringing an open tab forward) and the usual way when not.

use gpui::{App, ClipboardItem, Context, Div, SharedString, Stateful, div, prelude::*, svg};
use shed_model::{ProjectLink, Rgb};

use crate::assets::icon;
use crate::menu::{self, MenuEntry};
use crate::theme::{UiColors, hsla, hsla_alpha};

pub use shed_model::links::{parse, renamed, unparse, with};

/// Opens a link, in a tab of its own when `new_tab`.
pub fn open(url: String, new_tab: bool, cx: &mut App) {
    cx.spawn(async move |cx| {
        let u = url.clone();
        let asked = cx.background_executor().spawn(async move { crate::browser::open(&u, new_tab) }).await;
        if asked.is_err() {
            cx.update(|cx| cx.open_url(&url));
        }
    })
    .detach();
}

/// A link's chip; the caller adds what clicking does.
pub fn chip(id: impl Into<SharedString>, link: &ProjectLink, accent: Rgb, ui: &UiColors) -> Stateful<Div> {
    let ink = ui.ink(accent);
    div()
        .id(id.into())
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .max_w(crate::theme::us(180.0))
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .border_color(hsla_alpha(accent, 0.5))
        .cursor_pointer()
        .text_size(crate::theme::us(12.0))
        .text_color(hsla(ink))
        .hover(move |s| s.bg(hsla_alpha(accent, 0.15)))
        .child(svg().path(icon("link")).size(crate::theme::us(12.0)).flex_none().text_color(hsla(ink)))
        .child(div().min_w_0().truncate().child(link.name.clone()))
}

/// A link's right-click menu. `set` stores the bench's links; `rename`
/// starts editing the link's name, `edit` its name and address.
pub fn menu<V: 'static>(
    links: Vec<ProjectLink>,
    i: usize,
    set: impl Fn(&mut V, Vec<ProjectLink>, &mut Context<V>) + 'static,
    rename: impl Fn(&mut V, &mut gpui::Window, &mut Context<V>) + 'static,
    edit: impl Fn(&mut V, &mut gpui::Window, &mut Context<V>) + 'static,
) -> Vec<MenuEntry<V>> {
    let Some(link) = links.get(i).cloned() else { return Vec::new() };
    let (a, b, c) = (link.url.clone(), link.url.clone(), link.url.clone());
    vec![
        menu::item("Open", move |_: &mut V, _, cx| open(a.clone(), false, cx)).icon("link"),
        menu::item("Open in New Tab", move |_: &mut V, _, cx| open(b.clone(), true, cx)),
        menu::item("Copy URL", move |_: &mut V, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(c.clone()))),
        menu::item("Rename…", rename).icon("pencil"),
        menu::item("Change Address…", edit),
        MenuEntry::Separator,
        menu::danger("Remove Link", move |this: &mut V, _, cx| {
            let rest: Vec<ProjectLink> = links.iter().filter(|l| l.name != link.name).cloned().collect();
            set(this, rest, cx);
        })
        .icon("close"),
    ]
}

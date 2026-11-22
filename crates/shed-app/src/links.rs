//! A bench's links: web pages that go with its work, shown as chips on its
//! tab bar and in Shed, opened through the browser extension when it is
//! there (bringing an open tab forward) and the usual way when not.
//!
//! While another workspace is shown, what it says to open (its benches'
//! links, its terminals' links, Claude's answers there) opens here only if
//! it is a web page (`may_open`): never a file, nor another scheme's handler.

use gpui::{App, ClipboardItem, Context, Div, SharedString, Stateful, div, prelude::*, svg};
use shed_model::{ProjectLink, Rgb};

use crate::assets::icon;
use crate::menu::{self, MenuEntry};
use crate::theme::{UiColors, hsla, hsla_alpha};

pub use shed_model::links::{parse, renamed, unparse, with};

/// Whether the desktop opens `url` for the workspace shown: anything for
/// this PC's; for another's (`remote`), only `http` and `https` addresses.
pub fn may_open(url: &str, remote: bool) -> bool {
    if !remote {
        return true;
    }
    let url = url.trim_start().to_ascii_lowercase();
    url.starts_with("http://") || url.starts_with("https://")
}

/// Whether another workspace is shown.
fn showing_remote(cx: &App) -> bool {
    cx.try_global::<crate::state::AppHandle>().and_then(|h| h.0.upgrade()).is_some_and(|s| s.read(cx).is_remote())
}

/// Opens `url` with the system's handler, as `may_open` lets it.
pub fn open_external(url: &str, cx: &mut App) {
    if !may_open(url, showing_remote(cx)) {
        tracing::warn!("not opening a link from another workspace that is not a web page");
        return;
    }
    cx.open_url(url);
}

/// Opens a link, in a tab of its own when `new_tab`.
pub fn open(url: String, new_tab: bool, cx: &mut App) {
    if !may_open(&url, showing_remote(cx)) {
        tracing::warn!("not opening a link from another workspace that is not a web page");
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::may_open;

    #[test]
    fn another_workspace_opens_only_web_pages() {
        for url in ["https://example.com/x", "http://localhost:3000", "HTTPS://Example.com"] {
            assert!(may_open(url, true), "{url}");
        }
        for url in ["file:///etc/passwd", "file:///tmp/x.desktop", "mailto:a@b.c", "ssh://host", "javascript:alert(1)", "/home/me/x", "vscode://file/x", ""] {
            assert!(!may_open(url, true), "{url}");
            assert!(may_open(url, false), "this PC's: as before ({url})");
        }
    }
}

//! A bench's links: web pages that go with its work, shown as chips on its
//! tab bar and in Shed, opened through the browser extension when it is
//! there (bringing an open tab forward) and the usual way when not.

use gpui::{App, ClipboardItem, Context, Div, SharedString, Stateful, div, prelude::*, svg};
use shed_model::{ProjectLink, Rgb};

use crate::assets::icon;
use crate::menu::{self, MenuEntry};
use crate::theme::{UiColors, hsla, hsla_alpha};

/// A link as typed: `name url`, or just a URL (named by its host).
/// `localhost:3000` becomes `http://…`, other bare hosts `https://…`.
pub fn parse(text: &str) -> Option<ProjectLink> {
    let text = text.trim();
    let (name, url) = match text.rsplit_once(char::is_whitespace) {
        Some((name, url)) => (name.trim().to_string(), url.to_string()),
        None => (String::new(), text.to_string()),
    };
    let url = if url.contains("://") {
        url
    } else if url.starts_with("localhost") || url.starts_with("127.") {
        format!("http://{url}")
    } else {
        format!("https://{url}")
    };
    let host = url.split("://").nth(1)?.split(['/', '?', '#']).next()?.to_string();
    if host.is_empty() {
        return None;
    }
    Some(ProjectLink { name: if name.is_empty() { host } else { name }, url })
}

/// How a link is typed back: `name url`.
pub fn unparse(link: &ProjectLink) -> String {
    format!("{} {}", link.name, link.url)
}

/// `links` with `link` in place of the one at `at` (or of the one with its
/// name), else added at the end.
pub fn with(links: &[ProjectLink], link: ProjectLink, at: Option<usize>) -> Vec<ProjectLink> {
    let mut out: Vec<ProjectLink> = links.to_vec();
    match at.filter(|i| *i < out.len()).or_else(|| out.iter().position(|l| l.name == link.name)) {
        Some(i) => {
            out[i] = link;
            // Renamed onto another's name: that one goes.
            let name = out[i].name.clone();
            let mut j = 0;
            out.retain(|l| {
                j += 1;
                j - 1 == i || l.name != name
            });
            out
        }
        None => {
            out.push(link);
            out
        }
    }
}

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

/// `links` with the one at `i` called `name` (the address stays).
pub fn renamed(links: &[ProjectLink], i: usize, name: &str) -> Option<Vec<ProjectLink>> {
    let name = name.trim();
    let link = links.get(i)?;
    (!name.is_empty()).then(|| with(links, ProjectLink { name: name.to_string(), url: link.url.clone() }, Some(i)))
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
    use super::*;

    #[test]
    fn links_are_typed_name_then_url() {
        assert_eq!(parse("app localhost:3000").unwrap(), ProjectLink { name: "app".into(), url: "http://localhost:3000".into() });
        assert_eq!(parse("grafana.example.com/d/x").unwrap().name, "grafana.example.com");
        assert_eq!(parse("CI board https://ci.example/").unwrap().name, "CI board");
        assert!(parse("").is_none());
        let link = |n: &str| ProjectLink { name: n.into(), url: format!("https://{n}/") };
        let links = vec![link("a"), link("b")];
        assert_eq!(with(&links, link("c"), None), vec![link("a"), link("b"), link("c")]);
        assert_eq!(with(&links, link("c"), Some(0)), vec![link("c"), link("b")]);
        assert_eq!(with(&links, link("b"), Some(0)), vec![link("b")], "renamed onto another");
        let r = renamed(&links, 1, " My App ").unwrap();
        assert_eq!((r[1].name.as_str(), r[1].url.as_str()), ("My App", "https://b/"));
        assert!(renamed(&links, 1, "  ").is_none());
    }
}

//! A bench window's bookmarks list: files, folders and AI messages, opened
//! from the Bookmarks button on the tab bar (or its key), under it.

use gpui::{
    AnyElement, ClickEvent, Context, MouseButton, MouseDownEvent, SharedString, anchored, deferred, div, point,
    prelude::*, px, svg,
};
use shed_model::bookmarks::{Group, grouped};
use shed_model::{Bookmark, BookmarkId, Rgb};
use shed_proto::ClientMsg;

use super::{BenchWindow, EditTarget, item};
use crate::assets::icon;
use crate::menu::{self, MenuEntry, MenuHost, danger};
use crate::theme::{UiColors, hsla, hsla_alpha};

/// How wide the list is.
const WIDTH: f32 = 380.0;

impl BenchWindow {
    pub(super) fn toggle_bookmarks(&mut self, cx: &mut Context<Self>) {
        if self.bookmarks_open {
            self.close_bookmarks(cx);
        } else {
            self.bookmarks_open = true;
            self.bookmark_problem = None;
            self.menu = None;
            self.redraw(cx);
        }
    }

    pub(super) fn close_bookmarks(&mut self, cx: &mut Context<Self>) {
        self.bookmarks_open = false;
        self.bookmark_problem = None;
        self.redraw(cx);
    }

    /// Goes to a bookmark; the list closes, or says why it could not.
    pub(super) fn go_to_bookmark(&mut self, id: BookmarkId, cx: &mut Context<Self>) {
        let Some(bookmark) = self.shown.bookmarks.iter().find(|b| b.id == id).cloned() else { return };
        let bench = self.bench;
        match self.state.update(cx, |state, cx| state.open_bookmark(bench, &bookmark, cx)) {
            Ok(()) => self.close_bookmarks(cx),
            Err(why) => {
                self.bookmark_problem = Some(why);
                cx.notify();
            }
        }
    }

    fn remove_bookmark(&mut self, id: BookmarkId, cx: &mut Context<Self>) {
        self.send(ClientMsg::RemoveBookmark { bench: self.bench, id }, cx);
    }

    /// A bookmark's right-click menu.
    fn bookmark_menu(&self, id: BookmarkId) -> Vec<MenuEntry<Self>> {
        vec![
            item("Open", move |this, _, cx| this.go_to_bookmark(id, cx)).icon("go"),
            item("Rename…", move |this, window, cx| {
                this.close_bookmarks(cx);
                this.start_edit(EditTarget::RenameBookmark(id), window, cx);
            })
            .icon("pencil"),
            MenuEntry::Separator,
            danger("Remove", move |this: &mut Self, _, cx| this.remove_bookmark(id, cx)).icon("close"),
        ]
    }

    pub(super) fn render_bookmarks(&self, ui: &UiColors, accent: Rgb, cx: &mut Context<Self>) -> AnyElement {
        // Under the Bookmarks button, kept inside the window.
        let button = self.bar.read(cx).layout.borrow().bookmarks_button;
        let at = button.map_or(point(px(f32::MAX), px(40.0)), |b| point(b.left(), b.bottom() + px(4.0)));
        let mut list = div()
            .id("bookmarks")
            .occlude()
            .w(px(WIDTH))
            .max_h(px(460.0))
            .overflow_y_scroll()
            .py_1()
            .bg(hsla(ui.tab_active))
            .border_1()
            .border_color(hsla(ui.divider))
            .rounded_md()
            .text_size(crate::theme::us(13.0))
            .text_color(hsla(ui.text))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .child(svg().path(icon("bookmark")).size(px(14.0)).text_color(hsla(ui.ink(accent))))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Bookmarks"))
                    .when(!self.shown.bookmarks.is_empty(), |d| d.child(div().text_color(hsla(ui.text_dim)).child(self.shown.bookmarks.len().to_string()))),
            );
        if self.shown.bookmarks.is_empty() {
            let keys = if cfg!(target_os = "macos") { "Cmd+Shift+B" } else { "Ctrl+Shift+B" };
            list = list.child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(crate::theme::us(12.0))
                    .text_color(hsla(ui.text_dim))
                    .child(format!(
                        "No bookmarks yet. Right-click a file or folder in a project tab and choose Bookmark; in a file, {keys} keeps the cursor's line; in an AI tab, hover a message and click Bookmark."
                    )),
            );
        }
        for (group, marks) in grouped(&self.shown.bookmarks) {
            list = list.child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_0p5()
                    .text_size(crate::theme::us(11.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(hsla(ui.text_dim))
                    .child(group.title().to_uppercase()),
            );
            for mark in marks {
                list = list.child(self.render_bookmark(mark, group, ui, cx));
            }
        }
        if let Some(why) = &self.bookmark_problem {
            list = list.child(div().px_3().py_1().text_size(crate::theme::us(12.0)).text_color(hsla(ui.danger)).child(why.clone()));
        }
        let list = list.on_mouse_down_out(cx.listener(|this, e: &MouseDownEvent, _, cx| {
            // A click on the button toggles it; any other outside closes it.
            let button = this.bar.read(cx).layout.borrow().bookmarks_button;
            if button.is_some_and(|b| b.contains(&e.position)) {
                return;
            }
            this.close_bookmarks(cx);
        }));
        deferred(anchored().position(at).snap_to_window_with_margin(px(8.0)).child(list)).into_any_element()
    }

    fn render_bookmark(&self, mark: &Bookmark, group: Group, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let id = mark.id;
        let glyph = match group {
            Group::Files => "file",
            Group::Folders => "folder",
            Group::Messages => "ai",
        };
        let row_group: SharedString = format!("bookmark-{id}").into();
        let detail = mark.detail(&self.shown.folder);
        div()
            .id(SharedString::from(format!("bookmark-row-{id}")))
            .group(row_group.clone())
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .cursor_pointer()
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .tooltip(menu::tooltip_text(detail.clone()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go_to_bookmark(id, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let entries = this.bookmark_menu(id);
                this.open_menu(e.position, entries, cx);
            }))
            .child(svg().path(icon(glyph)).size(px(14.0)).flex_none().text_color(hsla(ui.text_dim)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(mark.label.clone()))
                    .child(div().truncate().text_size(crate::theme::us(11.0)).text_color(hsla(ui.text_dim)).child(detail)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("bookmark-remove-{id}")))
                    .flex_none()
                    .p_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .invisible()
                    .group_hover(row_group, |s| s.visible())
                    .hover(|s| s.bg(hsla_alpha(ui.danger, 0.2)))
                    .tooltip(menu::tooltip("Remove the bookmark"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.remove_bookmark(id, cx);
                    }))
                    .child(svg().path(icon("close")).size(px(12.0)).text_color(hsla(ui.danger))),
            )
            .into_any_element()
    }
}

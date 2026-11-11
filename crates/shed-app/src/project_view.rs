//! A project tab: a folder of code. Its header names the folder and offers
//! what to do with it; on the left, its files as a tree, read a folder at a
//! time as folders are opened.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{
    ClipboardItem, Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, ScrollStrategy,
    SharedString, UniformListScrollHandle, Window, div, prelude::*, px, svg, uniform_list,
};
use shed_model::{BenchId, Rgb};
use shed_proto::ClientMsg;

use crate::assets::icon;
use crate::keeper_conn::KeeperConn;
use crate::menu::{self, Menu, MenuEntry, MenuHost};
use crate::theme::{UiColors, current_ui, hsla, hsla_alpha};

const ROW_HEIGHT: f32 = 24.0;
const INDENT: f32 = 14.0;
const TREE_WIDTH: f32 = 280.0;

/// A line of the file tree.
struct Row {
    path: PathBuf,
    name: SharedString,
    depth: usize,
    dir: bool,
    hidden: bool,
}

pub struct ProjectView {
    pub bench: BenchId,
    pub root: PathBuf,
    pub accent: Rgb,
    conn: KeeperConn,
    rows: Vec<Row>,
    /// Open folders, kept as the tree is rebuilt.
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    menu: Option<Menu<Self>>,
}

impl ProjectView {
    pub fn new(bench: BenchId, root: PathBuf, accent: Rgb, conn: KeeperConn, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            bench,
            root,
            accent,
            conn,
            rows: Vec::new(),
            expanded: HashSet::new(),
            selected: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            menu: None,
        };
        view.rebuild();
        view
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Follows the tab to another folder.
    pub fn set_root(&mut self, root: PathBuf) {
        if root != self.root {
            self.root = root;
            self.expanded.clear();
            self.selected = None;
            self.rebuild();
        }
    }

    /// Reads the tree again from the disk, keeping open folders open.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.expanded.retain(|dir| dir.is_dir());
        self.rebuild();
        cx.notify();
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        self.add_children(&self.root.clone(), 0, &mut rows);
        self.rows = rows;
    }

    fn add_children(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        for row in read_folder(dir, depth) {
            let open = row.dir && self.expanded.contains(&row.path);
            let path = row.path.clone();
            rows.push(row);
            if open {
                self.add_children(&path, depth + 1, rows);
            }
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.path == selected)
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(index) {
            self.selected = Some(row.path.clone());
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn toggle(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rebuild();
        cx.notify();
    }

    fn click_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index) else { return };
        let (path, dir) = (row.path.clone(), row.dir);
        self.selected = Some(path.clone());
        if dir {
            self.toggle(&path, cx);
        }
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let current = self.selected_index();
        let row = current.and_then(|i| self.rows.get(i)).map(|r| (r.path.clone(), r.dir, r.depth));
        match event.keystroke.key.as_str() {
            "down" => self.select(current.map_or(0, |i| (i + 1).min(self.rows.len().saturating_sub(1))), cx),
            "up" => self.select(current.map_or(0, |i| i.saturating_sub(1)), cx),
            "home" => self.select(0, cx),
            "end" => self.select(self.rows.len().saturating_sub(1), cx),
            "right" | "enter" => match row {
                Some((path, true, _)) if !self.expanded.contains(&path) => self.toggle(&path, cx),
                Some((_, true, _)) if event.keystroke.key == "right" => self.select(current.unwrap() + 1, cx),
                Some((path, true, _)) => self.toggle(&path, cx),
                _ => return,
            },
            "left" => match row {
                Some((path, true, _)) if self.expanded.contains(&path) => self.toggle(&path, cx),
                // To the folder this is in.
                Some((_, _, depth)) if depth > 0 => {
                    let i = current.unwrap();
                    if let Some(parent) = self.rows[..i].iter().rposition(|r| r.depth < depth) {
                        self.select(parent, cx);
                    }
                }
                _ => return,
            },
            _ => return,
        }
        cx.stop_propagation();
    }

    /// A new terminal tab in `dir`, next to this one.
    fn open_terminal(&self, dir: &Path) {
        self.conn.send_msg(ClientMsg::NewTab { bench: self.bench, cwd: Some(dir.to_path_buf()) });
    }

    fn row_menu(&self, path: &Path, dir: bool) -> Vec<MenuEntry<Self>> {
        let folder = if dir { path.to_path_buf() } else { path.parent().unwrap_or(&self.root).to_path_buf() };
        let relative = path.strip_prefix(&self.root).unwrap_or(path).display().to_string();
        let (full, file) = (path.display().to_string(), path.to_path_buf());
        vec![
            menu::item("Open Terminal Here", move |this: &mut Self, _, _| this.open_terminal(&folder)).icon("terminal"),
            menu::item("Show in File Manager", move |_, _, cx| cx.reveal_path(&file)).icon("folder"),
            MenuEntry::Separator,
            menu::item("Copy Path", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(full.clone()))),
            menu::item("Copy Relative Path", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(relative.clone()))),
        ]
    }

    fn render_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let ui = current_ui(cx);
        let accent = self.accent;
        let selected = self.selected_index();
        range
            .filter_map(|i| {
                let row = self.rows.get(i)?;
                let (path, dir) = (row.path.clone(), row.dir);
                let open = dir && self.expanded.contains(&row.path);
                let color = if row.hidden { ui.text_dim } else { ui.text };
                let mut el = div()
                    .id(i)
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .pl(px(8.0 + row.depth as f32 * INDENT))
                    .pr_2()
                    .cursor_pointer()
                    .text_color(hsla(color))
                    .on_click(cx.listener(move |this, _, _, cx| this.click_row(i, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        this.selected = Some(path.clone());
                        let entries = this.row_menu(&path, dir);
                        this.open_menu(e.position, entries, cx);
                    }));
                el = if selected == Some(i) {
                    el.bg(hsla_alpha(accent, 0.25))
                } else {
                    el.hover(|s| s.bg(hsla(ui.tab_hover)))
                };
                let chevron = if !dir { "" } else if open { "▾" } else { "▸" };
                let glyph = if dir { "folder" } else { "file" };
                let glyph_color = if dir { ui.ink(accent) } else { ui.text_dim };
                Some(
                    el.child(div().w(px(10.0)).flex_none().text_color(hsla(ui.text_dim)).child(chevron))
                        .child(svg().path(icon(glyph)).size(px(14.0)).flex_none().text_color(hsla(glyph_color)))
                        .child(div().truncate().child(row.name.clone()))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_header(&self, ui: &UiColors, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.root.display().to_string());
        let button = |id: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_color(hsla(ui.text_dim))
                .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                .child(svg().path(icon(glyph)).size(px(14.0)).text_color(hsla(ui.text_dim)))
                .child(label)
        };
        let root = self.root.clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(36.0))
            .px_3()
            .border_b_1()
            .border_color(hsla(ui.divider))
            .bg(hsla(ui.background))
            .child(svg().path(icon("project")).size(px(16.0)).flex_none().text_color(hsla(ui.ink(self.accent))))
            .child(div().flex_none().text_size(px(14.0)).text_color(hsla(ui.text)).child(name))
            .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child(shed_save::home_relative(&self.root)))
            .child(button("project-terminal", "terminal", "Terminal").on_click(cx.listener(|this, _, _, _| this.open_terminal(&this.root.clone()))))
            .child(button("project-files", "folder", "File Manager").on_click(move |_, _, cx| cx.open_with_system(&root)))
            .child(button("project-refresh", "refresh", "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
    }
}

/// A folder's entries, folders first, each group by name; `.git` is left out.
fn read_folder(dir: &Path, depth: usize) -> Vec<Row> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut rows: Vec<Row> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != ".git")
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Through symlinks, so a linked folder opens like a folder.
            let dir = e.path().is_dir();
            Row { hidden: name.starts_with('.'), name: name.into(), path: e.path(), depth, dir }
        })
        .collect();
    rows.sort_by_cached_key(|r| (!r.dir, r.name.to_lowercase()));
    rows
}

/// The project a folder belongs to: the nearest folder above it (or itself)
/// with a `.git`, else the folder itself.
pub fn project_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| d.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

impl MenuHost for ProjectView {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.menu
    }

    fn menu_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }
}

impl Render for ProjectView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = current_ui(cx);
        let tree = uniform_list("project-tree", self.rows.len(), cx.processor(|this, range, _, cx| this.render_rows(range, cx)))
            .track_scroll(&self.scroll)
            .size_full()
            .py_1();
        let empty = div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_size(px(13.0))
            .text_color(hsla(ui.text_dim))
            .child(svg().path(icon("file")).size(px(28.0)).text_color(hsla(ui.divider)))
            .child("No file open");
        let menu = self.menu.as_ref().map(|m| menu::render(m, &ui, cx));
        div()
            .key_context("Project")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)))
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.0))
            .child(self.render_header(&ui, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(TREE_WIDTH))
                            .flex_none()
                            .h_full()
                            .bg(hsla(ui.background))
                            .border_r_1()
                            .border_color(hsla(ui.divider))
                            .child(tree),
                    )
                    .child(empty),
            )
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_first_and_git_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["src", ".git", "Docs"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        for f in ["b.rs", "A.md", ".env"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        let names: Vec<String> = read_folder(dir.path(), 0).iter().map(|r| r.name.to_string()).collect();
        assert_eq!(names, ["Docs", "src", ".env", "A.md", "b.rs"]);
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        assert_eq!(project_root(&dir.path().join("src/deep")), dir.path());
    }
}

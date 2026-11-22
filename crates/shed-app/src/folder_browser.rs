//! Choosing a folder on another workspace, for a project or AI tab: this
//! PC's folder picker shows this PC's folders, so a small window of Shed's
//! own lists that workspace's (`Browse` through the nexus). Its folders,
//! the way up, a field to type a path; Open chooses the folder shown.

use std::path::PathBuf;

use gpui::{
    AppContext, Context, Entity, FocusHandle, Focusable, KeyDownEvent, SharedString, Subscription, Task, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size, svg,
};
use shed_api::calls::{self as api, Call, Reply};
use shed_client::nexus_api::RemoteWorkspace;

use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::theme::{current_ui, hsla, hsla_alpha};
use crate::workspace_api::says;

/// What choosing gives: the folder, or `None` when cancelled (or the window closed).
pub type Chosen = async_channel::Receiver<Option<PathBuf>>;

/// Opens the browser for `workspace` (named `name`), starting at the home
/// folder there.
pub fn open(workspace: RemoteWorkspace, name: String, cx: &mut gpui::App) -> Result<Chosen, String> {
    let (tx, rx) = async_channel::bounded(1);
    let bounds = WindowBounds::Windowed(gpui::Bounds::centered(None, size(px(560.0), px(520.0)), cx));
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some(format!("Choose a Folder · {name}").into()), ..Default::default() }),
        window_bounds: Some(bounds),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| cx.new(|cx| FolderBrowser::new(workspace, name, tx, window, cx))).map_err(|e| e.to_string())?;
    Ok(rx)
}

pub struct FolderBrowser {
    workspace: RemoteWorkspace,
    name: String,
    /// Where it looks, absolute (as the workspace says).
    path: String,
    parent: Option<String>,
    folders: Vec<String>,
    error: Option<String>,
    loading: Option<Task<()>>,
    field: Entity<LineEditor>,
    _field_sub: Subscription,
    focus: FocusHandle,
    chosen: Option<async_channel::Sender<Option<PathBuf>>>,
    window: gpui::AnyWindowHandle,
}

impl Focusable for FolderBrowser {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FolderBrowser {
    fn new(workspace: RemoteWorkspace, name: String, chosen: async_channel::Sender<Option<PathBuf>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (field, sub) = Self::field(String::new(), cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = Self {
            workspace,
            name,
            path: String::new(),
            parent: None,
            folders: Vec::new(),
            error: None,
            loading: None,
            field,
            _field_sub: sub,
            focus,
            chosen: Some(chosen),
            window: window.window_handle(),
        };
        this.browse(String::new(), cx);
        this
    }

    /// The field with a path to type; Enter goes there.
    fn field(text: String, cx: &mut Context<Self>) -> (Entity<LineEditor>, Subscription) {
        let field = cx.new(|cx| LineEditor::new(text, cx).without_hint());
        let sub = cx.subscribe(&field, |this, _, e: &EditorEvent, cx| match e {
            EditorEvent::Submit(path) => this.browse(path.clone(), cx),
            EditorEvent::Cancel => this.finish(None, cx),
        });
        (field, sub)
    }

    /// Lists the folders in `path` there (`""`: the home folder).
    fn browse(&mut self, path: String, cx: &mut Context<Self>) {
        let answer = self.workspace.call(Call::Browse(api::Browse { path }));
        self.loading = Some(cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                this.loading = None;
                match answer {
                    Ok(Reply::Browsed(b)) => this.shown(b, cx),
                    Ok(other) => this.error = Some(says(&shed_client::nexus_api::unexpected(&other))),
                    Err(e) => this.error = Some(says(&e)),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn shown(&mut self, b: api::Browsed, cx: &mut Context<Self>) {
        self.error = None;
        self.folders = b.entries.into_iter().filter(|e| e.dir).map(|e| e.name).collect();
        self.parent = b.parent;
        self.path = b.path;
        let (field, sub) = Self::field(self.path.clone(), cx);
        self.field = field;
        self._field_sub = sub;
    }

    fn into(&mut self, name: &str, cx: &mut Context<Self>) {
        let path = PathBuf::from(&self.path).join(name).to_string_lossy().into_owned();
        self.browse(path, cx);
    }

    /// Gives the answer and closes the window.
    fn finish(&mut self, folder: Option<PathBuf>, cx: &mut Context<Self>) {
        if let Some(tx) = self.chosen.take() {
            let _ = tx.try_send(folder);
        }
        let handle = self.window;
        cx.defer(move |cx| {
            handle.update(cx, |_, window, _| window.remove_window()).ok();
        });
    }

    fn on_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => self.finish(None, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for FolderBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = current_ui(cx);
        let accent = ui.accent;
        let button = |id: &'static str, glyph: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(hsla(if primary { accent } else { ui.divider }))
                .cursor_pointer()
                .text_color(hsla(if primary { ui.ink(accent) } else { ui.text }))
                .hover(move |s| s.bg(hsla_alpha(if primary { accent } else { ui.text_dim }, 0.15)))
                .child(svg().path(icon(glyph)).size(px(13.0)).text_color(hsla(if primary { ui.ink(accent) } else { ui.text_dim })))
                .child(label)
        };
        let up = self.parent.clone();
        let rows = self.folders.iter().enumerate().map(|(i, name)| {
            let n = name.clone();
            div()
                .id(("folder", i))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .on_click(cx.listener(move |this, _, _, cx| this.into(&n, cx)))
                .child(svg().path(icon("folder")).size(px(14.0)).flex_none().text_color(hsla(ui.ink(accent))))
                .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(SharedString::from(name.clone())))
        });
        let status = match (&self.error, self.loading.is_some(), self.folders.is_empty()) {
            (Some(e), _, _) => Some((e.clone(), ui.danger)),
            (None, true, _) => Some(("Looking…".to_string(), ui.text_dim)),
            (None, false, true) => Some(("No folders in it.".to_string(), ui.text_dim)),
            _ => None,
        };
        let path = self.path.clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(hsla(ui.background))
            .text_size(crate::theme::us(13.0))
            .text_color(hsla(ui.text))
            .child(div().text_color(hsla(ui.text_dim)).child(format!("A folder on {}", self.name)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button("folder-up", "arrow-up", "Up", false)
                            .when(up.is_none(), |d| d.text_color(hsla(ui.text_dim)))
                            .when_some(up, |d, up| d.on_click(cx.listener(move |this, _, _, cx| this.browse(up.clone(), cx)))),
                    )
                    .child(div().flex_1().min_w_0().px_1().rounded_sm().border_1().border_color(hsla(ui.divider)).bg(hsla(ui.tab_active)).child(self.field.clone())),
            )
            .child(
                div()
                    .id("folders")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla(ui.divider))
                    .p_1()
                    .flex()
                    .flex_col()
                    .when_some(status, |d, (text, color)| d.child(div().p_2().text_color(hsla(color)).child(text)))
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1())
                    .child(button("folder-cancel", "close", "Cancel", false).on_click(cx.listener(|this, _, _, cx| this.finish(None, cx))))
                    .child(
                        button("folder-open", "folder", "Open This Folder", true)
                            .when(path.is_empty(), |d| d.text_color(hsla(ui.text_dim)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !path.is_empty() {
                                    this.finish(Some(PathBuf::from(&path)), cx);
                                }
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[gpui::test]
    fn another_workspaces_folders_are_browsed_and_chosen(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(crate::theme::UiColors::default()));
        });
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = asked.clone();
        let api = shed_client::nexus_api::NexusApi::fake(Arc::new(move |_, call| match call {
            Call::Browse(b) => {
                seen.lock().unwrap().push(b.path.clone());
                let path = if b.path.is_empty() { "/home/you".to_string() } else { b.path };
                let entries = vec![api::FolderEntry { name: "code".into(), dir: true, ignored: false }];
                Ok(Reply::Browsed(api::Browsed { parent: Some("/home".into()), path, entries }))
            }
            _ => Err(shed_api::ApiError::new(shed_api::ErrorCode::Unsupported, "no")),
        }));
        let (tx, rx) = async_channel::bounded(1);
        let (view, cx) = cx.add_window_view(|window, cx| FolderBrowser::new(api.workspace("w1"), "tower".into(), tx, window, cx));
        cx.run_until_parked();
        view.update(cx, |v, cx| {
            assert_eq!((v.path.as_str(), v.folders.clone()), ("/home/you", vec!["code".to_string()]));
            v.into("code", cx);
        });
        cx.run_until_parked();
        view.update(cx, |v, cx| {
            assert_eq!(v.path, "/home/you/code");
            v.finish(Some(PathBuf::from(&v.path)), cx);
        });
        assert_eq!(rx.try_recv(), Ok(Some(PathBuf::from("/home/you/code"))));
        assert_eq!(*asked.lock().unwrap(), ["", "/home/you/code"]);
    }
}

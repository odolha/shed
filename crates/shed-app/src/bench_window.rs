//! A bench's window: the bench-colour stripe and the active tab's pane.

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use shed_emu::Palette;
use shed_model::{BenchColor, PaneId};

use crate::keeper_conn::KeeperConn;
use crate::terminal_view::TerminalView;
use crate::theme::{FontSettings, hsla};

pub struct BenchWindow {
    pub color: BenchColor,
    pub terminal: Entity<TerminalView>,
}

impl BenchWindow {
    pub fn new(
        color: BenchColor,
        pane: PaneId,
        conn: KeeperConn,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminal = cx.new(|cx| TerminalView::new(pane, conn, palette, FontSettings::default(), cx));
        window.focus(&terminal.read(cx).focus_handle().clone(), cx);
        Self { color, terminal }
    }
}

impl Render for BenchWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().h(px(3.0)).w_full().flex_none().bg(hsla(self.color.accent())))
            .child(div().flex_1().min_h_0().child(self.terminal.clone()))
    }
}

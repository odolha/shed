//! What a Mac app is expected to have: a menu bar (with Quit, so Cmd+Q
//! closes Shed the way closing its window does) and a Dock icon that brings
//! Shed back when clicked.

use gpui::{App, Entity, Global, Menu, MenuItem};

use crate::actions::{Copy, Paste, Quit, ShowShed};
use crate::state::AppState;

struct Shed(Entity<AppState>);

impl Global for Shed {}

pub fn install(state: Entity<AppState>, cx: &mut App) {
    cx.set_menus([
        Menu {
            name: "Shed".into(),
            items: vec![MenuItem::action("Show Shed", ShowShed), MenuItem::separator(), MenuItem::action("Quit Shed", Quit)],
            disabled: false,
        },
        Menu { name: "Edit".into(), items: vec![MenuItem::action("Copy", Copy), MenuItem::action("Paste", Paste)], disabled: false },
    ]);
    let showing = state.clone();
    cx.on_action(move |_: &ShowShed, cx| showing.update(cx, |state, cx| state.show_shed(cx)));
    cx.set_global(Shed(state));
}

/// The Dock icon clicked while Shed runs.
pub fn reopen(cx: &mut App) {
    if let Some(state) = cx.try_global::<Shed>().map(|s| s.0.clone()) {
        state.update(cx, |state, cx| state.show_shed(cx));
    }
}

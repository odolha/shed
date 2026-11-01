//! Commands and their default keys. Bindings become configurable in `shed.toml`.

use gpui::{Action, App, KeyBinding, actions};

actions!(
    shed,
    [
        Copy,
        Paste,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ScrollPageUp,
        ScrollPageDown,
        NewTab,
        CloseTab,
        ClosePane,
        NextTab,
        PrevTab,
        SplitRight,
        SplitDown,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        NewBench,
        CloseBench,
        RenameTab,
        RenameBench,
    ]
);

/// Activates the tab at this position (0-based).
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = shed, no_json)]
pub struct ActivateTab(pub usize);

/// Raises the window of the bench at this position (0-based).
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = shed, no_json)]
pub struct FocusBench(pub usize);

pub fn bind_default_keys(cx: &mut App) {
    let t = Some("Terminal");
    let w = Some("BenchWindow");
    let mut keys = vec![
        KeyBinding::new("ctrl-shift-c", Copy, t),
        KeyBinding::new("ctrl-shift-v", Paste, t),
        KeyBinding::new("shift-insert", Paste, t),
        KeyBinding::new("ctrl-=", ZoomIn, t),
        KeyBinding::new("ctrl-+", ZoomIn, t),
        KeyBinding::new("ctrl--", ZoomOut, t),
        KeyBinding::new("ctrl-0", ZoomReset, t),
        KeyBinding::new("shift-pageup", ScrollPageUp, t),
        KeyBinding::new("shift-pagedown", ScrollPageDown, t),
        KeyBinding::new("ctrl-shift-t", NewTab, w),
        KeyBinding::new("ctrl-shift-w", ClosePane, w),
        KeyBinding::new("ctrl-tab", NextTab, w),
        KeyBinding::new("ctrl-shift-tab", PrevTab, w),
        KeyBinding::new("ctrl-pagedown", NextTab, w),
        KeyBinding::new("ctrl-pageup", PrevTab, w),
        KeyBinding::new("ctrl-shift-e", SplitRight, w),
        KeyBinding::new("ctrl-shift-o", SplitDown, w),
        KeyBinding::new("alt-left", FocusLeft, w),
        KeyBinding::new("alt-right", FocusRight, w),
        KeyBinding::new("alt-up", FocusUp, w),
        KeyBinding::new("alt-down", FocusDown, w),
        KeyBinding::new("ctrl-shift-n", NewBench, w),
        KeyBinding::new("ctrl-shift-q", CloseBench, w),
        KeyBinding::new("f2", RenameTab, w),
        KeyBinding::new("shift-f2", RenameBench, w),
    ];
    for i in 0..9 {
        keys.push(KeyBinding::new(&format!("ctrl-{}", i + 1), ActivateTab(i), w));
        keys.push(KeyBinding::new(&format!("super-{}", i + 1), FocusBench(i), w));
    }
    cx.bind_keys(keys);
}

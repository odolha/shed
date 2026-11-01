//! Commands and their default keys. Bindings become configurable in `shed.toml`.

use std::collections::BTreeMap;

use gpui::{Action, App, KeyBinding, Keystroke, actions};

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
        OpenLogbook,
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

/// The default keymap: key → action name. `shed.toml`'s `[keys]` merges over it.
pub const DEFAULT_KEYS: &[(&str, &str)] = &[
    ("ctrl-shift-c", "copy"),
    ("ctrl-shift-v", "paste"),
    ("shift-insert", "paste"),
    ("ctrl-=", "zoom_in"),
    ("ctrl-+", "zoom_in"),
    ("ctrl--", "zoom_out"),
    ("ctrl-0", "zoom_reset"),
    ("shift-pageup", "scroll_page_up"),
    ("shift-pagedown", "scroll_page_down"),
    ("ctrl-r", "logbook"),
    ("ctrl-shift-t", "new_tab"),
    ("ctrl-shift-w", "close_pane"),
    ("ctrl-tab", "next_tab"),
    ("ctrl-shift-tab", "prev_tab"),
    ("ctrl-pagedown", "next_tab"),
    ("ctrl-pageup", "prev_tab"),
    ("ctrl-shift-e", "split_right"),
    ("ctrl-shift-o", "split_down"),
    ("alt-left", "focus_left"),
    ("alt-right", "focus_right"),
    ("alt-up", "focus_up"),
    ("alt-down", "focus_down"),
    ("ctrl-shift-n", "new_bench"),
    ("ctrl-shift-q", "close_bench"),
    ("f2", "rename_tab"),
    ("shift-f2", "rename_bench"),
    ("ctrl-1", "activate_tab_1"),
    ("ctrl-2", "activate_tab_2"),
    ("ctrl-3", "activate_tab_3"),
    ("ctrl-4", "activate_tab_4"),
    ("ctrl-5", "activate_tab_5"),
    ("ctrl-6", "activate_tab_6"),
    ("ctrl-7", "activate_tab_7"),
    ("ctrl-8", "activate_tab_8"),
    ("ctrl-9", "activate_tab_9"),
    ("super-1", "focus_bench_1"),
    ("super-2", "focus_bench_2"),
    ("super-3", "focus_bench_3"),
    ("super-4", "focus_bench_4"),
    ("super-5", "focus_bench_5"),
    ("super-6", "focus_bench_6"),
    ("super-7", "focus_bench_7"),
    ("super-8", "focus_bench_8"),
    ("super-9", "focus_bench_9"),
];

/// The binding of `key` to the action named `action`; `Ok(None)` for `"none"`.
pub fn binding(key: &str, action: &str) -> Result<Option<KeyBinding>, String> {
    for part in key.split_whitespace() {
        Keystroke::parse(part).map_err(|e| format!("key {key:?}: {e}"))?;
    }
    let (t, w) = (Some("Terminal"), Some("BenchWindow"));
    let numbered = |prefix: &str| {
        action.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=9).contains(n)).map(|n| n - 1)
    };
    Ok(Some(match action {
        "none" => return Ok(None),
        "copy" => KeyBinding::new(key, Copy, t),
        "paste" => KeyBinding::new(key, Paste, t),
        "zoom_in" => KeyBinding::new(key, ZoomIn, t),
        "zoom_out" => KeyBinding::new(key, ZoomOut, t),
        "zoom_reset" => KeyBinding::new(key, ZoomReset, t),
        "scroll_page_up" => KeyBinding::new(key, ScrollPageUp, t),
        "scroll_page_down" => KeyBinding::new(key, ScrollPageDown, t),
        // Only at a shell prompt: in vim and other programs the key is theirs.
        "logbook" => KeyBinding::new(key, OpenLogbook, Some("Terminal && at_prompt")),
        "new_tab" => KeyBinding::new(key, NewTab, w),
        "close_tab" => KeyBinding::new(key, CloseTab, w),
        "close_pane" => KeyBinding::new(key, ClosePane, w),
        "next_tab" => KeyBinding::new(key, NextTab, w),
        "prev_tab" => KeyBinding::new(key, PrevTab, w),
        "split_right" => KeyBinding::new(key, SplitRight, w),
        "split_down" => KeyBinding::new(key, SplitDown, w),
        "focus_left" => KeyBinding::new(key, FocusLeft, w),
        "focus_right" => KeyBinding::new(key, FocusRight, w),
        "focus_up" => KeyBinding::new(key, FocusUp, w),
        "focus_down" => KeyBinding::new(key, FocusDown, w),
        "new_bench" => KeyBinding::new(key, NewBench, w),
        "close_bench" => KeyBinding::new(key, CloseBench, w),
        "rename_tab" => KeyBinding::new(key, RenameTab, w),
        "rename_bench" => KeyBinding::new(key, RenameBench, w),
        _ => {
            if let Some(i) = numbered("activate_tab_") {
                KeyBinding::new(key, ActivateTab(i), w)
            } else if let Some(i) = numbered("focus_bench_") {
                KeyBinding::new(key, FocusBench(i), w)
            } else {
                return Err(format!("key {key:?}: unknown action {action:?}"));
            }
        }
    }))
}

/// Replaces every binding with the defaults merged with `overrides`.
/// Returns problems with the overrides; the rest still apply.
pub fn bind_keys(cx: &mut App, overrides: &BTreeMap<String, String>) -> Vec<String> {
    let mut keymap: BTreeMap<String, String> =
        DEFAULT_KEYS.iter().map(|(k, a)| (k.to_string(), a.to_string())).collect();
    keymap.extend(overrides.iter().map(|(k, a)| (k.clone(), a.clone())));
    let mut errors = Vec::new();
    let mut bindings = Vec::new();
    for (key, action) in &keymap {
        match binding(key, action) {
            Ok(Some(b)) => bindings.push(b),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_binding_is_valid() {
        for (key, action) in DEFAULT_KEYS {
            assert!(matches!(binding(key, action), Ok(Some(_))), "{key} = {action}");
        }
    }

    #[test]
    fn bad_bindings_are_reported() {
        assert!(binding("ctrl-x", "no_such_action").is_err());
        assert!(binding("hyper-x", "copy").is_err());
        assert!(binding("ctrl-x", "activate_tab_10").is_err());
        assert!(matches!(binding("ctrl-x", "none"), Ok(None)));
    }
}

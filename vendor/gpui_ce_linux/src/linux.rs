#[cfg(any(feature = "wayland", feature = "x11"))]
mod character_palette;
mod dispatcher;
mod display_connection;
mod headless;
mod keyboard;
mod platform;
mod system_notifications;
#[cfg(feature = "wayland")]
mod wayland;
#[cfg(feature = "x11")]
mod x11;

#[cfg(any(feature = "wayland", feature = "x11"))]
mod xdg_desktop_portal;

pub use dispatcher::*;
pub(crate) use display_connection::{Backend, DisplayConnection, select_backend};
pub(crate) use headless::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
#[cfg(feature = "wayland")]
pub(crate) use wayland::*;
#[cfg(feature = "x11")]
pub(crate) use x11::*;

use std::rc::Rc;

use gpui::WindowingModes;

/// Shed patch: what a copy carrying formatted text offers as `text/html`. Shed
/// puts it in the string's metadata, after this prefix.
pub const SHED_HTML_PREFIX: &str = "shed-html:";

#[allow(dead_code)]
pub(crate) fn shed_html(item: &gpui::ClipboardItem) -> Option<String> {
    item.entries().iter().find_map(|e| match e {
        gpui::ClipboardEntry::String(s) => s.metadata.as_deref()?.strip_prefix(SHED_HTML_PREFIX).map(str::to_string),
        _ => None,
    })
}

/// Shed patch: the local files of a `text/uri-list` (what a file manager
/// copies), as paths; `file://` URIs only, percent-escapes undone.
#[allow(dead_code)]
pub(crate) fn file_paths_from_uri_list(list: &str) -> Vec<std::path::PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        // `file://host/path`: only this machine's.
        .filter_map(|rest| rest.strip_prefix("localhost").or(Some(rest)).filter(|p| p.starts_with('/')))
        .filter_map(|p| {
            let bytes = p.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' && i + 2 < bytes.len() {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                    out.push(u8::from_str_radix(hex, 16).ok()?);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            use std::os::unix::ffi::OsStringExt;
            Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(out)))
        })
        .collect()
}

/// Shed patch: a clipboard item with the files copied, beside what it held
/// (after it: text readers still find its text first).
#[allow(dead_code)]
pub(crate) fn with_files(item: Option<gpui::ClipboardItem>, files: Vec<std::path::PathBuf>) -> Option<gpui::ClipboardItem> {
    if files.is_empty() {
        return item;
    }
    Some(plus_files(item.unwrap_or(gpui::ClipboardItem { entries: Vec::new() }), files))
}

#[allow(dead_code)]
pub(crate) fn plus_files(mut item: gpui::ClipboardItem, files: Vec<std::path::PathBuf>) -> gpui::ClipboardItem {
    if !files.is_empty() {
        item.entries.push(gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(files.into())));
    }
    item
}

/// Returns the default platform implementation for the current OS.
///
/// A windowed platform connects to the display server the process environment names, or starts
/// headless when it names none, and can switch between those modes later. A headless platform
/// stays headless.
pub fn current_platform(headless: bool) -> Rc<dyn gpui::Platform> {
    if headless || std::env::var_os("ZED_HEADLESS").is_some() {
        linux_platform(WindowingModes::HEADLESS)
    } else {
        linux_platform(WindowingModes::all())
    }
}

/// Returns a platform that may switch among `allowed_modes`.
///
/// It starts windowed in the process's own environment, or headless if that names no allowed
/// display server. Set another initial mode with [`gpui::Application::with_windowing`].
pub fn linux_platform(allowed_modes: WindowingModes) -> Rc<dyn gpui::Platform> {
    Rc::new(LinuxPlatform::new(allowed_modes))
}

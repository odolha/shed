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

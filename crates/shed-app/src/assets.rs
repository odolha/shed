//! Icons compiled into the binary.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((concat!("icons/ui/", $name, ".svg"), include_bytes!(concat!("../../../assets/icons/ui/", $name, ".svg")))),*]
    };
}

const FILES: &[(&str, &[u8])] = icons![
    "bench", "bench-new", "close", "detach", "keeper", "logbook", "pane", "pegboard", "private", "rack",
    "shed", "split-down", "split-right", "tab", "tab-new", "tag", "tool", "tool-waiting", "bench-color", "bench-load", "theme-auto", "theme-light",
    "theme-dark", "more", "power", "trash", "pencil", "go",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(p, _)| *p == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(FILES.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

/// The path of a UI icon, as `svg().path(...)` takes it.
pub fn icon(name: &str) -> SharedString {
    format!("icons/ui/{name}.svg").into()
}

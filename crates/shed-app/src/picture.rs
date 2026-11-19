//! Pictures in a project: one shown at a zoom (fitted, or at a scale of its
//! pixels), and two versions of one side by side, for diffs. Reading them
//! (kind, size) is `shed_project::picture`'s.
//!
//! A picture is drawn from its bytes (`gpui::Image`, keyed by their hash),
//! not from its path: GPUI caches by path, and a picture changed on disk
//! would keep showing as it was.

use std::path::Path;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{AnyElement, ElementId, Image, ImageFormat, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, img, px};

use crate::theme::{UiColors, hsla};

pub use shed_project::picture::is_picture;
use shed_project::picture::{PictureData, PictureKind};

/// A picture ready to draw: its data's bytes moved into a `gpui::Image`.
#[derive(Clone)]
pub struct Picture {
    pub image: Arc<Image>,
    /// Width and height in pixels, when known (not for every SVG).
    pub size: Option<(u32, u32)>,
    pub bytes: usize,
}

impl From<PictureData> for Picture {
    fn from(data: PictureData) -> Picture {
        let format = match data.kind {
            PictureKind::Png => ImageFormat::Png,
            PictureKind::Jpeg => ImageFormat::Jpeg,
            PictureKind::Gif => ImageFormat::Gif,
            PictureKind::Webp => ImageFormat::Webp,
            PictureKind::Bmp => ImageFormat::Bmp,
            PictureKind::Ico => ImageFormat::Ico,
            PictureKind::Tiff => ImageFormat::Tiff,
            PictureKind::Svg => ImageFormat::Svg,
        };
        Picture { size: data.size, bytes: data.bytes.len(), image: Arc::new(Image::from_bytes(format, data.bytes)) }
    }
}

/// How a picture is shown: fitted into the space (never enlarged), or at a
/// scale of its pixels (1.0: one pixel per point).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Zoom {
    Fit,
    Scale(f32),
}

impl Zoom {
    /// The next step in or out from here (`fitted` is the scale "Fit" shows at).
    pub fn step(self, inward: bool, fitted: f32) -> Zoom {
        const STEPS: [f32; 12] = [0.1, 0.25, 0.33, 0.5, 0.67, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];
        let now = match self {
            Zoom::Fit => fitted,
            Zoom::Scale(s) => s,
        };
        let next = if inward {
            STEPS.iter().copied().find(|s| *s > now + 0.001).unwrap_or(8.0)
        } else {
            STEPS.iter().rev().copied().find(|s| *s < now - 0.001).unwrap_or(0.1)
        };
        Zoom::Scale(next)
    }
}

/// A picture from its bytes (`path` names its kind when the bytes do not);
/// `None` for no bytes.
pub fn from_bytes(path: &Path, bytes: Vec<u8>) -> Option<Picture> {
    PictureData::from_bytes(path, bytes).map(Picture::from)
}

/// A picture from disk; `None` if it cannot be read or is too large.
pub fn read(path: &Path) -> Option<Picture> {
    PictureData::read(path).map(Picture::from)
}

/// "1280 × 800 · 245 KB".
pub fn describe(p: &Picture) -> String {
    shed_project::picture::describe(p.size, p.bytes)
}

/// The picture at a zoom, in the space it is given: fitted (centred, never
/// enlarged), or at a scale, scrolling when larger than the space.
/// With `framed`, a thin border shows where the picture ends (a fitted one
/// fills its box only when the box has the picture's shape: not beside another).
pub fn view(id: impl Into<ElementId>, p: &Picture, zoom: Zoom, framed: bool, ui: &UiColors) -> AnyElement {
    let frame = |e: gpui::Img| e.when(framed, |e| e.border_1().border_color(hsla(ui.divider)));
    match (zoom, p.size) {
        (Zoom::Scale(s), Some((w, h))) => div()
            .id(id)
            .size_full()
            .overflow_scroll()
            .child(
                div()
                    .min_w_full()
                    .min_h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_3()
                    .child(frame(img(p.image.clone())).flex_none().w(px(w as f32 * s)).h(px(h as f32 * s))),
            )
            .into_any_element(),
        _ => div()
            .size_full()
            .p_3()
            .flex()
            .items_center()
            .justify_center()
            .child(frame(img(p.image.clone())).max_w_full().max_h_full())
            .into_any_element(),
    }
}

/// Two versions of a picture side by side, each fitted, with its size; a
/// side without one says it was added or deleted.
pub fn pair(id: &str, old: Option<&Picture>, new: Option<&Picture>, ui: &UiColors, text_size: f32) -> AnyElement {
    let side = |n: usize, label: &str, p: Option<&Picture>, none: &str| {
        let head = match p {
            Some(p) => format!("{label} · {}", describe(p)),
            None => format!("{label} · {none}"),
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .when(n == 1, |d| d.border_l_1().border_color(hsla(ui.divider)))
            .child(div().flex_none().px_3().py_1().text_color(hsla(ui.text_dim)).truncate().child(head))
            .child(div().flex_1().min_h_0().child(match p {
                Some(p) => view(ElementId::Name(format!("{id}-{n}").into()), p, Zoom::Fit, false, ui),
                None => div().into_any_element(),
            }))
    };
    div()
        .size_full()
        .flex()
        .text_size(px(text_size))
        .child(side(0, "Before", old, "not there (added)"))
        .child(side(1, "After", new, "not there (deleted)"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_keep_their_kind_and_zoom_steps() {
        let p = from_bytes(Path::new("a.svg"), br#"<svg viewBox="0 0 24 16"></svg>"#.to_vec()).unwrap();
        assert_eq!((p.image.format, p.size), (ImageFormat::Svg, Some((24, 16))));
        let mut png = Vec::new();
        image::RgbaImage::new(2, 1).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let p = from_bytes(Path::new("c.bin"), png).unwrap();
        assert_eq!((p.image.format, p.size), (ImageFormat::Png, Some((2, 1))));
        assert_eq!(Zoom::Fit.step(true, 0.4), Zoom::Scale(0.5));
        assert_eq!(Zoom::Scale(1.0).step(false, 0.4), Zoom::Scale(0.67));
    }
}

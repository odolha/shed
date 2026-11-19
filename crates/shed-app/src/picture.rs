//! Pictures in a project: one shown at a zoom (fitted, or at a scale of its
//! pixels), and two versions of one side by side, for diffs.
//!
//! A picture is drawn from its bytes (`gpui::Image`, keyed by their hash),
//! not from its path: GPUI caches by path, and a picture changed on disk
//! would keep showing as it was.

use std::path::Path;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{AnyElement, ElementId, Image, ImageFormat, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, img, px};

use crate::theme::{UiColors, hsla};

/// Larger pictures are not read.
pub const MAX_PICTURE: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Picture {
    pub image: Arc<Image>,
    /// Width and height in pixels, when known (not for every SVG).
    pub size: Option<(u32, u32)>,
    pub bytes: usize,
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

/// Whether a file is a picture Shed shows, by its name.
pub fn is_picture(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" | "tif" | "tiff"))
}

/// A picture from its bytes (`path` names its kind when the bytes do not);
/// `None` for no bytes.
pub fn from_bytes(path: &Path, bytes: Vec<u8>) -> Option<Picture> {
    if bytes.is_empty() {
        return None;
    }
    let guessed = image::guess_format(&bytes).ok();
    let format = match guessed {
        Some(image::ImageFormat::Png) => ImageFormat::Png,
        Some(image::ImageFormat::Jpeg) => ImageFormat::Jpeg,
        Some(image::ImageFormat::Gif) => ImageFormat::Gif,
        Some(image::ImageFormat::WebP) => ImageFormat::Webp,
        Some(image::ImageFormat::Bmp) => ImageFormat::Bmp,
        Some(image::ImageFormat::Ico) => ImageFormat::Ico,
        Some(image::ImageFormat::Tiff) => ImageFormat::Tiff,
        _ => match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("png") => ImageFormat::Png,
            Some("jpg" | "jpeg") => ImageFormat::Jpeg,
            Some("gif") => ImageFormat::Gif,
            Some("webp") => ImageFormat::Webp,
            Some("bmp") => ImageFormat::Bmp,
            Some("ico") => ImageFormat::Ico,
            Some("tif" | "tiff") => ImageFormat::Tiff,
            _ => ImageFormat::Svg,
        },
    };
    let size = if format == ImageFormat::Svg {
        svg_size(&bytes)
    } else {
        image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok())
    };
    Some(Picture { size, bytes: bytes.len(), image: Arc::new(Image::from_bytes(format, bytes)) })
}

/// A picture from disk; `None` if it cannot be read or is too large.
pub fn read(path: &Path) -> Option<Picture> {
    if std::fs::metadata(path).ok()?.len() > MAX_PICTURE {
        return None;
    }
    from_bytes(path, std::fs::read(path).ok()?)
}

/// An SVG's size from its `width` and `height`, else its `viewBox`.
fn svg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(&bytes[..bytes.len().min(4096)]).ok()?;
    let tag = &text[text.find("<svg")?..];
    let tag = &tag[..tag.find('>')?];
    let attr = |name: &str| -> Option<&str> {
        let at = tag.find(&format!(" {name}=")).or_else(|| tag.find(&format!("\n{name}=")))? + name.len() + 2;
        let quote = tag[at..].chars().next()?;
        let rest = &tag[at + 1..];
        Some(&rest[..rest.find(quote)?])
    };
    let number = |v: &str| v.trim_end_matches("px").trim().parse::<f32>().ok().filter(|n| *n > 0.0);
    if let (Some(w), Some(h)) = (attr("width").and_then(number), attr("height").and_then(number)) {
        return Some((w.round() as u32, h.round() as u32));
    }
    let view: Vec<f32> = attr("viewBox")?.split([' ', ',']).filter_map(|n| n.parse().ok()).collect();
    (view.len() == 4 && view[2] > 0.0 && view[3] > 0.0).then(|| (view[2].round() as u32, view[3].round() as u32))
}

/// "1280 × 800 · 245 KB".
pub fn describe(p: &Picture) -> String {
    let bytes = match p.bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b / 1024),
        b => format!("{b} bytes"),
    };
    match p.size {
        Some((w, h)) => format!("{w} × {h} · {bytes}"),
        None => bytes,
    }
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
    fn sizes_and_kinds() {
        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 16"><path/></svg>"#;
        let p = from_bytes(Path::new("a.svg"), svg.to_vec()).unwrap();
        assert_eq!((p.image.format, p.size), (ImageFormat::Svg, Some((24, 16))));
        let p = from_bytes(Path::new("b.svg"), br#"<svg width="40px" height="30" viewBox="0 0 4 3"></svg>"#.to_vec()).unwrap();
        assert_eq!(p.size, Some((40, 30)));
        // A 2×1 PNG.
        let mut png = Vec::new();
        image::RgbaImage::new(2, 1).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let p = from_bytes(Path::new("c.bin"), png).unwrap();
        assert_eq!((p.image.format, p.size), (ImageFormat::Png, Some((2, 1))));
        assert!(from_bytes(Path::new("d.png"), Vec::new()).is_none());
        assert_eq!(Zoom::Fit.step(true, 0.4), Zoom::Scale(0.5));
        assert_eq!(Zoom::Scale(1.0).step(false, 0.4), Zoom::Scale(0.67));
    }
}

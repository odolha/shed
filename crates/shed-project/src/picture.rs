//! Pictures in a project as data: which files are pictures, a picture's
//! bytes, kind and size, and how it is described ("1280 × 800 · 245 KB").
//! Drawing them is the app's.

use std::path::Path;

/// Larger pictures are not read.
pub const MAX_PICTURE: u64 = 64 * 1024 * 1024;

/// The kinds of picture Shed shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Ico,
    Tiff,
    Svg,
}

impl PictureKind {
    /// Its media type, as the web names it.
    pub fn media_type(self) -> &'static str {
        match self {
            PictureKind::Png => "image/png",
            PictureKind::Jpeg => "image/jpeg",
            PictureKind::Gif => "image/gif",
            PictureKind::Webp => "image/webp",
            PictureKind::Bmp => "image/bmp",
            PictureKind::Ico => "image/x-icon",
            PictureKind::Tiff => "image/tiff",
            PictureKind::Svg => "image/svg+xml",
        }
    }
}

/// A picture's bytes, its kind, and its size in pixels when known (not for
/// every SVG).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureData {
    pub bytes: Vec<u8>,
    pub kind: PictureKind,
    pub size: Option<(u32, u32)>,
}

impl PictureData {
    /// A picture from its bytes (`path` names its kind when the bytes do
    /// not); `None` for no bytes.
    pub fn from_bytes(path: &Path, bytes: Vec<u8>) -> Option<PictureData> {
        if bytes.is_empty() {
            return None;
        }
        let guessed = image::guess_format(&bytes).ok();
        let kind = match guessed {
            Some(image::ImageFormat::Png) => PictureKind::Png,
            Some(image::ImageFormat::Jpeg) => PictureKind::Jpeg,
            Some(image::ImageFormat::Gif) => PictureKind::Gif,
            Some(image::ImageFormat::WebP) => PictureKind::Webp,
            Some(image::ImageFormat::Bmp) => PictureKind::Bmp,
            Some(image::ImageFormat::Ico) => PictureKind::Ico,
            Some(image::ImageFormat::Tiff) => PictureKind::Tiff,
            _ => match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
                Some("png") => PictureKind::Png,
                Some("jpg" | "jpeg") => PictureKind::Jpeg,
                Some("gif") => PictureKind::Gif,
                Some("webp") => PictureKind::Webp,
                Some("bmp") => PictureKind::Bmp,
                Some("ico") => PictureKind::Ico,
                Some("tif" | "tiff") => PictureKind::Tiff,
                _ => PictureKind::Svg,
            },
        };
        let size = if kind == PictureKind::Svg {
            svg_size(&bytes)
        } else {
            image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok())
        };
        Some(PictureData { bytes, kind, size })
    }

    /// A picture from disk; `None` if it cannot be read or is too large.
    pub fn read(path: &Path) -> Option<PictureData> {
        if std::fs::metadata(path).ok()?.len() > MAX_PICTURE {
            return None;
        }
        PictureData::from_bytes(path, std::fs::read(path).ok()?)
    }

    /// "1280 × 800 · 245 KB".
    pub fn describe(&self) -> String {
        describe(self.size, self.bytes.len())
    }
}

/// Whether a file is a picture Shed shows, by its name.
pub fn is_picture(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" | "tif" | "tiff"))
}

/// A picture of `size` taking `bytes`: "1280 × 800 · 245 KB".
pub fn describe(size: Option<(u32, u32)>, bytes: usize) -> String {
    let bytes = match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b / 1024),
        b => format!("{b} bytes"),
    };
    match size {
        Some((w, h)) => format!("{w} × {h} · {bytes}"),
        None => bytes,
    }
}

/// An SVG's size from its `width` and `height`, else its `viewBox`.
pub fn svg_size(bytes: &[u8]) -> Option<(u32, u32)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_kinds() {
        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 16"><path/></svg>"#;
        let p = PictureData::from_bytes(Path::new("a.svg"), svg.to_vec()).unwrap();
        assert_eq!((p.kind, p.size), (PictureKind::Svg, Some((24, 16))));
        let p = PictureData::from_bytes(Path::new("b.svg"), br#"<svg width="40px" height="30" viewBox="0 0 4 3"></svg>"#.to_vec()).unwrap();
        assert_eq!(p.size, Some((40, 30)));
        // A 2×1 PNG, whatever its name says.
        let mut png = Vec::new();
        image::RgbaImage::new(2, 1).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let p = PictureData::from_bytes(Path::new("c.bin"), png).unwrap();
        assert_eq!((p.kind, p.size), (PictureKind::Png, Some((2, 1))));
        assert_eq!(p.kind.media_type(), "image/png");
        assert!(PictureData::from_bytes(Path::new("d.png"), Vec::new()).is_none());
        assert!(is_picture(Path::new("x/Logo.SVG")) && !is_picture(Path::new("x.txt")));
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe(Some((1280, 800)), 245 * 1024 + 10), "1280 × 800 · 245 KB");
        assert_eq!(describe(None, 3 * 1024 * 1024 / 2), "1.5 MB");
        assert_eq!(describe(None, 12), "12 bytes");
    }
}

//! A contact sheet of the bench icon in every colour, at launcher and Dock
//! sizes, on dark and light, beside Shed's own icon:
//! `cargo run -p shed-desktop --example icon_preview -- sheet.png [template.svg]`.
//! With a template, it is drawn instead of the built-in one (`{{TOP}}`,
//! `{{BOTTOM}}` and `{{BADGE}}` filled in as for the real icons).

use resvg::tiny_skia::{Color, Pixmap, PixmapPaint, Transform};
use shed_model::BenchColor;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().expect("usage: icon_preview <sheet.png> [template.svg]");
    let template = args.next().map(|p| std::fs::read_to_string(p).expect("the template reads"));
    let svg = |color: BenchColor, badge: Option<&str>| match &template {
        Some(t) => {
            let (top, bottom) = color.gradient();
            t.replace("{{TOP}}", &format!("#{:06X}", top.to_hex()))
                .replace("{{BOTTOM}}", &format!("#{:06X}", bottom.to_hex()))
                .replace("{{BADGE}}", badge.unwrap_or(""))
        }
        None => shed_desktop::icon_svg(color, badge),
    };

    let colors = BenchColor::NAMED;
    let (big, small, pad) = (128u32, 48u32, 16u32);
    let cols = colors.len() as u32 / 2 + 1;
    let width = cols * (big + pad) + pad;
    let band = 2 * (big + pad) + small + pad;
    let mut sheet = Pixmap::new(width, 2 * band + pad).unwrap();
    for (i, background) in [Color::from_rgba8(0x22, 0x25, 0x2C, 255), Color::from_rgba8(0xEC, 0xEC, 0xF0, 255)].into_iter().enumerate() {
        let y0 = i as u32 * band;
        let mut fill = Pixmap::new(width, band).unwrap();
        fill.fill(background);
        sheet.draw_pixmap(0, y0 as i32, fill.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        let mut place = |svg: &str, size: u32, x: u32, y: u32| {
            let png = shed_desktop::icon_png(svg, size).expect("the icon renders");
            let icon = Pixmap::decode_png(&png).unwrap();
            sheet.draw_pixmap(x as i32, y as i32, icon.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        };
        place(shed_desktop::SHED_ICON, big, pad, y0 + pad);
        place(&svg(colors[0], Some("WS")), big, pad, y0 + pad + big + pad);
        for (j, color) in colors.into_iter().enumerate() {
            let (col, row) = (1 + j as u32 / 2, j as u32 % 2);
            place(&svg(color, None), big, pad + col * (big + pad), y0 + pad + row * (big + pad));
            place(&svg(color, None), small, pad + j as u32 * (small + pad / 2), y0 + pad + 2 * (big + pad));
        }
    }
    sheet.save_png(&out).expect("the sheet writes");
    println!("{out}");
}

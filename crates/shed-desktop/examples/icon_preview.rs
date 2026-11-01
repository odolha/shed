//! Writes the bench icon in every colour as RGBA files, for looking at:
//! `cargo run -p shed-desktop --example icon_preview -- <dir>` (128×128, raw RGBA).

use shed_model::BenchColor;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    for (i, color) in BenchColor::NAMED.into_iter().enumerate() {
        let badge = (i == 0).then_some("WS");
        let svg = shed_desktop::icon_svg(color, badge);
        let rgba = shed_desktop::icon_rgba(&svg, 128).expect("renders");
        std::fs::write(format!("{dir}/{i}-{}.rgba", color.name().unwrap_or("custom")), rgba).unwrap();
    }
}

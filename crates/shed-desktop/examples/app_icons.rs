//! Renders Shed's app icon into a hicolor theme tree, for packages:
//! `cargo run -p shed-desktop --example app_icons -- <share>/icons/hicolor`.
//! Writes `scalable/apps/dev.shed.svg`, `symbolic/apps/dev.shed-symbolic.svg`
//! and `NxN/apps/dev.shed.png` at the usual sizes.

use std::path::Path;

const SYMBOLIC: &str = include_str!("../../../assets/icons/app/shed-symbolic.svg");
const SIZES: [u32; 9] = [16, 22, 24, 32, 48, 64, 128, 256, 512];

fn main() {
    let root = std::env::args().nth(1).expect("usage: app_icons <hicolor dir>");
    let root = Path::new(&root);
    let write = |rel: String, bytes: &[u8]| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    };
    write("scalable/apps/dev.shed.svg".into(), shed_desktop::SHED_ICON.as_bytes());
    write("symbolic/apps/dev.shed-symbolic.svg".into(), SYMBOLIC.as_bytes());
    for size in SIZES {
        let png = shed_desktop::icon_png(shed_desktop::SHED_ICON, size).expect("the app icon renders");
        write(format!("{size}x{size}/apps/dev.shed.png"), &png);
    }
}

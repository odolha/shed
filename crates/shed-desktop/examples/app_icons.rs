//! Renders Shed's app icon for packages:
//! `cargo run -p shed-desktop --example app_icons -- <share>/icons/hicolor`
//! writes a hicolor theme tree (`scalable/apps/dev.shed.svg`,
//! `symbolic/apps/dev.shed-symbolic.svg`, `NxN/apps/dev.shed.png`);
//! `-- --iconset <dir>.iconset` writes a macOS iconset, for `iconutil`.

use std::path::Path;

const SYMBOLIC: &str = include_str!("../../../assets/icons/app/shed-symbolic.svg");
const SIZES: [u32; 9] = [16, 22, 24, 32, 48, 64, 128, 256, 512];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, dir] = args.as_slice()
        && flag == "--iconset"
    {
        iconset(Path::new(dir));
        return;
    }
    let root = args.first().expect("usage: app_icons <hicolor dir> | --iconset <dir>");
    let root = Path::new(root);
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

/// `icon_16x16.png`, `icon_16x16@2x.png` … `icon_512x512@2x.png`.
fn iconset(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for size in [16, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            let png = shed_desktop::icon_png(shed_desktop::SHED_ICON, size * scale).expect("the app icon renders");
            std::fs::write(dir.join(format!("icon_{size}x{size}{suffix}.png")), png).unwrap();
        }
    }
}

//! Per-bench window identity: generated icons and desktop entries.
//!
//! Linux has no single way to give one app's windows different taskbar
//! icons. Each bench window gets its own app ID (`dev.shed.bench.<name>`,
//! the X11 `WM_CLASS`), and a hidden desktop entry with the same
//! `StartupWMClass` and a generated icon in the bench colour, so docks treat
//! every bench as its own app and pinning one gives a launcher for it.

use std::io;
use std::path::{Path, PathBuf};

use shed_model::{BenchColor, Rgb};

const TEMPLATE: &str = include_str!("../../../assets/icons/bench/bench-template.svg");
const TEMPLATE_BADGE: &str = include_str!("../../../assets/icons/bench/bench-template-badge.svg");

/// A bench name made safe for app IDs and file names.
pub fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "bench".into() } else { s }
}

/// The app ID (X11 `WM_CLASS`, Wayland `app_id`) of a bench's window.
pub fn app_id(bench_name: &str) -> String {
    format!("dev.shed.bench.{}", slug(bench_name))
}

fn hex(c: Rgb) -> String {
    format!("#{:06X}", c.to_hex())
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The shed icon in the bench colour, with an optional one- or two-letter badge.
pub fn icon_svg(color: BenchColor, badge: Option<&str>) -> String {
    let (top, bottom) = color.gradient();
    let template = if badge.is_some() { TEMPLATE_BADGE } else { TEMPLATE };
    let badge: String = badge.unwrap_or_default().chars().take(2).collect();
    template
        .replace("{{TOP}}", &hex(top))
        .replace("{{BOTTOM}}", &hex(bottom))
        .replace("{{BADGE}}", &escape_xml(&badge.to_uppercase()))
}

/// Rasterizes an icon to straight (not premultiplied) RGBA, `size` × `size`.
pub fn icon_rgba(svg: &str, size: u32) -> Option<Vec<u8>> {
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(svg, &options).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(
        pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect(),
    )
}

/// What a bench's launcher shows and runs.
pub struct BenchIdentity<'a> {
    pub name: &'a str,
    pub color: BenchColor,
    /// Letters drawn on the icon, if badges are on.
    pub badge: Option<&'a str>,
    /// The `shed` binary the launcher runs.
    pub shed: &'a Path,
}

/// Where desktop entries and icons go: `~/.local/share` by default.
pub struct Desktop {
    pub data_home: PathBuf,
}

impl Desktop {
    pub fn new(data_home: PathBuf) -> Self {
        Self { data_home }
    }

    pub fn entry_path(&self, name: &str) -> PathBuf {
        self.data_home.join("applications").join(format!("{}.desktop", app_id(name)))
    }

    pub fn icon_path(&self, name: &str) -> PathBuf {
        self.data_home.join("icons/hicolor/scalable/apps").join(format!("{}.svg", app_id(name)))
    }

    /// Writes the bench's icon and desktop entry. Files already up to date
    /// are left alone, so docks are not prodded to reload for nothing.
    pub fn install(&self, bench: &BenchIdentity<'_>) -> io::Result<()> {
        let icon = self.icon_path(bench.name);
        write_if_changed(&icon, &icon_svg(bench.color, bench.badge))?;
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={name} — Shed\n\
             Comment=Shed bench “{name}”\n\
             Exec={exe} load {quoted}\n\
             Icon={icon}\n\
             StartupWMClass={class}\n\
             Terminal=false\n\
             Categories=System;TerminalEmulator;\n\
             NoDisplay=true\n",
            name = bench.name,
            exe = desktop_quote(&bench.shed.to_string_lossy()),
            quoted = desktop_quote(bench.name),
            icon = icon.display(),
            class = app_id(bench.name),
        );
        write_if_changed(&self.entry_path(bench.name), &entry)
    }

    /// Removes a bench's desktop entry and icon.
    pub fn remove(&self, name: &str) {
        let _ = std::fs::remove_file(self.entry_path(name));
        let _ = std::fs::remove_file(self.icon_path(name));
    }
}

/// Quotes an `Exec` argument per the desktop entry spec.
fn desktop_quote(arg: &str) -> String {
    if arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)) {
        return arg.to_string();
    }
    let escaped: String = arg
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            '%' => vec!['%', '%'],
            c => vec![c],
        })
        .collect();
    format!("\"{escaped}\"")
}

fn write_if_changed(path: &Path, contents: &str) -> io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|c| c == contents) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_ids() {
        assert_eq!(app_id("backend"), "dev.shed.bench.backend");
        assert_eq!(slug("My Web Site!"), "my-web-site");
        assert_eq!(slug("!!!"), "bench");
    }

    #[test]
    fn icons_take_the_bench_colours() {
        let svg = icon_svg(BenchColor::Coral, None);
        assert!(svg.contains("#FF9A5A") && svg.contains("#F0475E") && !svg.contains("{{"));
        let badged = icon_svg(BenchColor::Teal, Some("be"));
        assert!(badged.contains(">BE<"));
        let rgba = icon_rgba(&svg, 32).unwrap();
        assert_eq!(rgba.len(), 32 * 32 * 4);
        assert!(rgba.chunks(4).any(|p| p[3] == 255), "something is drawn");
    }

    #[test]
    fn install_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let desktop = Desktop::new(dir.path().to_path_buf());
        let bench = BenchIdentity { name: "web site", color: BenchColor::Green, badge: None, shed: Path::new("/opt/shed bin/shed") };
        desktop.install(&bench).unwrap();
        let entry = std::fs::read_to_string(desktop.entry_path("web site")).unwrap();
        assert!(entry.contains("StartupWMClass=dev.shed.bench.web-site\n"));
        assert!(entry.contains("Exec=\"/opt/shed bin/shed\" load \"web site\"\n"));
        assert!(desktop.icon_path("web site").exists());
        desktop.remove("web site");
        assert!(!desktop.entry_path("web site").exists());
    }
}

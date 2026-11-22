//! Per-bench window identity: generated icons and desktop entries.
//!
//! Linux has no single way to give one app's windows different taskbar
//! icons. Each bench window gets its own app ID (`dev.shed.bench.<launcher>`,
//! the X11 `WM_CLASS`), and a hidden desktop entry with the same
//! `StartupWMClass` and a generated icon in the bench colour, so docks treat
//! every bench as its own app and pinning one gives a launcher for it.
//!
//! Everything is keyed by the bench's launcher key (see
//! [`shed_model::Bench::launcher`]), not its name, so renaming a bench
//! rewrites its entry in place and a pinned launcher keeps working. Another
//! workspace's benches (shown through a nexus) get launchers of their own,
//! `dev.shed.bench.<workspace>.<launcher>`, which switch Shed to that
//! workspace (`shed load --workspace`).
//!
//! `taskbar` swaps the panel's pinned bench launchers as Shed switches
//! workspace.

pub mod taskbar;

use std::io;
use std::path::{Path, PathBuf};

use shed_model::{BenchColor, Rgb};

/// Shed's own icon, for the Shed window and its launcher.
pub const SHED_ICON: &str = include_str!("../../../assets/icons/app/shed.svg");
/// The app ID of the Shed window.
pub const SHED_APP_ID: &str = "dev.shed";

const TEMPLATE: &str = include_str!("../../../assets/icons/bench/bench-template.svg");
const TEMPLATE_BADGE: &str = include_str!("../../../assets/icons/bench/bench-template-badge.svg");

pub use shed_model::slug;

/// The app ID (X11 `WM_CLASS`, Wayland `app_id`) of a bench's window.
pub fn app_id(launcher: &str) -> String {
    format!("dev.shed.bench.{}", slug(launcher))
}

/// The app ID of another workspace's bench window: the workspace's slug
/// first (a dot apart, which a bench of this PC's never has).
pub fn remote_app_id(workspace: &str, launcher: &str) -> String {
    format!("dev.shed.bench.{}.{}", slug(workspace), slug(launcher))
}

/// Another workspace, as its launchers name it.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceRef<'a> {
    /// Its id at the nexus (what `shed load --workspace` is given).
    pub id: &'a str,
    pub name: &'a str,
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
    let pixmap = render(svg, size)?;
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

/// Rasterizes an icon to PNG, `size` × `size`.
pub fn icon_png(svg: &str, size: u32) -> Option<Vec<u8>> {
    render(svg, size)?.encode_png().ok()
}

fn render(svg: &str, size: u32) -> Option<resvg::tiny_skia::Pixmap> {
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(svg, &options).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(pixmap)
}

/// What a bench's launcher shows and runs.
pub struct BenchIdentity<'a> {
    /// The key its files and app ID go by.
    pub launcher: &'a str,
    pub name: &'a str,
    pub color: BenchColor,
    /// Letters drawn on the icon, if badges are on.
    pub badge: Option<&'a str>,
    /// The `shed` binary the launcher runs.
    pub shed: &'a Path,
    /// Another workspace's bench; `None` for this PC's.
    pub workspace: Option<WorkspaceRef<'a>>,
}

impl BenchIdentity<'_> {
    /// Its windows' app ID, which its launcher's files are named by.
    pub fn app_id(&self) -> String {
        match self.workspace {
            None => app_id(self.launcher),
            Some(w) => remote_app_id(w.name, self.launcher),
        }
    }
}

/// Where desktop entries and icons go: `~/.local/share` by default.
pub struct Desktop {
    pub data_home: PathBuf,
}

impl Desktop {
    pub fn new(data_home: PathBuf) -> Self {
        Self { data_home }
    }

    pub fn entry_path(&self, launcher: &str) -> PathBuf {
        self.entry_path_of(&app_id(launcher))
    }

    pub fn icon_path(&self, launcher: &str) -> PathBuf {
        self.icon_path_of(&app_id(launcher))
    }

    /// A launcher's desktop entry, by its app ID.
    pub fn entry_path_of(&self, app_id: &str) -> PathBuf {
        self.data_home.join("applications").join(format!("{app_id}.desktop"))
    }

    pub fn icon_path_of(&self, app_id: &str) -> PathBuf {
        self.data_home.join("icons/hicolor/scalable/apps").join(format!("{app_id}.svg"))
    }

    /// Writes the bench's icon and desktop entry. Files already up to date
    /// are left alone, so docks are not prodded to reload for nothing.
    /// Desktop entries are a Linux thing: elsewhere this does nothing.
    pub fn install(&self, bench: &BenchIdentity<'_>) -> io::Result<()> {
        if !cfg!(target_os = "linux") {
            return Ok(());
        }
        let class = bench.app_id();
        let icon = self.icon_path_of(&class);
        write_if_changed(&icon, &icon_svg(bench.color, bench.badge))?;
        let (name, workspace) = match bench.workspace {
            None => (bench.name.to_string(), String::new()),
            Some(w) => (format!("{} · {}", bench.name, w.name), format!("--workspace {} ", desktop_quote(w.id))),
        };
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Shed — {name}\n\
             Comment=Shed bench “{name}”\n\
             Exec={exe} load {workspace}{key}\n\
             Icon={icon}\n\
             StartupWMClass={class}\n\
             Terminal=false\n\
             Categories=System;TerminalEmulator;\n\
             NoDisplay=true\n",
            name = desktop_value(&name),
            exe = desktop_quote(&bench.shed.to_string_lossy()),
            // By launcher key: it outlives renames, which the name does not.
            key = desktop_quote(bench.launcher),
            icon = icon.display(),
        );
        write_if_changed(&self.entry_path_of(&class), &entry)
    }

    /// Writes Shed's own launcher and icon, which the Shed window goes by,
    /// unless an installed package provides them; then any copy written
    /// before, by a build run from its source tree, is removed.
    pub fn install_shed(&self, shed: &Path) -> io::Result<()> {
        if !cfg!(target_os = "linux") {
            return Ok(());
        }
        let entry_path = self.data_home.join("applications").join(format!("{SHED_APP_ID}.desktop"));
        let icon = self.data_home.join("icons/hicolor/scalable/apps").join(format!("{SHED_APP_ID}.svg"));
        if packaged_launcher() {
            let _ = std::fs::remove_file(&entry_path);
            let _ = std::fs::remove_file(&icon);
            return Ok(());
        }
        write_if_changed(&icon, SHED_ICON)?;
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Shed\n\
             Comment=Terminals, code, Claude Code and servers, per project, kept running\n\
             Exec={exe}\n\
             Icon={icon}\n\
             StartupWMClass={SHED_APP_ID}\n\
             Terminal=false\n\
             Categories=System;TerminalEmulator;\n",
            exe = desktop_quote(&shed.to_string_lossy()),
            icon = icon.display(),
        );
        write_if_changed(&entry_path, &entry)
    }

    /// Removes a bench's desktop entry and icon.
    pub fn remove(&self, launcher: &str) {
        self.remove_app(&app_id(launcher));
    }

    /// Removes a launcher's desktop entry and icon, by its app ID.
    pub fn remove_app(&self, app_id: &str) {
        let _ = std::fs::remove_file(self.entry_path_of(app_id));
        let _ = std::fs::remove_file(self.icon_path_of(app_id));
    }

    /// The app IDs of the launchers written here for another workspace's
    /// benches (`remote_app_id`).
    pub fn remote_launchers(&self, workspace: &str) -> Vec<String> {
        let prefix = format!("dev.shed.bench.{}.", slug(workspace));
        let Ok(entries) = std::fs::read_dir(self.data_home.join("applications")) else { return Vec::new() };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".desktop").map(str::to_string))
            .filter(|id| id.starts_with(&prefix))
            .collect();
        ids.sort();
        ids
    }
}

/// A desktop entry's string value: no line breaks.
fn desktop_value(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Whether an installed package provides Shed's launcher, in the system
/// data directories (`XDG_DATA_DIRS`).
fn packaged_launcher() -> bool {
    let dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|d| !d.is_empty());
    let dirs = dirs.unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.split(':').any(|d| Path::new(d).join("applications").join(format!("{SHED_APP_ID}.desktop")).exists())
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
    fn install_rename_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let desktop = Desktop::new(dir.path().to_path_buf());
        let shed = Path::new("/opt/shed bin/shed");
        let bench = BenchIdentity { launcher: "web-site", name: "web site", color: BenchColor::Green, badge: None, shed, workspace: None };
        desktop.install(&bench).unwrap();
        let entry = std::fs::read_to_string(desktop.entry_path("web-site")).unwrap();
        assert!(entry.contains("StartupWMClass=dev.shed.bench.web-site\n"));
        assert!(entry.contains("Exec=\"/opt/shed bin/shed\" load web-site\n"));
        assert!(desktop.icon_path("web-site").exists());

        // Renamed: the same entry, so a pinned launcher still finds it, now loading the new name.
        desktop.install(&BenchIdentity { name: "shop", ..bench }).unwrap();
        let entry = std::fs::read_to_string(desktop.entry_path("web-site")).unwrap();
        assert!(entry.contains("Name=Shed — shop\n") && entry.contains(" load web-site\n"), "{entry}");
        assert!(entry.contains("StartupWMClass=dev.shed.bench.web-site\n"));

        desktop.remove("web-site");
        assert!(!desktop.entry_path("web-site").exists());
    }

    #[test]
    fn another_workspaces_launchers_go_by_it() {
        let dir = tempfile::tempdir().unwrap();
        let desktop = Desktop::new(dir.path().to_path_buf());
        let shed = Path::new("/usr/bin/shed");
        let office = WorkspaceRef { id: "w-1f3a", name: "Office PC" };
        let bench = BenchIdentity { launcher: "api", name: "api", color: BenchColor::Blue, badge: None, shed, workspace: Some(office) };
        assert_eq!(bench.app_id(), "dev.shed.bench.office-pc.api");
        desktop.install(&bench).unwrap();
        // This PC's bench of the same key is another launcher.
        desktop.install(&BenchIdentity { workspace: None, ..bench }).unwrap();
        let entry = std::fs::read_to_string(desktop.entry_path_of("dev.shed.bench.office-pc.api")).unwrap();
        assert!(entry.contains("Exec=/usr/bin/shed load --workspace w-1f3a api\n"), "{entry}");
        assert!(entry.contains("StartupWMClass=dev.shed.bench.office-pc.api\n"));
        assert!(entry.contains("Name=Shed — api · Office PC\n"));
        assert!(desktop.icon_path_of("dev.shed.bench.office-pc.api").exists());
        assert!(std::fs::read_to_string(desktop.entry_path("api")).unwrap().contains(" load api\n"));

        // Listed by workspace (never this PC's), and removed one by one.
        desktop.install(&BenchIdentity { launcher: "db", ..bench }).unwrap();
        assert_eq!(desktop.remote_launchers("Office PC"), ["dev.shed.bench.office-pc.api", "dev.shed.bench.office-pc.db"]);
        assert!(desktop.remote_launchers("office").is_empty());
        desktop.remove_app("dev.shed.bench.office-pc.db");
        assert_eq!(desktop.remote_launchers("Office PC"), ["dev.shed.bench.office-pc.api"]);
        assert!(desktop.entry_path("api").exists());
    }
}

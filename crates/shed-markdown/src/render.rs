//! Diagrams and math as pictures, in pure Rust: Mermaid
//! (`mermaid-rs-renderer`), Graphviz's dot (`layout-rs`) and TeX
//! (`latex-rust`) made into SVG in the theme's colours, and SVG into pixels
//! (`resvg`, with the system's fonts for the words in diagrams).
//!
//! Slow next to drawing a frame (milliseconds to tens of them): callers run
//! it off the UI thread and keep what it makes. Nothing here panics on bad
//! input: the libraries' panics are caught and become errors.

use std::sync::{Arc, OnceLock};

use resvg::{tiny_skia, usvg};

use crate::DiagramKind;

/// What to draw.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    Diagram(DiagramKind, &'a str),
    /// TeX, on a line of its own.
    Math(&'a str),
}

/// The colours a picture is drawn in, as the theme has them (RGB).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Colors {
    pub text: [u8; 3],
    /// What the picture sits on.
    pub ground: [u8; 3],
    pub accent: [u8; 3],
}

impl Colors {
    /// Whether the ground is dark.
    pub fn dark(&self) -> bool {
        let [r, g, b] = self.ground;
        (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 < 128
    }

    /// `t` of the way from the ground to `to`.
    fn mix(&self, to: [u8; 3], t: f32) -> String {
        let c = |i: usize| (self.ground[i] as f32 + (to[i] as f32 - self.ground[i] as f32) * t).round().clamp(0.0, 255.0) as u8;
        hex([c(0), c(1), c(2)])
    }
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// A picture's pixels: `width` × `height`, RGBA (not premultiplied), and
/// the size it is drawn at, in logical pixels.
#[derive(Clone, Debug)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub size: (f32, f32),
}

/// The longest side a picture is rasterised at, in pixels.
const MAX_SIDE: f32 = 8192.0;

/// `source` as a picture at most `max_width` logical pixels wide (never
/// enlarged), rasterised at `scale` pixels per logical pixel; `font_px` is
/// the text's size around it (math is set a little larger). An error says
/// briefly why it cannot be drawn.
pub fn picture(source: Source, colors: &Colors, font_px: f32, max_width: f32, scale: f32) -> Result<Raster, String> {
    let svg = svg(source, colors, font_px)?;
    rasterize(&svg, max_width, scale)
}

/// `source` as SVG in `colors`.
pub fn svg(source: Source, colors: &Colors, font_px: f32) -> Result<String, String> {
    let made = std::panic::catch_unwind(|| match source {
        Source::Diagram(DiagramKind::Mermaid, src) => mermaid(src, colors),
        Source::Diagram(DiagramKind::Dot, src) => dot(src, colors),
        Source::Math(tex) => math(tex, colors, font_px),
    });
    made.unwrap_or_else(|_| Err("could not be drawn".into()))
}

/// An error as one short line.
fn brief(e: impl std::fmt::Display) -> String {
    let e = e.to_string();
    let line = e.lines().next().unwrap_or("").trim();
    match line.char_indices().nth(160) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

fn mermaid(src: &str, colors: &Colors) -> Result<String, String> {
    use mermaid_rs_renderer::{RenderOptions, Theme};
    let text = colors.text;
    let mut theme = if colors.dark() { Theme::dark() } else { Theme::modern() };
    theme.background = hex(colors.ground);
    theme.primary_color = colors.mix(text, 0.07);
    theme.secondary_color = colors.mix(text, 0.14);
    theme.tertiary_color = colors.mix(text, 0.04);
    theme.primary_text_color = hex(text);
    theme.text_color = hex(text);
    theme.primary_border_color = colors.mix(text, 0.45);
    theme.line_color = colors.mix(text, 0.6);
    theme.edge_label_background = hex(colors.ground);
    theme.cluster_background = colors.mix(text, 0.035);
    theme.cluster_border = colors.mix(text, 0.3);
    theme.sequence_actor_fill = colors.mix(text, 0.07);
    theme.sequence_actor_border = colors.mix(text, 0.45);
    theme.sequence_actor_line = colors.mix(text, 0.45);
    theme.sequence_note_fill = colors.mix(colors.accent, 0.18);
    theme.sequence_note_border = colors.mix(colors.accent, 0.6);
    theme.sequence_activation_fill = colors.mix(text, 0.14);
    theme.sequence_activation_border = colors.mix(text, 0.45);
    theme.pie_title_text_color = hex(text);
    theme.pie_section_text_color = hex(text);
    theme.pie_legend_text_color = hex(text);
    theme.pie_stroke_color = hex(colors.ground);
    theme.pie_outer_stroke_color = colors.mix(text, 0.3);
    theme.pie_opacity = 1.0;
    // Slices that read on light and dark grounds alike.
    const SLICES: [[u8; 3]; 12] = [
        [0x4e, 0x79, 0xa7],
        [0xf2, 0x8e, 0x2b],
        [0x59, 0xa1, 0x4f],
        [0xe1, 0x57, 0x59],
        [0x76, 0xb7, 0xb2],
        [0xed, 0xc9, 0x48],
        [0xb0, 0x7a, 0xa1],
        [0xff, 0x9d, 0xa7],
        [0x9c, 0x75, 0x5f],
        [0xba, 0xb0, 0xac],
        [0x6b, 0x8e, 0xc9],
        [0x8c, 0xd1, 0x7d],
    ];
    theme.pie_colors = SLICES.map(|c| colors.mix(c, 0.85));
    mermaid_rs_renderer::render_with_options(src, RenderOptions { theme, layout: Default::default() }).map_err(brief)
}

fn dot(src: &str, colors: &Colors) -> Result<String, String> {
    use layout::backends::svg::SVGWriter;
    use layout::gv::{DotParser, GraphBuilder};
    let graph = DotParser::new(src).process().map_err(|e| format!("not a dot graph: {}", brief(e)))?;
    let mut builder = GraphBuilder::new();
    builder.visit_graph(&graph);
    let mut visual = builder.get();
    if visual.num_nodes() == 0 {
        return Err("the graph has no nodes".into());
    }
    let mut writer = SVGWriter::new();
    visual.do_it(false, false, false, &mut writer);
    // Black on white, as layout-rs draws by default, becomes the theme's
    // ink on a faint fill; words take the sans-serif face and the ink.
    let ink = hex(colors.text);
    let svg = writer
        .finalize()
        .replace("#000000ff", &ink)
        .replace("#ffffffff", &colors.mix(colors.text, 0.07))
        .replace("font-family: Times, serif;", "font-family: sans-serif;")
        .replace("<style>", &format!("<style>\ntext {{ fill: {ink}; }}"));
    Ok(svg)
}

/// STIX Two Math, read once.
fn math_font() -> Option<&'static latex_rust::MathFont> {
    static FONT: OnceLock<Option<latex_rust::MathFont>> = OnceLock::new();
    FONT.get_or_init(|| latex_rust::MathFont::stix_two_math().ok()).as_ref()
}

fn math(tex: &str, colors: &Colors, font_px: f32) -> Result<String, String> {
    let font = math_font().ok_or("no math font")?;
    let mut options = latex_rust::SvgOptions::new();
    options.display = true;
    let [r, g, b] = colors.text;
    options.color = latex_rust::Color::rgb(r, g, b);
    // In points (the SVG's unit); a little larger than the text around it.
    options.font_size_pt = latex_rust::Dim::ratio((font_px * 1.15 * 0.75 * 16.0).round() as i64, 16);
    latex_rust::latex_to_svg(tex.trim(), font, &options).map_err(|e| brief(format!("{e:?}")))
}

/// The system's fonts, read once (for the words in diagrams).
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            db.set_sans_serif_family(if cfg!(target_os = "macos") { "Helvetica Neue" } else { "DejaVu Sans" });
            Arc::new(db)
        })
        .clone()
}

/// SVG as pixels: at its own size, or narrowed to `max_width`, times `scale`.
pub fn rasterize(svg: &str, max_width: f32, scale: f32) -> Result<Raster, String> {
    let options = usvg::Options { fontdb: fonts(), ..Default::default() };
    let tree = usvg::Tree::from_str(svg, &options).map_err(brief)?;
    let natural = tree.size();
    let (w, h) = (natural.width(), natural.height());
    if !(w > 0.0 && h > 0.0) {
        return Err("the picture is empty".into());
    }
    let fit = (max_width.max(16.0) / w).min(1.0);
    let size = (w * fit, h * fit);
    let mut px_scale = fit * scale.max(0.5);
    let longest = w.max(h) * px_scale;
    if longest > MAX_SIDE {
        px_scale *= MAX_SIDE / longest;
    }
    let (pw, ph) = ((w * px_scale).ceil().max(1.0) as u32, (h * px_scale).ceil().max(1.0) as u32);
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or("the picture is too large")?;
    let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resvg::render(&tree, tiny_skia::Transform::from_scale(px_scale, px_scale), &mut pixmap.as_mut());
    }));
    if drawn.is_err() {
        return Err("could not be drawn".into());
    }
    let mut rgba = Vec::with_capacity((pw * ph * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok(Raster { width: pw, height: ph, rgba, size })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: Colors = Colors { text: [0xdd, 0xdd, 0xdd], ground: [0x1e, 0x1e, 0x24], accent: [0xff, 0x8a, 0x57] };
    const LIGHT: Colors = Colors { text: [0x22, 0x22, 0x22], ground: [0xfa, 0xfa, 0xfa], accent: [0xff, 0x8a, 0x57] };

    /// Mermaid keeps a copy of a font in `$XDG_CACHE_HOME/mmdr`: the tests
    /// keep theirs in a scratch folder, not in the home's cache.
    fn scratch_cache() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let dir = std::env::temp_dir().join("shed-markdown-tests");
            // SAFETY: every test here that draws calls this before drawing,
            // and nothing else in these tests reads the environment.
            unsafe { std::env::set_var("XDG_CACHE_HOME", dir) };
        });
    }

    fn draws(source: Source, colors: &Colors) -> Raster {
        scratch_cache();
        let r = picture(source, colors, 14.0, 600.0, 2.0).unwrap_or_else(|e| panic!("{source:?}: {e}"));
        assert!(r.width > 10 && r.height > 10, "{source:?}: {}×{}", r.width, r.height);
        assert_eq!(r.rgba.len(), (r.width * r.height * 4) as usize);
        assert!(r.size.0 <= 600.0);
        r
    }

    #[test]
    fn mermaid_diagrams_of_every_common_kind_draw() {
        let kinds = [
            "flowchart LR\n  A[Start] --> B{Ok?}\n  B -->|yes| C[Done]\n  B -->|no| A",
            "graph TD; A-->B; A-->C; B-->D; C-->D;",
            "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi",
            "classDiagram\n  Animal <|-- Duck\n  Animal : +int age\n  class Duck{\n    +swim()\n  }",
            "stateDiagram-v2\n  [*] --> Still\n  Still --> Moving\n  Moving --> [*]",
            "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE-ITEM : contains",
            "gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section A\n  One :a1, 2024-01-01, 30d\n  Two :after a1, 20d",
            "pie title Pets\n  \"Dogs\" : 386\n  \"Cats\" : 85",
            "mindmap\n  root((Shed))\n    Benches\n    Tabs",
            "gitGraph\n  commit\n  branch dev\n  commit\n  checkout main\n  merge dev",
            "journey\n  title Day\n  section Work\n    Code: 5: Me",
            "timeline\n  title History\n  2024 : One\n  2025 : Two",
        ];
        for src in kinds {
            draws(Source::Diagram(DiagramKind::Mermaid, src), &DARK);
        }
        draws(Source::Diagram(DiagramKind::Mermaid, kinds[0]), &LIGHT);
    }

    #[test]
    fn diagrams_follow_the_theme() {
        scratch_cache();
        let src = "flowchart LR\n  A --> B";
        let dark = svg(Source::Diagram(DiagramKind::Mermaid, src), &DARK, 14.0).unwrap();
        let light = svg(Source::Diagram(DiagramKind::Mermaid, src), &LIGHT, 14.0).unwrap();
        assert!(dark.contains("#1e1e24") && dark.contains("#dddddd"));
        assert!(light.contains("#fafafa") && light.contains("#222222"));
        let dot = svg(Source::Diagram(DiagramKind::Dot, "digraph { a -> b }"), &DARK, 14.0).unwrap();
        assert!(dot.contains("#dddddd") && !dot.contains("#000000ff"));
    }

    #[test]
    fn broken_sources_say_why() {
        scratch_cache();
        for src in ["flowchart TD\n  A --> \n  ]]] B{", "not a diagram at all", ""] {
            let e = picture(Source::Diagram(DiagramKind::Mermaid, src), &DARK, 14.0, 600.0, 1.0).unwrap_err();
            assert!(!e.is_empty() && !e.contains('\n') && e.chars().count() <= 161, "{e}");
        }
        assert!(picture(Source::Diagram(DiagramKind::Dot, "digraph { a -> ; }"), &DARK, 14.0, 600.0, 1.0).is_err());
        assert!(picture(Source::Math(r"\frac{1}{"), &DARK, 14.0, 600.0, 1.0).is_err());
    }

    #[test]
    fn dot_and_math_draw() {
        draws(Source::Diagram(DiagramKind::Dot, "digraph G { rankdir=LR; a -> b [label=\"x\"]; b -> c; c [shape=box]; }"), &DARK);
        let m = draws(Source::Math(r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}"), &LIGHT);
        draws(Source::Math(r"\begin{aligned} a &= b + c \\ d &= \sum_{i=1}^n x_i^2 \end{aligned}"), &DARK);
        // Math is drawn at about the text's size: a line or two high.
        assert!(m.size.1 > 14.0 && m.size.1 < 80.0, "{:?}", m.size);
    }

    #[test]
    fn wide_pictures_are_narrowed_not_enlarged() {
        scratch_cache();
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="100"><rect width="1000" height="100" fill="red"/></svg>"#;
        let r = rasterize(svg, 500.0, 2.0).unwrap();
        assert_eq!(r.size, (500.0, 50.0));
        assert_eq!((r.width, r.height), (1000, 100));
        let r = rasterize(svg, 2000.0, 1.0).unwrap();
        assert_eq!(r.size, (1000.0, 100.0));
        assert_eq!(&r.rgba[..4], &[255, 0, 0, 255]);
    }
}

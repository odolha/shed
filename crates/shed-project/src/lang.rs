//! The languages Shed colours, found by file name, and how to colour them.

use std::path::Path;
use std::sync::OnceLock;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// What a piece of code is, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Number,
    Constant,
    Property,
    Tag,
    Attribute,
    Builtin,
    Escape,
    Label,
    Heading,
    Punctuation,
}

/// Capture names, by dot-separated prefix (the longest that matches wins),
/// and what they are.
const CAPTURES: &[(&str, Style)] = &[
    ("keyword", Style::Keyword),
    ("conditional", Style::Keyword),
    ("repeat", Style::Keyword),
    ("include", Style::Keyword),
    ("exception", Style::Keyword),
    ("storageclass", Style::Keyword),
    ("string", Style::String),
    ("character", Style::String),
    ("text.literal", Style::String),
    ("text.uri", Style::String),
    ("comment", Style::Comment),
    ("function", Style::Function),
    ("method", Style::Function),
    ("constructor", Style::Type),
    ("type", Style::Type),
    ("module", Style::Type),
    ("namespace", Style::Type),
    ("number", Style::Number),
    ("float", Style::Number),
    ("boolean", Style::Constant),
    ("constant", Style::Constant),
    ("constant.builtin", Style::Constant),
    ("property", Style::Property),
    ("field", Style::Property),
    ("tag", Style::Tag),
    ("attribute", Style::Attribute),
    ("variable.builtin", Style::Builtin),
    ("function.builtin", Style::Builtin),
    ("function.macro", Style::Builtin),
    ("escape", Style::Escape),
    ("string.escape", Style::Escape),
    ("string.special", Style::Escape),
    ("label", Style::Label),
    ("text.title", Style::Heading),
    ("markup.heading", Style::Heading),
    ("punctuation.special", Style::Escape),
    ("operator", Style::Punctuation),
];

pub struct Lang {
    pub name: &'static str,
    /// What line comments start with, if the language has them.
    pub comment: Option<&'static str>,
    config: fn() -> Option<HighlightConfiguration>,
    loaded: OnceLock<Option<HighlightConfiguration>>,
}

impl Lang {
    fn config(&self) -> Option<&HighlightConfiguration> {
        self.loaded
            .get_or_init(|| {
                let mut c = (self.config)()?;
                let names: Vec<&str> = CAPTURES.iter().map(|(n, _)| *n).collect();
                c.configure(&names);
                Some(c)
            })
            .as_ref()
    }
}

fn configure(language: tree_sitter::Language, name: &str, highlights: &str, injections: &str) -> Option<HighlightConfiguration> {
    match HighlightConfiguration::new(language, name, highlights, injections, "") {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!("cannot colour {name}: {e:?}");
            None
        }
    }
}

macro_rules! lang {
    ($name:literal, $comment:expr, $config:expr) => {
        Lang { name: $name, comment: $comment, config: || $config, loaded: OnceLock::new() }
    };
}

static RUST: Lang = lang!("Rust", Some("//"), configure(tree_sitter_rust::LANGUAGE.into(), "rust", tree_sitter_rust::HIGHLIGHTS_QUERY, tree_sitter_rust::INJECTIONS_QUERY));
static JAVASCRIPT: Lang = lang!(
    "JavaScript",
    Some("//"),
    configure(
        tree_sitter_javascript::LANGUAGE.into(),
        "javascript",
        &format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY),
        tree_sitter_javascript::INJECTIONS_QUERY
    )
);
static TYPESCRIPT: Lang = lang!(
    "TypeScript",
    Some("//"),
    configure(
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "typescript",
        &format!("{}\n{}", tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY),
        ""
    )
);
static TSX: Lang = lang!(
    "TSX",
    Some("//"),
    configure(
        tree_sitter_typescript::LANGUAGE_TSX.into(),
        "tsx",
        &format!(
            "{}\n{}\n{}",
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            tree_sitter_javascript::HIGHLIGHT_QUERY
        ),
        ""
    )
);
static HTML: Lang = lang!("HTML", None, configure(tree_sitter_html::LANGUAGE.into(), "html", tree_sitter_html::HIGHLIGHTS_QUERY, tree_sitter_html::INJECTIONS_QUERY));
static CSS: Lang = lang!("CSS", None, configure(tree_sitter_css::LANGUAGE.into(), "css", tree_sitter_css::HIGHLIGHTS_QUERY, ""));
static JAVA: Lang = lang!("Java", Some("//"), configure(tree_sitter_java::LANGUAGE.into(), "java", tree_sitter_java::HIGHLIGHTS_QUERY, ""));
static KOTLIN: Lang = lang!("Kotlin", Some("//"), configure(tree_sitter_kotlin_ng::LANGUAGE.into(), "kotlin", include_str!("../queries/kotlin.scm"), ""));
static JSON: Lang = lang!("JSON", None, configure(tree_sitter_json::LANGUAGE.into(), "json", tree_sitter_json::HIGHLIGHTS_QUERY, ""));
static PYTHON: Lang = lang!("Python", Some("#"), configure(tree_sitter_python::LANGUAGE.into(), "python", tree_sitter_python::HIGHLIGHTS_QUERY, ""));
static BASH: Lang = lang!("Shell", Some("#"), configure(tree_sitter_bash::LANGUAGE.into(), "bash", tree_sitter_bash::HIGHLIGHT_QUERY, ""));
static GO: Lang = lang!("Go", Some("//"), configure(tree_sitter_go::LANGUAGE.into(), "go", tree_sitter_go::HIGHLIGHTS_QUERY, ""));
static C: Lang = lang!("C", Some("//"), configure(tree_sitter_c::LANGUAGE.into(), "c", tree_sitter_c::HIGHLIGHT_QUERY, ""));
static CPP: Lang = lang!(
    "C++",
    Some("//"),
    configure(tree_sitter_cpp::LANGUAGE.into(), "cpp", &format!("{}\n{}", tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY), "")
);
static YAML: Lang = lang!("YAML", Some("#"), configure(tree_sitter_yaml::LANGUAGE.into(), "yaml", tree_sitter_yaml::HIGHLIGHTS_QUERY, ""));
static MARKDOWN: Lang = lang!("Markdown", None, configure(tree_sitter_md::LANGUAGE.into(), "markdown", tree_sitter_md::HIGHLIGHT_QUERY_BLOCK, ""));
static TOML: Lang = lang!("TOML", Some("#"), configure(tree_sitter_toml_ng::LANGUAGE.into(), "toml", tree_sitter_toml_ng::HIGHLIGHTS_QUERY, ""));
static XML: Lang = lang!("XML", None, configure(tree_sitter_xml::LANGUAGE_XML.into(), "xml", tree_sitter_xml::XML_HIGHLIGHT_QUERY, ""));

/// The language of a file, by its name.
pub fn detect(path: &Path) -> Option<&'static Lang> {
    let name = path.file_name()?.to_str()?;
    let lower = name.to_ascii_lowercase();
    if matches!(lower.as_str(), "cargo.lock" | "pipfile" | "poetry.lock") {
        return Some(&TOML);
    }
    if matches!(lower.as_str(), ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" | ".zprofile" | ".envrc" | "pkgbuild") {
        return Some(&BASH);
    }
    let ext = lower.rsplit_once('.').map(|(_, e)| e)?;
    Some(match ext {
        "rs" => &RUST,
        "js" | "mjs" | "cjs" | "jsx" => &JAVASCRIPT,
        "ts" | "mts" | "cts" => &TYPESCRIPT,
        "tsx" => &TSX,
        "html" | "htm" | "xhtml" => &HTML,
        "css" | "scss" | "less" => &CSS,
        "java" => &JAVA,
        "kt" | "kts" => &KOTLIN,
        "json" | "jsonc" | "json5" | "webmanifest" => &JSON,
        "py" | "pyi" => &PYTHON,
        "sh" | "bash" | "zsh" => &BASH,
        "go" => &GO,
        "c" | "h" => &C,
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => &CPP,
        "yml" | "yaml" => &YAML,
        "md" | "markdown" => &MARKDOWN,
        "toml" => &TOML,
        "xml" | "svg" | "xsd" | "xsl" | "plist" | "pom" | "iml" | "csproj" => &XML,
        _ => return None,
    })
}

/// The language a code block names (```rust, ```py, ```sh…).
pub fn by_name(name: &str) -> Option<&'static Lang> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    let alias = match lower.as_str() {
        "rust" => "rs",
        "javascript" | "node" => "js",
        "typescript" => "ts",
        "python" | "python3" => "py",
        "shell" | "console" | "shellsession" => "sh",
        "kotlin" => "kt",
        "golang" => "go",
        "c++" => "cpp",
        "markdown" => "md",
        other => other,
    };
    detect(Path::new(&format!("x.{alias}")))
}

fn by_injection_name(name: &str) -> Option<&'static Lang> {
    Some(match name {
        "javascript" | "js" => &JAVASCRIPT,
        "typescript" | "ts" => &TYPESCRIPT,
        "css" => &CSS,
        "html" => &HTML,
        "json" => &JSON,
        "rust" => &RUST,
        "bash" | "sh" | "shell" => &BASH,
        "python" => &PYTHON,
        _ => return None,
    })
}

/// Each line's coloured spans: byte ranges within the line, in order.
pub type Spans = Vec<Vec<(std::ops::Range<usize>, Style)>>;

/// Colours `text` as `lang`. Returns `None` if it cannot be parsed.
pub fn highlight(lang: &Lang, text: &str) -> Option<Spans> {
    let config = lang.config()?;
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, text.as_bytes(), None, None, |name| by_injection_name(name).and_then(Lang::config))
        .ok()?;
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let mut spans: Spans = vec![Vec::new(); line_starts.len()];
    let mut stack: Vec<Style> = Vec::new();
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(h) => stack.push(CAPTURES[h.0].1),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let Some(style) = stack.last().copied() else { continue };
                // Split at line breaks.
                let mut line = line_starts.partition_point(|s| *s <= start) - 1;
                let mut at = start;
                while at < end {
                    let line_end = line_starts.get(line + 1).map_or(text.len(), |s| s - 1).max(at);
                    let stop = end.min(line_end);
                    if stop > at {
                        spans[line].push((at - line_starts[line]..stop - line_starts[line], style));
                    }
                    at = line_end + 1;
                    line += 1;
                    if line >= line_starts.len() {
                        break;
                    }
                }
            }
        }
    }
    Some(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_by_name() {
        assert_eq!(detect(Path::new("src/main.rs")).map(|l| l.name), Some("Rust"));
        assert_eq!(detect(Path::new("App.TSX")).map(|l| l.name), Some("TSX"));
        assert_eq!(detect(Path::new("Cargo.lock")).map(|l| l.name), Some("TOML"));
        assert_eq!(detect(Path::new("build.gradle.kts")).map(|l| l.name), Some("Kotlin"));
        assert!(detect(Path::new("LICENSE")).is_none());
    }

    #[test]
    fn spans_per_line() {
        let text = "fn main() {\n    // hi\n    let s = \"a\nb\";\n}\n";
        let spans = highlight(&RUST, text).unwrap();
        assert_eq!(spans.len(), 6);
        assert!(spans[0].contains(&(0..2, Style::Keyword)), "{:?}", spans[0]);
        assert!(spans[1].contains(&(4..9, Style::Comment)), "{:?}", spans[1]);
        // A string over two lines is split at the break.
        assert!(spans[2].contains(&(12..14, Style::String)), "{:?}", spans[2]);
        assert!(spans[3].iter().any(|(r, s)| *s == Style::String && r.start == 0), "{:?}", spans[3]);
    }

    #[test]
    fn every_language_loads() {
        for lang in [&RUST, &JAVASCRIPT, &TYPESCRIPT, &TSX, &HTML, &CSS, &JAVA, &KOTLIN, &JSON, &PYTHON, &BASH, &GO, &C, &CPP, &YAML, &MARKDOWN, &TOML, &XML] {
            assert!(highlight(lang, "x").is_some(), "{}", lang.name);
        }
        let html = highlight(&HTML, "<script>let a = 1;</script>").unwrap();
        assert!(html[0].iter().any(|(_, s)| *s == Style::Keyword), "scripts in HTML are coloured: {html:?}");
    }
}

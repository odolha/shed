//! Projects' files: a folder's entries, quick open, reading (with colours),
//! light writing; and folders on the machine to choose from.

use std::path::{Path, PathBuf};

use shed_api::calls::{self as api, Call, FileKind, Reply};
use shed_api::topics::Highlight;
use shed_api::{ApiError, ErrorCode};
use shed_model::Shed;

use super::{Ctx, invalid, not_found, open_tab};
use crate::keeper;

/// Larger files are not sent.
pub(crate) const MAX_TEXT: u64 = 2 * 1024 * 1024;

/// A project or AI tab's folder, and what its file list leaves out.
pub(crate) fn root_of(tree: &Shed, tab: &str) -> Result<(PathBuf, Vec<String>), ApiError> {
    let (_, t) = open_tab(tree, tab)?;
    match &t.kind {
        shed_model::TabKind::Project(p) => Ok((p.root.clone(), p.exclude.clone())),
        shed_model::TabKind::Ai(a) => Ok((a.folder.clone(), Vec::new())),
        _ => Err(invalid("not a project or AI tab")),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| keeper::failed(e.to_string()))?
}

pub(super) async fn call(ctx: &std::sync::Arc<Ctx>, tree: &Shed, call: Call) -> Result<Reply, ApiError> {
    match call {
        Call::ListFolder(a) => {
            let (root, exclude) = root_of(tree, &a.tab)?;
            let dir = crate::paths::inside(&root, &a.path)?;
            blocking(move || {
                if !dir.is_dir() {
                    return Err(not_found("no such folder"));
                }
                let excluder = shed_project::Excluder::new(&root, &exclude);
                Ok(Reply::Folder(
                    shed_project::read_folder(&dir)
                        .into_iter()
                        .map(|e| api::FolderEntry { ignored: excluder.excludes(&e.path, e.dir), name: e.name, dir: e.dir })
                        .collect(),
                ))
            })
            .await
        }
        Call::QuickOpen(a) => {
            let (root, exclude) = root_of(tree, &a.tab)?;
            let ctx = ctx.clone();
            let limit = if a.limit == 0 { 50 } else { a.limit.min(500) } as usize;
            blocking(move || {
                let files = ctx.files(&root, &exclude);
                Ok(Reply::Files(
                    shed_project::fuzzy::rank(&a.query, &files, limit)
                        .into_iter()
                        .map(|(i, hits)| api::FileMatch { path: slash(&files[i]), hits })
                        .collect(),
                ))
            })
            .await
        }
        Call::ReadFile(a) => {
            let (root, _) = root_of(tree, &a.tab)?;
            let path = crate::paths::inside(&root, &a.path)?;
            blocking(move || read(&path, a.path).map(Reply::File)).await
        }
        Call::WriteFile(a) => {
            let (root, _) = root_of(tree, &a.tab)?;
            let path = crate::paths::inside(&root, &a.path)?;
            if a.path.is_empty() {
                return Err(invalid("no file named"));
            }
            let ctx = ctx.clone();
            blocking(move || {
                if path.is_dir() {
                    return Err(invalid("that is a folder"));
                }
                if !path.parent().is_some_and(Path::is_dir) {
                    return Err(not_found("no such folder"));
                }
                shed_model::paths::write_whole(&path, a.text.as_bytes()).map_err(|e| keeper::failed(e.to_string()))?;
                ctx.forget_files(&root);
                Ok(Reply::Done)
            })
            .await
        }
        Call::CreateEntry(a) => {
            let (root, _) = root_of(tree, &a.tab)?;
            let folder = a.path.ends_with('/');
            let rel = a.path.trim_end_matches('/');
            if rel.is_empty() {
                return Err(invalid("no name given"));
            }
            let path = crate::paths::inside(&root, rel)?;
            let ctx = ctx.clone();
            blocking(move || {
                if path.exists() {
                    return Err(invalid("it is there already"));
                }
                let made = if folder {
                    std::fs::create_dir_all(&path)
                } else {
                    path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map(drop))
                };
                made.map_err(|e| keeper::failed(e.to_string()))?;
                ctx.forget_files(&root);
                Ok(Reply::Done)
            })
            .await
        }
        Call::BrowseFolders(a) => {
            let dir = crate::paths::expand(&a.path)?;
            blocking(move || {
                let entries = std::fs::read_dir(&dir).map_err(|e| not_found(format!("{}: {e}", dir.display())))?;
                let mut names: Vec<String> = entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with('.'))
                    .collect();
                names.sort_by_key(|n| n.to_lowercase());
                Ok(Reply::Folder(names.into_iter().map(|name| api::FolderEntry { name, dir: true, ignored: false }).collect()))
            })
            .await
        }
        _ => Err(ApiError::new(ErrorCode::Unsupported, "not a project call")),
    }
}

/// A relative path with `/` between its parts.
pub(crate) fn slash(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// A file as connectors get it: text with its colours, a picture, or just
/// what it is (binary, too large).
pub(crate) fn read(path: &Path, rel: String) -> Result<api::FileContent, ApiError> {
    let meta = std::fs::metadata(path).map_err(|_| not_found("no such file"))?;
    if meta.is_dir() {
        return Err(invalid("that is a folder"));
    }
    let size = meta.len() as f64;
    let empty = |kind| api::FileContent { path: rel.clone(), kind, text: String::new(), lang: None, highlights: Vec::new(), picture: None, size };
    if shed_project::picture::is_picture(path) {
        return Ok(match shed_project::picture::PictureData::read(path) {
            Some(p) => api::FileContent { picture: Some(super::ai::picture_of(&p)), ..empty(FileKind::Picture) },
            None => empty(FileKind::TooLarge),
        });
    }
    if meta.len() > MAX_TEXT {
        return Ok(empty(FileKind::TooLarge));
    }
    let bytes = std::fs::read(path).map_err(|e| keeper::failed(e.to_string()))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Ok(empty(FileKind::Binary));
    }
    let Ok(text) = String::from_utf8(bytes) else { return Ok(empty(FileKind::Binary)) };
    let lang = shed_project::detect(path);
    let highlights = lang.and_then(|l| shed_project::highlight(l, &text)).map(|spans| highlights(&text, &spans)).unwrap_or_default();
    Ok(api::FileContent { text, lang: lang.map(|l| l.name.to_string()), highlights, ..empty(FileKind::Text) })
}

/// Per line, its coloured ranges in character positions (the highlighter
/// gives bytes), each with its kind's name: `keyword`, `string`…
pub(crate) fn highlights(text: &str, spans: &shed_project::Spans) -> Vec<Vec<Highlight>> {
    text.split('\n')
        .zip(spans)
        .map(|(line, spans)| {
            let char_at = |byte: usize| line[..byte.min(line.len())].chars().count() as u32;
            spans
                .iter()
                .filter(|(r, _)| line.is_char_boundary(r.start.min(line.len())) && line.is_char_boundary(r.end.min(line.len())))
                .map(|(r, style)| Highlight { start: char_at(r.start), end: char_at(r.end), kind: style_name(*style).into() })
                .collect()
        })
        .collect()
}

pub(crate) fn style_name(s: shed_project::Style) -> &'static str {
    use shed_project::Style;
    match s {
        Style::Keyword => "keyword",
        Style::String => "string",
        Style::Comment => "comment",
        Style::Function => "function",
        Style::Type => "type",
        Style::Number => "number",
        Style::Constant => "constant",
        Style::Property => "property",
        Style::Tag => "tag",
        Style::Attribute => "attribute",
        Style::Builtin => "builtin",
        Style::Escape => "escape",
        Style::Label => "label",
        Style::Heading => "heading",
        Style::Punctuation => "operator",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_in_characters() {
        let text = "let é = \"ü\"; // x\nfn f() {}";
        let lang = shed_project::by_name("rust").unwrap();
        let spans = shed_project::highlight(lang, text).unwrap();
        let lines = highlights(text, &spans);
        assert_eq!(lines.len(), 2);
        let string = lines[0].iter().find(|h| h.kind == "string").unwrap();
        // `"ü"` starts at character 8 (byte 9: é is two bytes) and is 3 long.
        assert_eq!((string.start, string.end), (8, 11));
        let comment = lines[0].iter().find(|h| h.kind == "comment").unwrap();
        assert_eq!((comment.start, comment.end), (13, 17));
        assert_eq!(lines[1][0], Highlight { start: 0, end: 2, kind: "keyword".into() });
    }

    #[test]
    fn files_by_kind() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("b.bin"), [0u8, 1, 2]).unwrap();
        let a = read(&dir.path().join("a.rs"), "a.rs".into()).unwrap();
        assert_eq!((a.kind, a.lang.as_deref()), (FileKind::Text, Some("Rust")));
        assert_eq!(a.highlights.len(), 2);
        assert_eq!(read(&dir.path().join("b.bin"), "b.bin".into()).unwrap().kind, FileKind::Binary);
        assert_eq!(read(&dir.path().join("none"), "none".into()).unwrap_err().code, ErrorCode::NotFound);
    }
}

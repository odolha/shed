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
                        .map(|(i, hits)| {
                            let path = slash(&files[i]);
                            let hits = utf16_of_chars(&path, &hits);
                            api::FileMatch { path, hits }
                        })
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
            blocking(move || Ok(Reply::Folder(folders_in(&dir)?))).await
        }
        Call::Browse(a) => {
            let dir = crate::paths::expand(&a.path)?;
            blocking(move || {
                // Where it looked, as one path (no `..` nor links in it).
                let dir = std::fs::canonicalize(&dir).map_err(|e| not_found(format!("{}: {e}", dir.display())))?;
                let entries = folders_in(&dir)?;
                Ok(Reply::Browsed(api::Browsed {
                    path: dir.to_string_lossy().into_owned(),
                    parent: dir.parent().map(|p| p.to_string_lossy().into_owned()),
                    entries,
                }))
            })
            .await
        }
        _ => Err(ApiError::new(ErrorCode::Unsupported, "not a project call")),
    }
}

/// The folders in `dir`, hidden ones left out, by name.
fn folders_in(dir: &Path) -> Result<Vec<api::FolderEntry>, ApiError> {
    let entries = std::fs::read_dir(dir).map_err(|e| not_found(format!("{}: {e}", dir.display())))?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    Ok(names.into_iter().map(|name| api::FolderEntry { name, dir: true, ignored: false }).collect())
}

/// A byte offset in `text` as a position in UTF-16 code units, which is
/// how the API counts (JavaScript and Kotlin strings index so).
pub(crate) fn utf16_at(text: &str, byte: usize) -> u32 {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    text[..byte].encode_utf16().count() as u32
}

/// Character positions in `text` (as the fuzzy matcher gives them) as
/// UTF-16 positions.
pub(crate) fn utf16_of_chars(text: &str, chars: &[u32]) -> Vec<u32> {
    let mut at = Vec::with_capacity(text.len() + 1);
    let mut units = 0u32;
    for c in text.chars() {
        at.push(units);
        units += c.len_utf16() as u32;
    }
    at.push(units);
    chars.iter().filter_map(|&i| at.get(i as usize).copied()).collect()
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

/// Per line, its coloured ranges in UTF-16 positions (the highlighter gives
/// bytes), each with its kind's name: `keyword`, `string`…
pub(crate) fn highlights(text: &str, spans: &shed_project::Spans) -> Vec<Vec<Highlight>> {
    text.split('\n')
        .zip(spans)
        .map(|(line, spans)| {
            spans
                .iter()
                .filter(|(r, _)| line.is_char_boundary(r.start.min(line.len())) && line.is_char_boundary(r.end.min(line.len())))
                .map(|(r, style)| Highlight { start: utf16_at(line, r.start), end: utf16_at(line, r.end), kind: style_name(*style).into() })
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
    fn highlights_in_utf16_units() {
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
        // An emoji is two UTF-16 units (four bytes, one character).
        let text = "let s = \"🦀\"; // 🦀 ok";
        let spans = shed_project::highlight(lang, text).unwrap();
        let line = &highlights(text, &spans)[0];
        let string = line.iter().find(|h| h.kind == "string").unwrap();
        assert_eq!((string.start, string.end), (8, 12));
        let comment = line.iter().find(|h| h.kind == "comment").unwrap();
        let units: Vec<u16> = text.encode_utf16().collect();
        assert_eq!(String::from_utf16(&units[comment.start as usize..comment.end as usize]).unwrap(), "// 🦀 ok");
    }

    #[test]
    fn positions_in_utf16_units() {
        let text = "a🦀b€c";
        assert_eq!([0, 1, 5, 6, 9, 10].map(|b| utf16_at(text, b)), [0, 1, 3, 4, 5, 6]);
        // Inside a character: where it starts.
        assert_eq!(utf16_at(text, 2), 1);
        assert_eq!(utf16_of_chars(text, &[0, 1, 2, 3, 4, 5]), vec![0, 1, 3, 4, 5, 6]);
        assert_eq!(utf16_of_chars(text, &[9]), Vec::<u32>::new());
    }

    #[test]
    fn folders_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("b/c")).unwrap();
        std::fs::create_dir_all(dir.path().join("A")).unwrap();
        std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join("file"), "").unwrap();
        let names: Vec<String> = folders_in(dir.path()).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["A", "b"]);
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

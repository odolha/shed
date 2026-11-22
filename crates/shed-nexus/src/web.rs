//! The web app's files, served at `/`: built into the binary (`build.rs`
//! builds `web/` for release builds), or read from `web_dir` when that is
//! set. Either way the same rules: any path that is not a file gets
//! `index.html` (the app's routes), hashed bundles are cached for a year and
//! everything else is checked again each time (`no-cache`, with an `ETag`).

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{self, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};

/// One file of the built web app.
pub struct WebFile {
    /// Relative to the app's root, with `/` (`index.html`, `icons/shed.svg`).
    pub path: &'static str,
    pub bytes: &'static [u8],
    /// A quoted hash of the bytes.
    pub etag: &'static str,
}

/// The web app built into this binary: empty when it was built without it
/// (a debug build, or `SHED_NEXUS_NO_WEB=1`).
pub static EMBEDDED: &[WebFile] = include!(concat!(env!("OUT_DIR"), "/web_files.rs"));

/// Where the web app's files come from.
pub enum WebSource {
    /// Built into the binary.
    Embedded(&'static [WebFile]),
    /// A folder on disk (`web_dir`), read at each request.
    Dir(PathBuf),
    /// No web app: only the API.
    None,
}

impl WebSource {
    /// `web_dir` if set, else the built-in app, if there is one.
    pub fn new(web_dir: Option<&Path>) -> WebSource {
        match web_dir {
            Some(dir) => WebSource::Dir(dir.to_path_buf()),
            None if !EMBEDDED.is_empty() => WebSource::Embedded(EMBEDDED),
            None => WebSource::None,
        }
    }

    /// What the log says about it.
    pub fn describe(&self) -> String {
        match self {
            WebSource::Embedded(files) => format!("the web app is built in ({} files)", files.len()),
            WebSource::Dir(dir) => format!("the web app is served from {}", dir.display()),
            WebSource::None => "this build has no web app (a debug build, or built with SHED_NEXUS_NO_WEB): \
                                serving only the API"
                .into(),
        }
    }

    async fn file(&self, path: &str) -> Option<(Body, String)> {
        match self {
            WebSource::Embedded(files) => {
                files.iter().find(|f| f.path == path).map(|f| (Body::from(f.bytes), f.etag.to_string()))
            }
            WebSource::Dir(dir) => {
                let full = dir.join(path);
                let meta = tokio::fs::metadata(&full).await.ok()?;
                if !meta.is_file() {
                    return None;
                }
                let bytes = tokio::fs::read(&full).await.ok()?;
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                Some((Body::from(bytes), format!("\"{modified:x}-{:x}\"", meta.len())))
            }
            WebSource::None => None,
        }
    }

    /// Answers a request for the app: the file, or `index.html` for any
    /// path that is not one.
    pub async fn serve(&self, request: Request) -> Response {
        if matches!(self, WebSource::None) {
            return crate::http::Failure::new(shed_api::ErrorCode::NotFound, "no web app here").into_response();
        }
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "GET, HEAD")]).into_response();
        }
        let path = match request.uri().path().trim_start_matches('/') {
            "" => "index.html",
            path => path,
        };
        let found = match plain_path(path) {
            true => self.file(path).await.map(|file| (path, file)),
            false => None,
        };
        let Some((path, (body, etag))) = (match found {
            Some(found) => Some(found),
            None => self.file("index.html").await.map(|file| ("index.html", file)),
        }) else {
            return crate::http::Failure::new(shed_api::ErrorCode::NotFound, "no web app here").into_response();
        };
        let cache = if hashed(path) { "public, max-age=31536000, immutable" } else { "no-cache" };
        let etag_value = HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"\""));
        let unchanged = request
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|tag| tag.trim() == etag || tag.trim() == "*"));
        if unchanged {
            return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag_value), (header::CACHE_CONTROL, HeaderValue::from_static(cache))])
                .into_response();
        }
        let headers = [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type(path))),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            (header::ETAG, etag_value),
        ];
        (headers, body).into_response()
    }
}

/// A path made only of plain names (letters, digits, `.`, `_`, `-`, `~`),
/// none hidden, empty, `.` or `..`: safe to look up, on disk too.
fn plain_path(path: &str) -> bool {
    path.split('/').all(|part| {
        !part.is_empty()
            && !part.starts_with('.')
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'~'))
    })
}

/// Bundles whose name carries a hash of their content (`main-JLFEZLOM.js`,
/// `chunk-B0q9X4-q.js`, `styles-PDJCIHXX.css`, anything under `media/`):
/// a new build makes new names, so they can be cached for good.
fn hashed(path: &str) -> bool {
    if path.starts_with("media/") {
        return true;
    }
    let Some((stem, ext)) = path.rsplit_once('.') else { return false };
    if !matches!(ext, "js" | "css") || stem.contains('/') {
        return false;
    }
    let bytes = stem.as_bytes();
    bytes.len() > 9
        && bytes[bytes.len() - 9] == b'-'
        && bytes[bytes.len() - 8..].iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static FILES: &[WebFile] = &[
        WebFile { path: "index.html", bytes: b"<html>shed</html>", etag: "\"i\"" },
        WebFile { path: "main-JLFEZLOM.js", bytes: b"1", etag: "\"m\"" },
        WebFile { path: "icons/shed.svg", bytes: b"<svg/>", etag: "\"s\"" },
    ];

    async fn get(path: &str, etag: Option<&str>) -> Response {
        let mut request = Request::builder().uri(path);
        if let Some(etag) = etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        WebSource::Embedded(FILES).serve(request.body(Body::empty()).unwrap()).await
    }

    async fn text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn built_in_files_are_served_with_their_type_and_caching() {
        let page = get("/", None).await;
        assert_eq!(page.headers()[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(page.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(text(page).await, "<html>shed</html>");

        let bundle = get("/main-JLFEZLOM.js", None).await;
        assert_eq!(bundle.headers()[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(bundle.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        assert_eq!(bundle.headers()[header::ETAG], "\"m\"");

        let icon = get("/icons/shed.svg", None).await;
        assert_eq!(icon.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(icon.headers()[header::CACHE_CONTROL], "no-cache");

        assert_eq!(get("/icons/shed.svg", Some("\"s\"")).await.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn other_paths_get_the_index() {
        for path in ["/w/abc/b/def/t/ghi", "/setup?code=x", "/../Cargo.toml", "/.hidden", "/main-OLDHASH0.js"] {
            let page = get(path, None).await;
            assert_eq!(page.status(), StatusCode::OK, "{path}");
            assert_eq!(page.headers()[header::CACHE_CONTROL], "no-cache", "{path}");
            assert_eq!(text(page).await, "<html>shed</html>", "{path}");
        }
    }

    #[test]
    fn hashed_names_are_told_apart() {
        assert!(hashed("main-JLFEZLOM.js"));
        assert!(hashed("chunk-B0q9X4-q.js"));
        assert!(hashed("styles-PDJCIHXX.css"));
        assert!(!hashed("sw.js"));
        assert!(!hashed("icons/shed-badge-96.png"));
        assert!(!hashed("index.html"));
        assert!(!plain_path("a/../b"));
        assert!(!plain_path("a//b"));
        assert!(plain_path("icons/shed-192.png"));
    }
}

//! A bench's bookmarks: files (at a line), folders, and messages of its AI
//! tabs' conversations, to come back to. Kept with the bench.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{BookmarkId, TabId};

/// How long a message's excerpt is, in characters.
pub const EXCERPT: usize = 120;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: BookmarkId,
    /// What the list shows: a file's name (and line), a folder's, a
    /// message's first words, or what the user named it.
    pub label: String,
    pub kind: BookmarkKind,
    /// When it was made, Unix seconds.
    pub created: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BookmarkKind {
    /// A file (absolute), at a line from 1.
    File { path: PathBuf, line: Option<u32> },
    Folder { path: PathBuf },
    /// A message of an AI tab's conversation.
    AiMessage {
        tab: TabId,
        /// Claude Code's session it is in.
        session: Option<String>,
        /// Claude Code's id for the message, when known.
        uuid: Option<String>,
        role: MessageRole,
        /// Its first words (`excerpt`), to find it by when its id is not known.
        excerpt: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// The user's prompt.
    #[default]
    User,
    /// Claude's answer.
    Assistant,
}

/// Which list a bookmark is shown under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Files,
    Folders,
    Messages,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Files => "Files",
            Group::Folders => "Folders",
            Group::Messages => "Messages",
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

impl Bookmark {
    fn new(label: String, kind: BookmarkKind) -> Self {
        Bookmark { id: BookmarkId::new(), label, kind, created: now() }
    }

    /// A file, at a line (from 1): labelled `name:line`.
    pub fn file(path: PathBuf, line: Option<u32>) -> Self {
        let label = match line {
            Some(l) => format!("{}:{l}", name_of(&path)),
            None => name_of(&path),
        };
        Self::new(label, BookmarkKind::File { path, line })
    }

    pub fn folder(path: PathBuf) -> Self {
        Self::new(name_of(&path), BookmarkKind::Folder { path })
    }

    /// A message of an AI tab's conversation, labelled by its first words.
    pub fn message(tab: TabId, session: Option<String>, uuid: Option<String>, role: MessageRole, text: &str) -> Self {
        let excerpt = excerpt(text);
        let label = first_words(&excerpt, 48);
        Self::new(label, BookmarkKind::AiMessage { tab, session, uuid: uuid.filter(|u| !u.is_empty()), role, excerpt })
    }

    pub fn group(&self) -> Group {
        match self.kind {
            BookmarkKind::File { .. } => Group::Files,
            BookmarkKind::Folder { .. } => Group::Folders,
            BookmarkKind::AiMessage { .. } => Group::Messages,
        }
    }

    /// What is shown dimmed beside the label: a path relative to `folder`
    /// (the bench's), or the message's excerpt.
    pub fn detail(&self, folder: &Path) -> String {
        let rel = |p: &Path| match p.strip_prefix(folder) {
            Ok(r) if !r.as_os_str().is_empty() && folder != Path::new("/") => r.display().to_string(),
            _ => home_relative(p),
        };
        match &self.kind {
            BookmarkKind::File { path, line: Some(l) } => format!("{}:{l}", rel(path)),
            BookmarkKind::File { path, line: None } => rel(path),
            BookmarkKind::Folder { path } => format!("{}/", rel(path).trim_end_matches('/')),
            BookmarkKind::AiMessage { role, excerpt, .. } => {
                let who = match role {
                    MessageRole::User => "You",
                    MessageRole::Assistant => "Claude",
                };
                format!("{who}: {excerpt}")
            }
        }
    }

    /// The same place as `other` (the same file and line, folder, or
    /// message), whatever their labels.
    pub fn same_place(&self, other: &Bookmark) -> bool {
        match (&self.kind, &other.kind) {
            (BookmarkKind::AiMessage { tab: a, session: sa, uuid: ua, excerpt: ea, .. }, BookmarkKind::AiMessage { tab: b, session: sb, uuid: ub, excerpt: eb, .. }) => {
                a == b && sa == sb && if ua.is_some() || ub.is_some() { ua == ub } else { ea == eb }
            }
            (a, b) => a == b,
        }
    }
}

/// `~/…` for paths under the home folder.
fn home_relative(p: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rel) if rel.as_os_str().is_empty() => "~".into(),
        Some(rel) => format!("~/{}", rel.display()),
        None => p.display().to_string(),
    }
}

/// A message's first `EXCERPT` characters, on one line (runs of white
/// space as one space), with `…` when cut.
pub fn excerpt(text: &str) -> String {
    let flat = one_line(text);
    if flat.chars().count() <= EXCERPT {
        return flat;
    }
    let mut out: String = flat.chars().take(EXCERPT).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

/// Text on one line: runs of white space as one space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Up to `max` characters of `text`, cut at a word, with `…` when cut.
fn first_words(text: &str, max: usize) -> String {
    let text = text.trim_end_matches('…');
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let cut = match cut.rfind(' ') {
        Some(at) if at > max / 2 => &cut[..at],
        _ => &cut[..],
    };
    format!("{}…", cut.trim_end())
}

/// Whether a message's text is the one an excerpt was taken from: it starts
/// with it (or, cut otherwise, holds it).
pub fn matches_excerpt(text: &str, excerpt: &str) -> bool {
    let wanted = excerpt.trim_end_matches('…').trim();
    if wanted.is_empty() {
        return false;
    }
    let flat = one_line(text);
    flat.starts_with(wanted) || flat.contains(wanted)
}

/// `bookmarks` grouped as listed: files, folders, messages, each in the
/// order they were made.
pub fn grouped(bookmarks: &[Bookmark]) -> Vec<(Group, Vec<&Bookmark>)> {
    let mut out: Vec<(Group, Vec<&Bookmark>)> = Vec::new();
    for group in [Group::Files, Group::Folders, Group::Messages] {
        let these: Vec<&Bookmark> = bookmarks.iter().filter(|b| b.group() == group).collect();
        if !these.is_empty() {
            out.push((group, these));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_details() {
        let f = Bookmark::file("/work/api/src/main.rs".into(), Some(42));
        assert_eq!(f.label, "main.rs:42");
        assert_eq!(f.detail(Path::new("/work/api")), "src/main.rs:42");
        assert_eq!(f.detail(Path::new("/elsewhere")), "/work/api/src/main.rs:42");
        assert_eq!(f.group(), Group::Files);
        let d = Bookmark::folder("/work/api/docs".into());
        assert_eq!((d.label.as_str(), d.detail(Path::new("/work/api")).as_str()), ("docs", "docs/"));
        let long = "Please look at the login flow:\n\n  the session cookie is not renewed when the user comes back after a week away, and the refresh token is lost too.";
        let m = Bookmark::message(TabId::new(), Some("s".into()), Some(String::new()), MessageRole::User, long);
        let BookmarkKind::AiMessage { uuid, excerpt, .. } = &m.kind else { panic!() };
        assert_eq!(*uuid, None, "an empty id is none");
        assert!(excerpt.ends_with('…') && excerpt.chars().count() <= EXCERPT + 1 && !excerpt.contains('\n'));
        assert_eq!(m.label, "Please look at the login flow: the session…");
        assert!(m.detail(Path::new("/")).starts_with("You: Please look"));
        assert!(matches_excerpt(long, excerpt));
        assert!(!matches_excerpt("Something else", excerpt));
    }

    #[test]
    fn grouped_by_kind_in_order() {
        let tab = TabId::new();
        let all = vec![
            Bookmark::message(tab, None, Some("u1".into()), MessageRole::Assistant, "Done."),
            Bookmark::file("/a/b.rs".into(), None),
            Bookmark::folder("/a".into()),
            Bookmark::file("/a/c.rs".into(), Some(3)),
        ];
        let g = grouped(&all);
        let shape: Vec<(Group, Vec<&str>)> = g.iter().map(|(g, b)| (*g, b.iter().map(|b| b.label.as_str()).collect())).collect();
        assert_eq!(shape, [(Group::Files, vec!["b.rs", "c.rs:3"]), (Group::Folders, vec!["a"]), (Group::Messages, vec!["Done."])]);
        assert!(all[1].same_place(&Bookmark::file("/a/b.rs".into(), None)));
        assert!(!all[3].same_place(&Bookmark::file("/a/c.rs".into(), Some(4))));
        assert!(all[0].same_place(&Bookmark::message(tab, None, Some("u1".into()), MessageRole::Assistant, "Other words")));
    }
}

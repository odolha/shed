//! The sessions list as shown: a row per conversation (the session files
//! that go on from one another), its files under it when opened, and the
//! draft sessions first; all filtered by words.

use std::collections::HashSet;

use shed_model::DraftSession;

use crate::SessionInfo;

/// A row of the sessions list.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionRow {
    /// A conversation, shown by its most recent file; `root` is its first
    /// file's id, `parts` how many files it has, `open` whether they are
    /// listed under it.
    Conversation { tip: SessionInfo, root: String, parts: usize, open: bool },
    /// One of its files, listed under it.
    Part(SessionInfo),
    /// Drafts written in a new session, nothing sent yet.
    Draft(DraftSession),
}

/// The rows for `sessions` (as `list_sessions` gives them: each family's
/// first file at depth 0, the rest after it) and `drafts`
/// (`AiTab::draft_sessions`). A family is listed with its files when its
/// root is in `open` (and it has more than one). Every word of `filter`
/// must be in a title (any file's, for a family), case aside.
pub fn session_rows(sessions: &[SessionInfo], drafts: Vec<DraftSession>, open: &HashSet<String>, filter: &str) -> Vec<SessionRow> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let mut rows: Vec<SessionRow> = drafts.into_iter().filter(|d| words.iter().all(|w| d.title.to_lowercase().contains(w))).map(SessionRow::Draft).collect();
    let mut family: Vec<SessionInfo> = Vec::new();
    let flush = |family: &mut Vec<SessionInfo>, rows: &mut Vec<SessionRow>| {
        let Some(root) = family.first() else { return };
        let root = root.id.clone();
        // The conversation goes on in its most recent file.
        let Some(tip) = family.iter().max_by_key(|s| s.modified).cloned() else { return };
        if !words.iter().all(|w| family.iter().any(|s| s.title.to_lowercase().contains(w))) {
            family.clear();
            return;
        }
        let opened = family.len() > 1 && open.contains(&root);
        let parts = std::mem::take(family);
        rows.push(SessionRow::Conversation { tip, root, parts: parts.len(), open: opened });
        if opened {
            rows.extend(parts.into_iter().map(SessionRow::Part));
        }
    };
    for s in sessions {
        if s.depth == 0 {
            flush(&mut family, &mut rows);
        }
        family.push(s.clone());
    }
    flush(&mut family, &mut rows);
    rows
}

/// The first file of the conversation a session file belongs to.
pub fn family_of<'a>(sessions: &'a [SessionInfo], id: &str) -> Option<&'a str> {
    let mut root = None;
    for s in sessions {
        if s.depth == 0 {
            root = Some(s.id.as_str());
        }
        if s.id == id {
            return root;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str, modified: u64, depth: usize) -> SessionInfo {
        SessionInfo { id: id.into(), title: title.into(), modified, depth, ..Default::default() }
    }

    #[test]
    fn families_drafts_and_filters() {
        // Two conversations: one of three files (the newest is the second), one of one.
        let sessions = vec![session("a", "Fix the build", 10, 0), session("a2", "Fix the build, more", 30, 1), session("a3", "Fix tests", 20, 2), session("b", "Write docs", 15, 0)];
        let drafts = vec![DraftSession { key: "draft-1".into(), title: "Plan the docs".into(), drafts: 2 }];
        let tips = |rows: &[SessionRow]| -> Vec<String> {
            rows.iter()
                .map(|r| match r {
                    SessionRow::Conversation { tip, root, parts, open } => format!("{root}:{}:{parts}:{open}", tip.id),
                    SessionRow::Part(s) => format!("part {}", s.id),
                    SessionRow::Draft(d) => format!("draft {}", d.key),
                })
                .collect()
        };
        let none = HashSet::new();
        assert_eq!(tips(&session_rows(&sessions, drafts.clone(), &none, "")), ["draft draft-1", "a:a2:3:false", "b:b:1:false"]);
        // Opened: its files under it; a one-file conversation does not open.
        let open: HashSet<String> = ["a".to_string(), "b".to_string()].into();
        assert_eq!(tips(&session_rows(&sessions, drafts.clone(), &open, "")), ["draft draft-1", "a:a2:3:true", "part a", "part a2", "part a3", "b:b:1:false"]);
        // Words: every one, in any file's title, case aside.
        assert_eq!(tips(&session_rows(&sessions, drafts.clone(), &none, "TESTS fix")), ["a:a2:3:false"]);
        assert_eq!(tips(&session_rows(&sessions, drafts.clone(), &none, "docs")), ["draft draft-1", "b:b:1:false"]);
        assert!(session_rows(&sessions, drafts, &none, "nothing").is_empty());
        assert_eq!(family_of(&sessions, "a3"), Some("a"));
        assert_eq!(family_of(&sessions, "b"), Some("b"));
        assert_eq!(family_of(&sessions, "c"), None);
    }
}

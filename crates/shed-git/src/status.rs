//! `git status --porcelain=v2 --branch -z`, read.

/// Where the working folder stands against its branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branch {
    /// The branch checked out; `None` when detached.
    pub head: Option<String>,
    /// The commit checked out, short.
    pub oid: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// A changed file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the repository's root.
    pub path: String,
    /// Where a renamed file came from.
    pub orig: Option<String>,
    /// The index's state against HEAD: `.` unchanged, `M`, `A`, `D`, `R`, `C`.
    pub index: char,
    /// The working folder's state against the index.
    pub worktree: char,
    pub untracked: bool,
    pub conflict: bool,
}

impl Entry {
    pub fn staged(&self) -> bool {
        !self.untracked && !self.conflict && self.index != '.'
    }

    pub fn unstaged(&self) -> bool {
        self.untracked || (!self.conflict && self.worktree != '.')
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: Branch,
    pub entries: Vec<Entry>,
    /// A merge, rebase or the like under way.
    pub operation: Option<String>,
}

impl Status {
    pub fn staged(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.staged())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.unstaged())
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.conflict)
    }
}

pub fn parse_status(out: &str) -> Status {
    let mut status = Status::default();
    let mut records = out.split('\0').filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        if let Some(header) = rec.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => status.branch.oid = value.chars().take(8).collect(),
                "branch.head" if value != "(detached)" => status.branch.head = Some(value.to_string()),
                "branch.upstream" => status.branch.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            status.branch.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.branch.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let fields: Vec<&str> = rec.splitn(11, ' ').collect();
        let xy = |s: &str| {
            let mut c = s.chars();
            (c.next().unwrap_or('.'), c.next().unwrap_or('.'))
        };
        match fields.first().copied() {
            Some("1") if fields.len() >= 9 => {
                let (index, worktree) = xy(fields[1]);
                let path = rec.splitn(9, ' ').nth(8).unwrap_or("").to_string();
                status.entries.push(Entry { path, index, worktree, ..Default::default() });
            }
            Some("2") if fields.len() >= 10 => {
                let (index, worktree) = xy(fields[1]);
                let path = rec.splitn(10, ' ').nth(9).unwrap_or("").to_string();
                // The original path is the next record.
                let orig = records.next().map(String::from);
                status.entries.push(Entry { path, orig, index, worktree, ..Default::default() });
            }
            Some("u") if fields.len() >= 11 => {
                let (index, worktree) = xy(fields[1]);
                let path = rec.splitn(11, ' ').nth(10).unwrap_or("").to_string();
                status.entries.push(Entry { path, index, worktree, conflict: true, ..Default::default() });
            }
            Some("?") => {
                status.entries.push(Entry { path: rec[2..].to_string(), index: '?', worktree: '?', untracked: true, ..Default::default() });
            }
            _ => {}
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_kind() {
        let out = "# branch.oid 4f940577dde93214435c0b28dd6a018723c09dc5\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -3\0\
1 .M N... 100644 100644 100644 422c 422c src/a file.rs\0\
2 R. N... 100644 100644 100644 1111 1111 R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.txt\0\
? notes.md\0";
        let s = parse_status(out);
        assert_eq!(s.branch, Branch { head: Some("main".into()), oid: "4f940577".into(), upstream: Some("origin/main".into()), ahead: 2, behind: 3 });
        assert_eq!(s.entries.len(), 4);
        assert_eq!(s.entries[0].path, "src/a file.rs");
        assert!(s.entries[0].unstaged() && !s.entries[0].staged());
        assert_eq!((s.entries[1].path.as_str(), s.entries[1].orig.as_deref()), ("new.rs", Some("old.rs")));
        assert!(s.entries[1].staged());
        assert!(s.entries[2].conflict);
        assert!(s.entries[3].untracked && s.entries[3].unstaged());
        assert_eq!(s.staged().count(), 1);
        assert_eq!(s.conflicts().count(), 1);
    }
}

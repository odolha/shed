//! An AI tab of another workspace: what the view reads from that
//! workspace through the nexus instead of this PC's disk. Claude Code's
//! sessions (listed, renamed, deleted there), the pictures of the
//! conversation, files sent there to attach, and the review (as data,
//! converted to what the review draws).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AppContext, Context};
use shed_api::calls::{self as api, Call, Reply};
use shed_api::{ApiError, ErrorCode};
use shed_client::nexus_api::{RemoteWorkspace, unexpected};
use shed_review::{Change, CommitInfo, Found, Layer, RepoReview};

use super::AiView;
use crate::workspace_api::{decode, encode, says, tab};

/// What the review shows of each change of another workspace, by its place
/// in `Found::changes`: kept there (the Viewed marks and explanations are
/// that workspace's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    pub keys: Vec<String>,
    pub viewed: Vec<bool>,
    pub explanations: Vec<Option<String>>,
}

/// A session of another workspace, as the sessions list takes it (the
/// workspace lists it as its own GUI does).
pub fn session_of(s: api::AiSession) -> shed_ai::SessionInfo {
    shed_ai::SessionInfo {
        id: s.id,
        title: s.title,
        modified: s.modified as u64,
        branch: s.branch,
        prompts: s.prompts as usize,
        parent: s.parent,
        from: 0,
        branched: false,
        depth: s.depth as usize,
        whole: s.prompts as usize,
    }
}

/// Claude Code's sessions from a `Sessions` reply: draft sessions are the
/// tab's own (from the tree), so left out.
pub fn sessions_of(list: Vec<api::AiSession>) -> Vec<shed_ai::SessionInfo> {
    list.into_iter().filter(|s| !s.draft).map(session_of).collect()
}

fn picture_of(path: &Path, p: &api::Picture) -> Option<shed_project::picture::PictureData> {
    shed_project::picture::PictureData::from_bytes(path, decode(&p.data)?)
}

/// Another workspace's review as the review view draws it, and its marks.
pub fn found_of(review: api::Review) -> (Found, Marks) {
    let mut found = Found::default();
    let mut marks = Marks::default();
    for (ri, repo) in review.repos.into_iter().enumerate() {
        let root = PathBuf::from(&repo.root);
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| repo.root.clone());
        found.repos.push(RepoReview {
            repo: shed_git::Repo { root },
            name,
            // Its commits are listed as the workspace read them.
            base: Some(String::new()),
            commits: repo.commits.iter().map(|c| CommitInfo { hash: c.hash.clone(), short: c.short.clone(), subject: c.subject.clone(), pushed: false }).collect(),
        });
        for c in repo.changes {
            let layer = match c.layer.as_str() {
                "uncommitted" => Layer::Unstaged { untracked: c.status == "?" },
                "staged" => Layer::Staged,
                hash => Layer::Committed { hash: hash.to_string(), pushed: false },
            };
            let at = Path::new(&c.path);
            let pictures = (c.diff.old_picture.is_some() || c.diff.new_picture.is_some())
                .then(|| (c.diff.old_picture.as_ref().and_then(|p| picture_of(at, p)), c.diff.new_picture.as_ref().and_then(|p| picture_of(at, p))));
            found.changes.push(Change {
                repo: ri,
                layer,
                code: if c.status == "?" { 'A' } else { c.status.chars().next().unwrap_or('M') },
                hunks: crate::workspace_api::hunks_of(&c.diff.hunks),
                hash: String::new(),
                pictures,
                path: c.path,
            });
            marks.keys.push(c.key);
            marks.viewed.push(c.viewed);
            marks.explanations.push(c.explanation);
        }
    }
    (found, marks)
}

/// Opens a picture fetched from another workspace, from a copy in the
/// temporary folder.
pub fn open_picture(image: &gpui::Image, cx: &mut gpui::App) {
    let ext = match image.format {
        gpui::ImageFormat::Jpeg => "jpg",
        gpui::ImageFormat::Gif => "gif",
        gpui::ImageFormat::Webp => "webp",
        gpui::ImageFormat::Svg => "svg",
        _ => "png",
    };
    let dir = std::env::temp_dir().join(format!("shed-pictures-{}", std::process::id()));
    let path = dir.join(format!("{:016x}.{ext}", image.id));
    if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &image.bytes)).is_ok() {
        cx.open_url(&format!("file://{}", path.display()));
    }
}

/// The largest file sent to another workspace to attach.
const MAX_UPLOAD: u64 = api::MAX_UPLOAD as u64;

impl AiView {
    /// Lists the folder's sessions on the workspace.
    pub(super) fn load_remote_sessions(&mut self, workspace: RemoteWorkspace, cx: &mut Context<Self>) {
        let answer = workspace.call(Call::AiSessions(api::TabOnly { tab: tab(self.tab) }));
        self.sessions_task = Some(cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                match answer {
                    Ok(Reply::Sessions(list)) => this.sessions = Arc::new(sessions_of(list)),
                    Ok(other) => this.delete_problem = Some(says(&unexpected(&other))),
                    Err(e) => this.delete_problem = Some(format!("Cannot list the sessions: {}", says(&e))),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn rename_remote_session(&mut self, workspace: RemoteWorkspace, id: String, name: String, cx: &mut Context<Self>) {
        let answer = workspace.call(Call::AiRenameSession(api::AiRenameSession { tab: tab(self.tab), session: id, name }));
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                if let Err(e) = answer {
                    this.delete_problem = Some(format!("Cannot rename the session: {}", says(&e)));
                }
                this.load_sessions(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Deletes a session there; the list again once it is answered.
    pub(super) fn delete_remote_session(&mut self, workspace: RemoteWorkspace, id: String, cx: &mut Context<Self>) {
        let answer = workspace.call(Call::AiDeleteSession(api::AiDeleteSession { tab: tab(self.tab), session: id.clone() }));
        let task = cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                this.deleting = None;
                if let Err(e) = answer {
                    this.delete_problem = Some(says(&e));
                }
                this.load_sessions(cx);
            })
            .ok();
        });
        self.deleting = Some((id, task));
        cx.notify();
    }

    /// Sends a file to the workspace to attach: its path there goes with
    /// the prompt (this PC's paths mean nothing there).
    pub(super) fn upload_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(workspace) = self.remote.clone() else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => {
                self.attach_problem = Some(format!("{name} is a folder: another workspace takes files only."));
                return;
            }
            Ok(m) if m.len() > MAX_UPLOAD => {
                self.attach_problem = Some(format!("{name} is larger than {} MB, too large to send to the workspace.", MAX_UPLOAD / 1024 / 1024));
                return;
            }
            Ok(_) => {}
            Err(e) => {
                self.attach_problem = Some(format!("Cannot read {name}: {e}"));
                return;
            }
        }
        self.uploading += 1;
        let tab = tab(self.tab);
        cx.spawn(async move |this, cx| {
            let data = cx.background_spawn(async move { std::fs::read(&path).map(|b| encode(&b)) }).await;
            let answer = match data {
                Ok(data) => workspace.call(Call::AiUpload(api::AiUpload { tab, name: name.clone(), data })).await,
                Err(e) => Err(ApiError::new(ErrorCode::Failed, e.to_string())),
            };
            this.update(cx, |this, cx| {
                this.uploading = this.uploading.saturating_sub(1);
                match answer {
                    Ok(Reply::Text(there)) => {
                        let there = PathBuf::from(there);
                        if !this.files.contains(&there) {
                            this.files.push(there);
                        }
                    }
                    Ok(other) => this.attach_problem = Some(says(&unexpected(&other))),
                    Err(e) => this.attach_problem = Some(format!("Could not send {name} to the workspace: {}", says(&e))),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// For another workspace, pictures in the prompt go there as files
    /// before a draft keeps them (by their paths there); the draft is saved
    /// once they are. Whether it waits for that.
    pub(super) fn pictures_to_upload(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(workspace) = self.remote.clone() else { return false };
        if self.images.is_empty() {
            return false;
        }
        let images = std::mem::take(&mut self.images);
        let previews = std::mem::take(&mut self.previews);
        self.uploading += images.len();
        let tab = tab(self.tab);
        cx.spawn(async move |this, cx| {
            let mut paths = Vec::new();
            let mut failed = None;
            for (n, image) in images.iter().enumerate() {
                let ext = image.media_type.rsplit('/').next().unwrap_or("png").replace("jpeg", "jpg");
                let data = image.data.clone();
                let data = cx.background_spawn(async move { encode(&data) }).await;
                let call = Call::AiUpload(api::AiUpload { tab: tab.clone(), name: format!("picture-{}.{ext}", n + 1), data });
                match workspace.call(call).await {
                    Ok(Reply::Text(there)) => paths.push(PathBuf::from(there)),
                    Ok(other) => failed = Some(unexpected(&other)),
                    Err(e) => failed = Some(e),
                }
                if failed.is_some() {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                this.uploading = this.uploading.saturating_sub(images.len());
                match failed {
                    // Kept in the prompt, to try again.
                    Some(e) => {
                        this.images.splice(0..0, images);
                        this.previews.splice(0..0, previews);
                        this.attach_problem = Some(format!("Could not send the pictures to the workspace: {}", says(&e)));
                    }
                    None => {
                        this.files.extend(paths);
                        this.save_draft(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
        true
    }

    /// Another workspace's pictures (paths there), fetched as they are first
    /// shown; `None` for this PC's, which are read from disk.
    pub(super) fn remote_shown(&mut self, pictures: &[String], cx: &mut Context<Self>) -> Option<Vec<Option<Arc<gpui::Image>>>> {
        let workspace = self.remote.clone()?;
        for path in pictures {
            if self.remote_pictures.contains_key(path) {
                continue;
            }
            self.remote_pictures.insert(path.clone(), None);
            let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let answer = workspace.call(Call::AiPicture(api::AiPictureName { tab: tab(self.tab), name }));
            let path = path.clone();
            cx.spawn(async move |this, cx| {
                let Ok(Reply::Picture(p)) = answer.await else { return };
                let Some(bytes) = decode(&p.data) else { return };
                let format = gpui::ImageFormat::from_mime_type(&p.media_type).unwrap_or(gpui::ImageFormat::Png);
                this.update(cx, |this, cx| {
                    this.remote_pictures.insert(path, Some(Arc::new(gpui::Image::from_bytes(format, bytes))));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Some(pictures.iter().map(|p| self.remote_pictures.get(p).cloned().flatten()).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::theme::UiColors;
    use shed_model::{BenchId, Rgb, TabId};

    #[gpui::test]
    fn another_workspaces_tab_reads_its_sessions_review_and_takes_files_there(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let calls: Arc<Mutex<Vec<Call>>> = Arc::default();
        let seen = calls.clone();
        let api = shed_client::nexus_api::NexusApi::fake(Arc::new(move |_, call| {
            seen.lock().unwrap().push(call.clone());
            match call {
                Call::AiSessions(_) => Ok(Reply::Sessions(vec![session("draft-1", true), session("s1", false)])),
                Call::AiUpload(u) => Ok(Reply::Text(format!("/remote/up/{}", u.name))),
                Call::AiReview(_) => Ok(Reply::Review(api::Review {
                    repos: vec![api::ReviewRepo {
                        root: "/srv/app".into(),
                        commits: Vec::new(),
                        changes: vec![api::ReviewChange {
                            key: "/srv/app|worktree|a.rs".into(),
                            path: "a.rs".into(),
                            layer: "uncommitted".into(),
                            status: "M".into(),
                            viewed: false,
                            explanation: None,
                            diff: Default::default(),
                        }],
                    }],
                })),
                Call::AiSetViewed(_) | Call::AiRenameSession(_) => Ok(Reply::Done),
                _ => Err(ApiError::new(ErrorCode::Unsupported, "no")),
            }
        }));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let ai = shed_model::AiTab { folder: "/srv/app".into(), session: Some("s1".into()), ..Default::default() };
            let mut v = AiView::new(BenchId::new(), TabId::new(), ai, Rgb::hex(0xFF8A57), shed_client::keeper_conn::KeeperConn::detached(), crate::settings::Settings::default(), cx);
            v.set_remote(api.workspace("w1"), cx);
            v
        });
        cx.run_until_parked();
        view.read_with(cx, |v, _| assert_eq!(v.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s1"]));

        // A file attached goes to the workspace; its path there goes with the prompt.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "hello").unwrap();
        view.update(cx, |v, cx| v.attach_paths(std::slice::from_ref(&file), cx));
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert_eq!(v.files, [PathBuf::from("/remote/up/notes.txt")]);
            assert_eq!(v.uploading, 0);
        });
        // A folder cannot go: said so.
        view.update(cx, |v, cx| v.attach_paths(&[dir.path().to_path_buf()], cx));
        view.read_with(cx, |v, _| assert!(v.attach_problem.as_deref().is_some_and(|p| p.contains("folder"))));

        // A picture kept in a draft is sent there first, then named by its path there.
        view.update(cx, |v, cx| {
            v.files.clear();
            v.add_picture(shed_proto::AiImage { media_type: "image/png".into(), data: vec![1, 2, 3] });
            v.save_draft(cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            let draft = v.ai.drafts.last().expect("a draft");
            assert!(draft.text.contains("/remote/up/picture-1.png"), "{}", draft.text);
            assert!(v.images.is_empty());
        });

        // The review: the whole session's, from there; Viewed marked there.
        cx.update(|window, cx| view.update(cx, |v, cx| v.open_review(super::super::review::Scope::Turn, window, cx)));
        cx.run_until_parked();
        view.update(cx, |v, cx| {
            let r = v.review.as_ref().unwrap();
            assert_eq!(r.scope, super::super::review::Scope::Session);
            assert_eq!(r.found.changes.len(), 1);
            assert_eq!(r.marks.as_ref().unwrap().viewed, [false]);
            v.toggle_viewed(0, cx);
        });
        cx.run_until_parked();
        view.update(cx, |v, _| {
            assert_eq!(v.review.as_ref().unwrap().marks.as_ref().unwrap().viewed, [true]);
        });
        let calls = calls.lock().unwrap();
        assert!(calls.iter().any(|c| matches!(c, Call::AiSetViewed(s) if s.viewed && s.key == "/srv/app|worktree|a.rs")), "{calls:?}");
    }

    fn session(id: &str, draft: bool) -> api::AiSession {
        api::AiSession { id: id.into(), title: format!("about {id}"), modified: 1_700_000_000, branch: Some("main".into()), prompts: 3, parent: None, depth: 0, draft, drafts: 0 }
    }

    #[test]
    fn sessions_from_the_workspace_leave_drafts_to_the_tab() {
        let list = sessions_of(vec![session("draft-1", true), session("s1", false)]);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].id.as_str(), list[0].prompts, list[0].modified, list[0].whole), ("s1", 3, 1_700_000_000, 3));
    }

    #[test]
    fn a_review_reads_as_the_view_draws_it() {
        let hunk = api::Hunk {
            header: "@@ -3,2 +3,3 @@".into(),
            lines: vec![
                api::DiffLine { kind: api::DiffLineKind::Context, text: "a".into(), old: Some(3), new: Some(3) },
                api::DiffLine { kind: api::DiffLineKind::Added, text: "b".into(), old: None, new: Some(4) },
            ],
        };
        let review = api::Review {
            repos: vec![api::ReviewRepo {
                root: "/srv/app".into(),
                commits: vec![api::Commit { hash: "abc123".into(), short: "abc".into(), subject: "Fix".into(), author: String::new(), time: 0 }],
                changes: vec![
                    api::ReviewChange {
                        key: "/srv/app|worktree|src/a.rs".into(),
                        path: "src/a.rs".into(),
                        layer: "uncommitted".into(),
                        status: "?".into(),
                        viewed: true,
                        explanation: Some("Adds b.".into()),
                        diff: api::Diff { hunks: vec![hunk], ..Default::default() },
                    },
                    api::ReviewChange { key: "/srv/app|commit abc123|b.rs".into(), path: "b.rs".into(), layer: "abc123".into(), status: "M".into(), viewed: false, explanation: None, diff: Default::default() },
                ],
            }],
        };
        let (found, marks) = found_of(review);
        assert_eq!(found.repos[0].name, "app");
        assert_eq!(found.changes[0].layer, Layer::Unstaged { untracked: true });
        assert_eq!(found.changes[1].layer, Layer::Committed { hash: "abc123".into(), pushed: false });
        // The keys this side makes are the workspace's.
        assert_eq!(found.key(&found.changes[0]), marks.keys[0]);
        assert_eq!(found.key(&found.changes[1]), marks.keys[1]);
        let h = &found.changes[0].hunks[0];
        assert_eq!((h.old_start, h.old_len, h.new_start, h.new_len, h.lines.len()), (3, 2, 3, 3, 2));
        assert_eq!(h.header(), "@@ -3,2 +3,3 @@");
        assert_eq!(marks.viewed, [true, false]);
        assert_eq!(marks.explanations[0].as_deref(), Some("Adds b."));
    }
}

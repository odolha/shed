//! AI tabs' calls: most are the keeper's messages; sessions and pictures
//! are read here.

use std::collections::HashSet;

use base64::Engine;
use shed_api::calls::{self as api, Call, Reply};
use shed_api::{ApiError, ErrorCode};
use shed_model::{Shed, TabId};
use shed_proto::{AiChange, AiImage, ClientMsg};

use super::{Ctx, invalid, not_found, open_tab};
use crate::keeper;

/// The AI tab a call names, and its settings.
fn ai_tab(tree: &Shed, tab: &str) -> Result<(TabId, shed_model::AiTab), ApiError> {
    let (id, t) = open_tab(tree, tab)?;
    match t.kind {
        shed_model::TabKind::Ai(a) => Ok((id, a)),
        _ => Err(invalid("not an AI tab")),
    }
}

/// Pictures from a connector, as the keeper takes them.
fn images(pictures: Vec<api::Picture>) -> Result<Vec<AiImage>, ApiError> {
    pictures
        .into_iter()
        .map(|p| {
            if !p.media_type.starts_with("image/") {
                return Err(invalid(format!("{} is not a picture", p.media_type)));
            }
            let data = base64::engine::general_purpose::STANDARD.decode(p.data.as_bytes()).map_err(|_| invalid("a picture is not base64"))?;
            Ok(AiImage { media_type: p.media_type, data })
        })
        .collect()
}

/// What Claude changed in the session's repositories, read as the AI
/// tab's review reads it (the whole session).
async fn review(ctx: &Ctx, tab: TabId, ai: &shed_model::AiTab) -> Result<shed_review::Found, ApiError> {
    let entries = ctx.ai_entries(tab).await?;
    let dirs = shed_review::dirs(ai);
    tokio::task::spawn_blocking(move || shed_review::read(dirs, shed_review::review_since(&entries, shed_review::Scope::Session)))
        .await
        .map_err(|e| keeper::failed(e.to_string()))
}

fn review_reply(found: &shed_review::Found, ai: &shed_model::AiTab) -> api::Review {
    let repos = found
        .repos
        .iter()
        .enumerate()
        .map(|(i, r)| api::ReviewRepo {
            root: r.repo.root.to_string_lossy().into_owned(),
            commits: r.commits.iter().map(|c| api::Commit { hash: c.hash.clone(), short: c.short.clone(), subject: c.subject.clone(), author: String::new(), time: 0 }).collect(),
            changes: found
                .changes
                .iter()
                .filter(|c| c.repo == i)
                .map(|c| api::ReviewChange {
                    key: found.key(c),
                    path: c.path.clone(),
                    layer: match &c.layer {
                        shed_review::Layer::Unstaged { .. } => "uncommitted".into(),
                        shed_review::Layer::Staged => "staged".into(),
                        shed_review::Layer::Committed { hash, .. } => hash.clone(),
                    },
                    status: match &c.layer {
                        shed_review::Layer::Unstaged { untracked: true } => "?".into(),
                        _ => c.code.to_string(),
                    },
                    viewed: found.is_viewed(c, &ai.viewed),
                    explanation: found.explanation(c, &ai.explained).map(|(text, _)| text),
                    diff: match &c.pictures {
                        Some((old, new)) => api::Diff {
                            old_picture: old.as_ref().map(picture_of),
                            new_picture: new.as_ref().map(picture_of),
                            ..Default::default()
                        },
                        None => api::Diff { hunks: super::git::shown(&c.hunks), ..Default::default() },
                    },
                })
                .collect(),
        })
        .collect();
    api::Review { repos }
}

pub(super) async fn call(ctx: &Ctx, tree: &Shed, call: Call) -> Result<Reply, ApiError> {
    let quiet = |msg| ctx.quiet(msg);
    match call {
        Call::AiSessions(a) => {
            let (_, ai) = ai_tab(tree, &a.tab)?;
            let hidden = private_sessions(ctx, tree).await;
            let sessions = tokio::task::spawn_blocking(move || sessions(&ai, &hidden)).await.map_err(|e| keeper::failed(e.to_string()))?;
            Ok(Reply::Sessions(sessions))
        }
        Call::AiOpenSession(a) => {
            let (tab, _) = ai_tab(tree, &a.tab)?;
            if let Some(session) = &a.session
                && private_sessions(ctx, tree).await.contains(session)
            {
                return Err(super::forbidden_private());
            }
            quiet(ClientMsg::AiOpenSession { tab, session: a.session })
        }
        Call::AiRenameSession(a) => {
            let (_, ai) = ai_tab(tree, &a.tab)?;
            if private_sessions(ctx, tree).await.contains(&a.session) {
                return Err(super::forbidden_private());
            }
            if shed_model::AiTab::is_draft_key(&a.session) || a.session.contains(['/', '\\']) || a.session.starts_with('.') {
                return Err(invalid("only a Claude Code session can be renamed"));
            }
            if a.name.trim().is_empty() {
                return Err(invalid("a session needs a name"));
            }
            let folder = ai.folder.clone();
            tokio::task::spawn_blocking(move || shed_ai::rename_session(&folder, &a.session, &a.name))
                .await
                .map_err(|e| keeper::failed(e.to_string()))?
                .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { not_found("no such session") } else { keeper::failed(e.to_string()) })?;
            Ok(Reply::Done)
        }
        Call::AiSend(a) => {
            let (tab, _) = ai_tab(tree, &a.tab)?;
            let images = images(a.pictures)?;
            if a.text.trim().is_empty() && images.is_empty() {
                return Err(invalid("nothing to send"));
            }
            quiet(ClientMsg::AiSend { tab, text: a.text, images })
        }
        Call::AiStop(a) => quiet(ClientMsg::AiStop { tab: ai_tab(tree, &a.tab)?.0 }),
        Call::AiStart(a) => quiet(ClientMsg::AiStart { tab: ai_tab(tree, &a.tab)?.0 }),
        Call::AiAnswer(a) => quiet(ClientMsg::AiAnswer { tab: ai_tab(tree, &a.tab)?.0, request: a.request, allow: a.allow, always: a.always }),
        Call::AiAnswerQuestions(a) => quiet(ClientMsg::AiAnswerQuestions { tab: ai_tab(tree, &a.tab)?.0, request: a.request, answers: a.answers }),
        Call::AiEdit(a) => {
            let (tab, _) = ai_tab(tree, &a.tab)?;
            quiet(ClientMsg::AiEdit { tab, entry: a.entry, text: a.text, images: images(a.pictures)? })
        }
        Call::AiRevert(a) => quiet(ClientMsg::AiRevert { tab: ai_tab(tree, &a.tab)?.0, entry: a.entry, files: a.files }),
        Call::AiQueued(a) => quiet(ClientMsg::AiQueued { tab: ai_tab(tree, &a.tab)?.0, index: a.index, text: a.text, send: a.send }),
        Call::AiSettings(a) => {
            let (tab, _) = ai_tab(tree, &a.tab)?;
            let clearable = |v: Option<String>| v.map(|s| Some(s).filter(|s| !s.is_empty()));
            let add_dirs = match a.add_dirs {
                Some(dirs) => Some(dirs.iter().map(|d| crate::paths::expand(d)).collect::<Result<Vec<_>, _>>()?),
                None => None,
            };
            let change = AiChange::Settings {
                model: clearable(a.model),
                mode: a.mode.filter(|m| !m.is_empty()),
                effort: clearable(a.effort),
                follow: a.follow,
                browser: a.browser,
                add_dirs,
            };
            quiet(ClientMsg::AiChange { tab, change })
        }
        Call::AiSetUnsent(a) => quiet(ClientMsg::AiChange { tab: ai_tab(tree, &a.tab)?.0, change: AiChange::Unsent(a.text) }),
        Call::AiAddDraft(a) => {
            if a.text.trim().is_empty() {
                return Err(invalid("an empty draft"));
            }
            quiet(ClientMsg::AiChange { tab: ai_tab(tree, &a.tab)?.0, change: AiChange::AddDraft { session: a.session, text: a.text } })
        }
        Call::AiRemoveDraft(a) => {
            let (tab, ai) = ai_tab(tree, &a.tab)?;
            let draft = ai.drafts.get(a.index as usize).filter(|d| d.text == a.text).ok_or_else(|| not_found("that draft is not there (any more)"))?;
            quiet(ClientMsg::AiChange { tab, change: AiChange::RemoveDraft { index: a.index, session: draft.session.clone(), text: a.text } })
        }
        Call::AiPicture(a) => {
            ai_tab(tree, &a.tab)?;
            let name = crate::paths::plain_name(&a.name)?.to_string();
            let path = ctx.pictures.join(name);
            let picture = tokio::task::spawn_blocking(move || shed_project::picture::PictureData::read(&path))
                .await
                .map_err(|e| keeper::failed(e.to_string()))?
                .ok_or_else(|| not_found("no such picture"))?;
            Ok(Reply::Picture(picture_of(&picture)))
        }
        Call::AiReview(a) => {
            let (tab, ai) = ai_tab(tree, &a.tab)?;
            let found = review(ctx, tab, &ai).await?;
            Ok(Reply::Review(review_reply(&found, &ai)))
        }
        Call::AiSetViewed(a) => {
            let (tab, ai) = ai_tab(tree, &a.tab)?;
            let found = review(ctx, tab, &ai).await?;
            // Marked as it is now: a change that differs later is not viewed.
            let change = found.changes.iter().find(|c| found.key(c) == a.key).ok_or_else(|| not_found("no such change (any more)"))?;
            quiet(ClientMsg::AiChange { tab, change: AiChange::Viewed { key: a.key, hash: change.hash.clone(), viewed: a.viewed } })
        }
        Call::AiExplain(a) => {
            let (tab, ai) = ai_tab(tree, &a.tab)?;
            let session = ai.session.clone().ok_or_else(|| invalid("nothing to explain before the first prompt"))?;
            let found = review(ctx, tab, &ai).await?;
            let which: Vec<usize> = found.changes.iter().enumerate().filter(|(_, c)| a.keys.contains(&found.key(c))).map(|(i, _)| i).collect();
            if which.is_empty() {
                return Err(not_found("none of those changes are there (any more)"));
            }
            let request = found.explain_request(&which);
            let (folder, model) = (ai.folder.clone(), ai.model.clone());
            let kept = tokio::task::spawn_blocking(move || {
                let answer = request.ask(&folder, &session, model.as_deref())?;
                Ok::<_, String>(request.answered(answer))
            })
            .await
            .map_err(|e| keeper::failed(e.to_string()))?
            .map_err(keeper::failed)?;
            ctx.keeper.send_msg(ClientMsg::AiChange { tab, change: AiChange::Explained(kept.clone()) });
            Ok(Reply::Explained(kept.into_iter().map(|(key, _, text)| api::Explanation { key, text }).collect()))
        }
        Call::AiUpload(a) => {
            let (tab, _) = ai_tab(tree, &a.tab)?;
            let name = upload_name(&a.name);
            // Base64 is 4 bytes for 3: refused before it is decoded.
            if a.data.len() / 4 * 3 > api::MAX_UPLOAD as usize + 3 {
                return Err(invalid(format!("{name} is larger than {} MB", api::MAX_UPLOAD / 1024 / 1024)));
            }
            let data = base64::engine::general_purpose::STANDARD.decode(a.data.as_bytes()).map_err(|_| invalid("the file is not base64"))?;
            if data.len() > api::MAX_UPLOAD as usize {
                return Err(invalid(format!("{name} is larger than {} MB", api::MAX_UPLOAD / 1024 / 1024)));
            }
            let dir = ctx.uploads.join(tab.to_string());
            let path = tokio::task::spawn_blocking(move || keep_upload(&dir, &name, &data))
                .await
                .map_err(|e| keeper::failed(e.to_string()))?
                .map_err(|e| keeper::failed(format!("cannot keep the file: {e}")))?;
            Ok(Reply::Text(path.to_string_lossy().into_owned()))
        }
        _ => Err(ApiError::new(ErrorCode::Unsupported, "not an AI call")),
    }
}

/// An uploaded file's name made safe: its last part only, no control
/// characters, not hidden, not too long (the extension kept).
fn upload_name(name: &str) -> String {
    const MAX: usize = 120;
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim().trim_start_matches('.').trim();
    if clean.is_empty() {
        return "file".into();
    }
    if clean.len() <= MAX {
        return clean.to_string();
    }
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 16 => (s, format!(".{e}")),
        _ => (clean, String::new()),
    };
    let mut cut = MAX - ext.len();
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{ext}", &stem[..cut])
}

/// Keeps an uploaded file in `dir` (the tab's), in a folder named by its
/// contents: the same file again is the same path, and two files of one
/// name stay apart. Its path.
fn keep_upload(dir: &std::path::Path, name: &str, data: &[u8]) -> std::io::Result<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    let folder = dir.join(format!("{:016x}", h.finish()));
    let path = folder.join(name);
    if let Ok(f) = std::fs::File::options().append(true).open(&path) {
        // Sent again: kept longer (the keeper prunes by this time).
        f.set_modified(std::time::SystemTime::now())?;
        return Ok(path);
    }
    std::fs::create_dir_all(&folder)?;
    let part = folder.join(format!(".{name}.part"));
    std::fs::write(&part, data)?;
    std::fs::rename(&part, &path)?;
    Ok(path)
}

/// A picture as connectors get it.
pub(crate) fn picture_of(p: &shed_project::picture::PictureData) -> api::Picture {
    api::Picture { media_type: p.kind.media_type().into(), data: base64::engine::general_purpose::STANDARD.encode(&p.bytes) }
}

/// Sessions private AI tabs show or keep drafts for (Claude Code's, and
/// draft sessions), in these benches. Another AI tab in the same folder
/// lists the same Claude Code sessions: these are left out of its list, and
/// cannot be opened or renamed from it.
fn private_sessions_in<'a>(benches: impl Iterator<Item = &'a shed_model::Bench>) -> HashSet<String> {
    let mut hidden = HashSet::new();
    for tab in benches.flat_map(|b| &b.tabs).filter(|t| t.private) {
        if let shed_model::TabKind::Ai(ai) = &tab.kind {
            hidden.extend(ai.session.iter().chain(&ai.draft_session).cloned());
            hidden.extend(ai.drafts.iter().map(|d| d.session.clone()));
        }
    }
    hidden
}

/// `private_sessions_in` the running benches and the closed ones.
async fn private_sessions(ctx: &Ctx, tree: &Shed) -> HashSet<String> {
    let mut hidden = private_sessions_in(tree.benches.iter());
    if let Ok(pegboard) = ctx.pegboard().await {
        hidden.extend(private_sessions_in(pegboard.stored.iter().map(|s| &s.bench)));
    }
    hidden
}

/// The folder's sessions as the GUI lists them: draft sessions first (the
/// newest first), then Claude Code's, each with how many drafts it has;
/// those of private tabs (`hidden`) left out.
fn sessions(ai: &shed_model::AiTab, hidden: &HashSet<String>) -> Vec<api::AiSession> {
    let drafts_of = |key: &str| ai.drafts.iter().filter(|d| d.session == key).count() as u32;
    let drafts = ai.draft_sessions().into_iter().map(|d| api::AiSession {
        id: d.key.clone(),
        title: d.title,
        modified: 0,
        branch: None,
        prompts: 0,
        parent: None,
        depth: 0,
        draft: true,
        drafts: d.drafts as u32,
    });
    let found = shed_ai::list_sessions(&ai.folder).into_iter().map(|s| api::AiSession {
        drafts: drafts_of(&s.id),
        id: s.id,
        title: s.title,
        modified: s.modified.min(u32::MAX as u64) as u32,
        branch: s.branch,
        prompts: s.prompts as u32,
        parent: s.parent,
        depth: s.depth as u32,
        draft: false,
    });
    drafts.chain(found).filter(|s| !hidden.contains(&s.id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_tabs_sessions_are_hidden() {
        let mut public = shed_model::Tab::ai("/srv".into());
        let mut private = shed_model::Tab::ai("/srv".into());
        private.private = true;
        if let shed_model::TabKind::Ai(a) = &mut private.kind {
            a.session = Some("secret-session".into());
            a.drafts.push(shed_model::Draft { session: "draft-x".into(), text: "psst".into() });
            a.drafts.push(shed_model::Draft { session: "other-secret".into(), text: "later".into() });
        }
        if let shed_model::TabKind::Ai(a) = &mut public.kind {
            a.session = Some("open-session".into());
        }
        let bench = shed_model::Bench {
            id: shed_model::BenchId::new(),
            name: "b".into(),
            color: shed_model::BenchColor::Blue,
            badge: None,
            cwd: "/srv".into(),
            active_tab: public.id,
            tabs: vec![public, private],
            aliases: Default::default(),
            closing: false,
            launcher: "b".into(),
            window: None,
            started: false,
            links: Vec::new(),
        };
        let hidden = private_sessions_in(std::iter::once(&bench));
        assert!(hidden.contains("secret-session") && hidden.contains("draft-x") && hidden.contains("other-secret"));
        assert!(!hidden.contains("open-session"));
    }

    #[test]
    fn uploaded_names_are_made_safe() {
        assert_eq!(upload_name("report.pdf"), "report.pdf");
        assert_eq!(upload_name("../../etc/passwd"), "passwd");
        assert_eq!(upload_name("C:\\Users\\you\\notes.txt"), "notes.txt");
        assert_eq!(upload_name(".bashrc"), "bashrc");
        assert_eq!(upload_name(".."), "file");
        assert_eq!(upload_name("a\nb\u{0}.md"), "ab.md");
        assert_eq!(upload_name(""), "file");
        let long = format!("{}.tar.gz", "é".repeat(100));
        let safe = upload_name(&long);
        assert!(safe.len() <= 120 && safe.ends_with(".gz"), "{safe}");
    }

    #[test]
    fn an_upload_is_kept_once_and_apart_from_others_of_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let a = keep_upload(dir.path(), "notes.txt", b"one").unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        assert_eq!(keep_upload(dir.path(), "notes.txt", b"one").unwrap(), a);
        let b = keep_upload(dir.path(), "notes.txt", b"two").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(dir.path()) && b.ends_with("notes.txt"));
    }
}

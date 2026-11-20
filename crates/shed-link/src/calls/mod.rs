//! Calls from connectors: changes go to the keeper as the same messages the
//! GUI sends; reading files, Git and the logbook is done here, off the
//! link's own task. (Terminal input, sizes and scrollback need the link's
//! emulators: `run` answers those.)

mod ai;
mod git;
mod project;

pub(crate) use project::{root_of, slash, utf16_at};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shed_api::calls::{self as api, Call, Reply};
use shed_api::{ApiError, ErrorCode};
use shed_client::{BenchEdit, KeeperConn};
use shed_model::{BenchId, PaneId, Shed, Tab, TabId};
use shed_proto::{ClientMsg, PegboardInfo, SavedEdit, ShowTarget};
use tokio::sync::{mpsc, oneshot, watch};

use crate::keeper::{self, WAIT};

/// What calls work with.
pub(crate) struct Ctx {
    pub keeper: KeeperConn,
    pub socket: PathBuf,
    pub tree: watch::Receiver<Arc<Shed>>,
    /// Asks the link's task for things only it has.
    pub ask: mpsc::UnboundedSender<Ask>,
    pub logbook: PathBuf,
    pub pictures: PathBuf,
    /// Each project's files, as walked last, for quick open and search.
    files: Mutex<HashMap<PathBuf, Walked>>,
}

/// A project's files, and when they were walked.
type Walked = (Instant, Arc<Vec<PathBuf>>);

/// What a call asks of the link's task.
pub(crate) enum Ask {
    /// The keeper's pegboard (closed benches, previews), freshly asked for.
    Pegboard(oneshot::Sender<Arc<PegboardInfo>>),
    /// An AI tab's conversation, whole.
    AiEntries(TabId, oneshot::Sender<Vec<shed_ai::Entry>>),
}

/// A project's file list is walked again after this long (or when a watch
/// on it sees a change).
const FILES_KEPT: Duration = Duration::from_secs(30);

impl Ctx {
    pub fn new(keeper: KeeperConn, socket: PathBuf, tree: watch::Receiver<Arc<Shed>>, ask: mpsc::UnboundedSender<Ask>, logbook: PathBuf, pictures: PathBuf) -> Ctx {
        Ctx { keeper, socket, tree, ask, logbook, pictures, files: Mutex::new(HashMap::new()) }
    }

    pub fn tree(&self) -> Arc<Shed> {
        self.tree.borrow().clone()
    }

    async fn pegboard(&self) -> Result<Arc<PegboardInfo>, ApiError> {
        let (tx, rx) = oneshot::channel();
        let _ = self.ask.send(Ask::Pegboard(tx));
        match tokio::time::timeout(WAIT, rx).await {
            Ok(Ok(p)) => Ok(p),
            _ => Err(ApiError::new(ErrorCode::Timeout, "the keeper did not list the closed benches")),
        }
    }

    /// An AI tab's conversation as the keeper has it.
    async fn ai_entries(&self, tab: TabId) -> Result<Vec<shed_ai::Entry>, ApiError> {
        let (tx, rx) = oneshot::channel();
        let _ = self.ask.send(Ask::AiEntries(tab, tx));
        match tokio::time::timeout(WAIT, rx).await {
            Ok(Ok(e)) => Ok(e),
            _ => Err(ApiError::new(ErrorCode::Timeout, "the keeper did not give the conversation")),
        }
    }

    /// A project's files (relative to `root`), walked at most `FILES_KEPT` ago.
    pub fn files(&self, root: &std::path::Path, exclude: &[String]) -> Arc<Vec<PathBuf>> {
        if let Some((at, files)) = self.files.lock().unwrap().get(root)
            && at.elapsed() < FILES_KEPT
        {
            return files.clone();
        }
        let files = Arc::new(shed_project::walk(root, exclude).files);
        self.files.lock().unwrap().insert(root.to_path_buf(), (Instant::now(), files.clone()));
        files
    }

    /// Something changed in the project: walk it again next time.
    pub fn forget_files(&self, root: &std::path::Path) {
        self.files.lock().unwrap().remove(root);
    }

    /// Sends a change that the keeper answers only through the tree.
    fn quiet(&self, msg: ClientMsg) -> Result<Reply, ApiError> {
        self.keeper.send_msg(msg);
        Ok(Reply::Done)
    }

    /// Sends a change the keeper answers (`Done` or an error).
    async fn answered(&self, msg: ClientMsg) -> Result<Reply, ApiError> {
        keeper::ask(&self.socket, msg, WAIT).await.map(|_| Reply::Done)
    }

    /// An edit to a bench, running (through the tree) or closed (its file).
    async fn edit(&self, bench: &shed_proto::BenchRef, edit: BenchEdit) -> Result<Reply, ApiError> {
        let msg = edit.to_msg(bench);
        match bench {
            shed_proto::BenchRef::Running(_) => self.quiet(msg),
            shed_proto::BenchRef::Saved(_) => self.answered(msg).await,
        }
    }
}

pub(crate) fn not_found(what: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::NotFound, what)
}

pub(crate) fn invalid(what: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, what)
}

pub(crate) fn forbidden_private() -> ApiError {
    ApiError::new(ErrorCode::Forbidden, "the tab is private: what is in it stays on the workspace")
}

fn uuid(s: &str) -> Result<uuid::Uuid, ApiError> {
    uuid::Uuid::parse_str(s).map_err(|_| invalid(format!("{s:?} is not an id")))
}

/// Ids as the keeper's types, from the strings connectors send.
pub(crate) fn bench_id(s: &str) -> Result<BenchId, ApiError> {
    uuid(s).map(BenchId)
}

pub(crate) fn tab_id(s: &str) -> Result<TabId, ApiError> {
    uuid(s).map(TabId)
}

pub(crate) fn pane_id(s: &str) -> Result<PaneId, ApiError> {
    uuid(s).map(PaneId)
}

fn bench_ref(b: &api::BenchRef) -> Result<shed_proto::BenchRef, ApiError> {
    Ok(match b {
        api::BenchRef::Running(id) => shed_proto::BenchRef::Running(bench_id(id)?),
        api::BenchRef::Closed(name) => shed_proto::BenchRef::Saved(name.clone()),
    })
}

/// A tab of a running bench.
pub(crate) fn running_tab(tree: &Shed, tab: TabId) -> Result<&Tab, ApiError> {
    tree.tab(tab).ok_or_else(|| not_found("no such tab (in a running bench)"))
}

/// A running tab whose contents a call reads or changes: not a private one.
pub(crate) fn open_tab(tree: &Shed, tab: &str) -> Result<(TabId, Tab), ApiError> {
    let id = tab_id(tab)?;
    let t = running_tab(tree, id)?;
    if t.private {
        return Err(forbidden_private());
    }
    Ok((id, t.clone()))
}

/// The pane, and whether its tab is private.
pub(crate) fn pane_tab(tree: &Shed, pane: PaneId) -> Result<&Tab, ApiError> {
    let (_, tab) = tree.locate_pane(pane).ok_or_else(|| not_found("no such pane"))?;
    running_tab(tree, tab)
}

/// A folder a connector names for a new tab: `~` expanded, absolute.
fn folder(path: &str) -> Result<PathBuf, ApiError> {
    crate::paths::expand(path)
}

/// Answers a call (all but the few `run` answers itself).
pub(crate) async fn call(ctx: Arc<Ctx>, call: Call) -> Result<Reply, ApiError> {
    let tree = ctx.tree();
    match call {
        // ---- benches
        Call::NewBench(a) => {
            if a.name.as_ref().is_some_and(|n| n.trim().is_empty()) {
                return Err(invalid("a bench needs a name"));
            }
            let cwd = a.cwd.as_deref().map(folder).transpose()?;
            let before: Vec<BenchId> = tree.benches.iter().map(|b| b.id).collect();
            let id = keeper::make(&ctx.socket, &ctx.tree, ClientMsg::Show(ShowTarget::NewBench { name: a.name, cwd }), |t| {
                t.benches.iter().find(|b| !before.contains(&b.id)).map(|b| b.id)
            })
            .await?;
            Ok(Reply::Bench(id.to_string()))
        }
        Call::LoadBench(a) => {
            let target = if a.start { ShowTarget::LoadAndStart { name: a.name.clone() } } else { ShowTarget::LoadQuiet { name: a.name.clone() } };
            let id = keeper::make(&ctx.socket, &ctx.tree, ClientMsg::Show(target), |t| t.benches.iter().find(|b| b.name == a.name).map(|b| b.id)).await?;
            Ok(Reply::Bench(id.to_string()))
        }
        Call::CloseBench(a) => ctx.quiet(ClientMsg::CloseBench { bench: running_bench(&tree, &a.bench)? }),
        Call::DeleteBench(a) => ctx.answered(ClientMsg::DeleteBench { name: a.name }).await,
        Call::SaveBench(a) => ctx.answered(ClientMsg::SaveBench { bench: running_bench(&tree, &a.bench)?, as_name: None }).await,
        Call::RunStartup(a) => ctx.quiet(ClientMsg::RunStartup { bench: running_bench(&tree, &a.bench)? }),
        Call::RunTeardown(a) => ctx.quiet(ClientMsg::RunTeardown { bench: running_bench(&tree, &a.bench)? }),
        Call::RenameBench(a) => {
            let name = a.name.trim().to_string();
            if name.is_empty() {
                return Err(invalid("a bench needs a name"));
            }
            match bench_ref(&a.bench)? {
                shed_proto::BenchRef::Running(id) => {
                    keeper::make(&ctx.socket, &ctx.tree, ClientMsg::RenameBench { bench: id, name: name.clone() }, |t| {
                        t.bench(id).filter(|b| b.name == name).map(|_| ())
                    })
                    .await?;
                    Ok(Reply::Done)
                }
                closed => ctx.edit(&closed, BenchEdit::Rename(name)).await,
            }
        }
        Call::SetBenchColor(a) => {
            let color = crate::state::bench_color(&a.color).ok_or_else(|| invalid(format!("{:?} is not a colour", a.color)))?;
            ctx.edit(&bench_ref(&a.bench)?, BenchEdit::Color(color)).await
        }
        Call::SetBadge(a) => ctx.edit(&bench_ref(&a.bench)?, BenchEdit::Badge(a.badge.filter(|b| !b.trim().is_empty()))).await,
        Call::Previews => previews(&ctx, &tree).await,

        // ---- tabs
        Call::NewTab(a) => new_tab(&ctx, &tree, a).await,
        Call::CloseTab(a) => {
            let tab = tab_id(&a.tab)?;
            // (A remote tab only lets go of its shell here, as the GUI's close does.)
            if tree.tab(tab).is_some() {
                return ctx.quiet(ClientMsg::CloseTab { tab });
            }
            let name = closed_bench_of(&ctx, tab).await?;
            ctx.answered(ClientMsg::EditSaved { name, edit: SavedEdit::RemoveTab { tab } }).await
        }
        Call::RenameTab(a) => {
            let name = a.name.filter(|n| !n.trim().is_empty());
            ctx.edit(&bench_ref(&a.bench)?, BenchEdit::RenameTab { tab: tab_id(&a.tab)?, name }).await
        }
        Call::SetTags(a) => {
            let mut tags = Vec::new();
            for t in a.tags {
                let label = t.label.trim().to_string();
                if label.is_empty() {
                    continue;
                }
                let color = shed_model::tag_color(&t.color).unwrap_or_else(|| shed_model::default_tag_color(&label));
                tags.push(shed_model::Tag { label, color, tint_tab: t.tint });
            }
            ctx.edit(&bench_ref(&a.bench)?, BenchEdit::TabTags { tab: tab_id(&a.tab)?, tags }).await
        }
        Call::SetPrivate(a) => {
            // Only someone at the workspace makes a private tab public again.
            if !a.private {
                return Err(forbidden_private());
            }
            ctx.edit(&bench_ref(&a.bench)?, BenchEdit::TabPrivate { tab: tab_id(&a.tab)?, private: true }).await
        }
        Call::MoveTab(a) => {
            let tab = tab_id(&a.tab)?;
            let (from, to) = (bench_ref(&a.from)?, bench_ref(&a.to)?);
            match (&from, &to) {
                (shed_proto::BenchRef::Running(_), shed_proto::BenchRef::Running(to)) => ctx.quiet(ClientMsg::MoveTab { tab, bench: *to, index: a.index as usize }),
                _ => ctx.answered(ClientMsg::MoveTabAcross { tab, from, to, index: a.index as usize }).await,
            }
        }
        Call::SetDivider(a) => {
            let tab = tab_id(&a.tab)?;
            running_tab(&tree, tab)?;
            let divider = match a.divider {
                shed_api::state::Divider::None => shed_model::TabDivider::None,
                shed_api::state::Divider::Space => shed_model::TabDivider::Space,
                shed_api::state::Divider::Row => shed_model::TabDivider::Row,
            };
            ctx.quiet(ClientMsg::SetTabDivider { tab, divider })
        }
        Call::FocusTab(a) => {
            let tab = tab_id(&a.tab)?;
            running_tab(&tree, tab)?;
            ctx.quiet(ClientMsg::FocusTab { tab })
        }

        // ---- terminals
        Call::Split(a) => {
            let pane = pane_id(&a.pane)?;
            let tab = pane_tab(&tree, pane)?.id;
            let cwd = a.folder.as_deref().map(folder).transpose()?;
            let dir = match a.dir {
                shed_api::state::SplitDir::Right => shed_model::SplitDir::Right,
                shed_api::state::SplitDir::Down => shed_model::SplitDir::Down,
            };
            let before: Vec<PaneId> = tree.tab(tab).map(|t| t.panes.iter().map(|p| p.id).collect()).unwrap_or_default();
            let new = keeper::make(&ctx.socket, &ctx.tree, ClientMsg::Split { pane, dir, cwd }, |t| {
                t.tab(tab)?.panes.iter().find(|p| !before.contains(&p.id)).map(|p| p.id)
            })
            .await?;
            Ok(Reply::Pane(new.to_string()))
        }
        Call::ClosePane(a) => {
            let pane = pane_id(&a.pane)?;
            pane_tab(&tree, pane)?;
            ctx.quiet(ClientMsg::ClosePane { pane })
        }
        Call::FocusPane(a) => {
            let pane = pane_id(&a.pane)?;
            pane_tab(&tree, pane)?;
            ctx.quiet(ClientMsg::FocusPane { pane })
        }
        Call::StartNow(a) => {
            let pane = pane_id(&a.pane)?;
            pane_tab(&tree, pane)?;
            ctx.quiet(ClientMsg::StartNow { pane })
        }
        Call::CancelAsk(a) => {
            let pane = pane_id(&a.pane)?;
            pane_tab(&tree, pane)?;
            ctx.quiet(ClientMsg::CancelAsk { pane })
        }
        Call::Input(_) | Call::TakeSize(_) | Call::ReleaseSize(_) | Call::Scrollback(_) => {
            Err(ApiError::new(ErrorCode::Failed, "answered by the link's own task"))
        }

        // ---- logbook
        Call::LogSearch(a) => log_search(&ctx, &tree, a).await,

        // ---- AI tabs
        Call::AiSessions(_)
        | Call::AiOpenSession(_)
        | Call::AiRenameSession(_)
        | Call::AiSend(_)
        | Call::AiStop(_)
        | Call::AiAnswer(_)
        | Call::AiAnswerQuestions(_)
        | Call::AiEdit(_)
        | Call::AiRevert(_)
        | Call::AiQueued(_)
        | Call::AiSettings(_)
        | Call::AiSetUnsent(_)
        | Call::AiAddDraft(_)
        | Call::AiRemoveDraft(_)
        | Call::AiPicture(_)
        | Call::AiReview(_)
        | Call::AiSetViewed(_)
        | Call::AiExplain(_)
        | Call::AiStart(_) => ai::call(&ctx, &tree, call).await,

        // ---- projects
        Call::ListFolder(_) | Call::QuickOpen(_) | Call::ReadFile(_) | Call::WriteFile(_) | Call::CreateEntry(_) | Call::BrowseFolders(_) | Call::Browse(_) => {
            project::call(&ctx, &tree, call).await
        }

        // ---- Git
        Call::GitStatus(_)
        | Call::GitDiff(_)
        | Call::GitStage(_)
        | Call::GitUnstage(_)
        | Call::GitDiscard(_)
        | Call::GitHunk(_)
        | Call::GitCommit(_)
        | Call::GitLog(_)
        | Call::GitCommitFiles(_)
        | Call::GitBranches(_)
        | Call::GitSwitch(_)
        | Call::GitCreateBranch(_)
        | Call::GitFetch(_)
        | Call::GitPull(_)
        | Call::GitPush(_) => git::call(&tree, call).await,
    }
}

/// A running bench by id.
fn running_bench(tree: &Shed, id: &str) -> Result<BenchId, ApiError> {
    let id = bench_id(id)?;
    tree.bench(id).map(|b| b.id).ok_or_else(|| not_found("no such bench (running)"))
}

/// The closed bench holding `tab`, by name.
async fn closed_bench_of(ctx: &Ctx, tab: TabId) -> Result<String, ApiError> {
    let pegboard = ctx.pegboard().await?;
    pegboard
        .stored
        .iter()
        .find(|s| s.bench.tabs.iter().any(|t| t.id == tab))
        .map(|s| s.bench.name.clone())
        .ok_or_else(|| not_found("no such tab"))
}

async fn new_tab(ctx: &Ctx, tree: &Shed, a: api::NewTab) -> Result<Reply, ApiError> {
    let bench = bench_ref(&a.bench)?;
    match bench {
        shed_proto::BenchRef::Running(bench) => {
            if tree.bench(bench).is_none() {
                return Err(not_found("no such bench (running)"));
            }
            let msg = match a.kind {
                api::NewTabKind::Terminal(f) => ClientMsg::NewTab { bench, cwd: f.folder.as_deref().map(folder).transpose()? },
                api::NewTabKind::Command(c) => {
                    if c.command.trim().is_empty() {
                        return Err(invalid("no command"));
                    }
                    ClientMsg::NewCommandTab { bench, cwd: folder(&c.folder)?, command: c.command }
                }
                api::NewTabKind::Project(f) => ClientMsg::NewProjectTab { bench, root: folder(&f.folder)? },
                api::NewTabKind::Ai(f) => ClientMsg::NewAiTab { bench, folder: folder(&f.folder)? },
                api::NewTabKind::Remote(r) => ClientMsg::NewRemoteTab { bench, host: r.host, join: r.join },
            };
            let before: Vec<TabId> = tree.bench(bench).map(|b| b.tabs.iter().map(|t| t.id).collect()).unwrap_or_default();
            let tab = keeper::make(&ctx.socket, &ctx.tree, msg, |t| t.bench(bench)?.tabs.iter().find(|t| !before.contains(&t.id)).map(|t| t.id)).await?;
            Ok(Reply::Tab(tab.to_string()))
        }
        shed_proto::BenchRef::Saved(name) => {
            let edit = match a.kind {
                api::NewTabKind::Terminal(api::OptionalFolder { folder: None }) => SavedEdit::NewTab,
                api::NewTabKind::Project(f) => SavedEdit::NewProjectTab { root: folder(&f.folder)? },
                api::NewTabKind::Ai(f) => SavedEdit::NewAiTab { folder: folder(&f.folder)? },
                _ => return Err(invalid("a closed bench takes new shells in its own folder, project tabs and AI tabs; open it for the rest")),
            };
            let tabs_of = |p: &PegboardInfo| -> Option<Vec<TabId>> {
                p.stored.iter().find(|s| s.bench.name == name).map(|s| s.bench.tabs.iter().map(|t| t.id).collect())
            };
            let before = tabs_of(&*ctx.pegboard().await?).ok_or_else(|| not_found("no such closed bench"))?;
            ctx.answered(ClientMsg::EditSaved { name: name.clone(), edit }).await?;
            let after = tabs_of(&*ctx.pegboard().await?).unwrap_or_default();
            let tab = after.into_iter().find(|t| !before.contains(t)).ok_or_else(|| ApiError::new(ErrorCode::Failed, "the tab did not show"))?;
            Ok(Reply::Tab(tab.to_string()))
        }
    }
}

/// The last lines of every tab's active pane, private tabs left out.
async fn previews(ctx: &Ctx, tree: &Shed) -> Result<Reply, ApiError> {
    let pegboard = ctx.pegboard().await?;
    let private: Vec<PaneId> = tree
        .benches
        .iter()
        .chain(pegboard.stored.iter().map(|s| &s.bench))
        .flat_map(|b| &b.tabs)
        .filter(|t| t.private)
        .flat_map(|t| t.panes.iter().map(|p| p.id).chain([t.active_pane]))
        .collect();
    let previews = pegboard
        .previews
        .iter()
        .filter(|(pane, _)| !private.contains(pane))
        .map(|(pane, lines)| api::Preview { pane: pane.to_string(), lines: lines.clone() })
        .collect();
    Ok(Reply::Previews(previews))
}

async fn log_search(ctx: &Ctx, tree: &Shed, a: api::LogSearch) -> Result<Reply, ApiError> {
    let tab = a.tab.as_deref().map(tab_id).transpose()?;
    // Private tabs' commands stay on the workspace: asked from a private
    // tab (whose own commands would show), nothing.
    let (tab, bench) = match tab {
        Some(tab) => {
            let t = running_tab(tree, tab)?;
            if t.private {
                return Err(forbidden_private());
            }
            (tab, tree.bench_of_tab(tab).expect("a running tab is in a bench"))
        }
        None if a.scope == api::LogScope::All => (TabId::new(), BenchId::new()),
        None => return Err(invalid("the tab and bench scopes need a tab")),
    };
    let scope = match a.scope {
        api::LogScope::Tab => shed_logbook::Scope::Local,
        api::LogScope::Bench => shed_logbook::Scope::Bench,
        api::LogScope::All => shed_logbook::Scope::Global,
    };
    let path = ctx.logbook.clone();
    let limit = if a.limit == 0 { 100 } else { a.limit.min(1000) } as usize;
    let entries = tokio::task::spawn_blocking(move || {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let book = shed_logbook::Logbook::open_readonly(&path).map_err(|e| keeper::failed(format!("the logbook: {e}")))?;
        book.search_typed(scope, shed_logbook::Context { tab, bench }, &a.query, limit).map_err(|e| keeper::failed(format!("the logbook: {e}")))
    })
    .await
    .map_err(|e| keeper::failed(e.to_string()))??;
    // Commands kept before their tab became private were not marked so in
    // logbooks older than that rule (the keeper marks them now): left out
    // here too.
    let private_now: std::collections::HashSet<String> =
        tree.benches.iter().flat_map(|b| &b.tabs).filter(|t| t.private).map(|t| t.id.to_string()).collect();
    Ok(Reply::Log(
        entries
            .into_iter()
            .filter(|e| !private_now.contains(&e.tab))
            .map(|e| api::LogEntry {
                command: e.command,
                cwd: e.cwd,
                exit: e.exit,
                started: (e.started_ms / 1000).clamp(0, u32::MAX as i64) as u32,
                duration: e.duration_ms.unwrap_or(0) as f64 / 1000.0,
                bench: e.bench,
                tab: e.tab,
            })
            .collect(),
    ))
}

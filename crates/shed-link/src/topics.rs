//! What a subscription streams, besides terminals (`screen`): an AI tab's
//! conversation (from the keeper's updates), a project's changes on disk,
//! and a search in its files.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use shed_api::topics::{self as api, Event, SearchTopic};
use shed_api::wire::{EventFrame, LinkFrame};
use tokio::sync::mpsc;

use crate::calls::Ctx;

pub(crate) type Out = mpsc::UnboundedSender<LinkFrame>;

pub(crate) fn send(out: &Out, sub: u32, event: Event) {
    let _ = out.send(LinkFrame::Event(EventFrame { sub, event }));
}

// ---------------------------------------------------------------------- AI

pub(crate) fn transcript(from: u32, entries: &[shed_ai::Entry], status: &shed_ai::Status) -> api::AiTranscript {
    api::AiTranscript { from, entries: entries.iter().map(entry).collect(), status: ai_status(status) }
}

/// A picture's name, from the path the keeper keeps it at.
fn picture_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn edit(d: &Option<shed_ai::Diff>) -> Option<api::AiTextEdit> {
    d.as_ref().map(|d| api::AiTextEdit { old: d.old.clone(), new: d.new.clone() })
}

fn secs(t: u64) -> u32 {
    t.min(u32::MAX as u64) as u32
}

fn entry(e: &shed_ai::Entry) -> api::AiEntry {
    match e {
        shed_ai::Entry::User { text, images, at, pictures } => api::AiEntry::User(api::AiUser {
            text: text.clone(),
            at: secs(*at),
            pictures: pictures.iter().map(|p| picture_name(p)).collect(),
            images: *images,
        }),
        shed_ai::Entry::Assistant { text, at } => api::AiEntry::Assistant(api::AiAssistant { text: text.clone(), at: secs(*at) }),
        shed_ai::Entry::Thinking { text } => api::AiEntry::Thinking(api::AiText { text: text.clone() }),
        shed_ai::Entry::Tool(t) => api::AiEntry::Tool(api::AiTool {
            id: t.id.clone(),
            name: t.name.clone(),
            summary: t.summary.clone(),
            detail: t.detail.clone(),
            file: t.file.clone(),
            edit: edit(&t.diff),
            result: t.result.clone(),
            error: t.error,
            pictures: t.pictures.iter().map(|p| picture_name(p)).collect(),
        }),
        shed_ai::Entry::Permission(p) => api::AiEntry::Permission(api::AiPermission {
            request: p.request.clone(),
            tool: p.tool.clone(),
            summary: p.summary.clone(),
            detail: p.detail.clone(),
            edit: edit(&p.diff),
            answer: p.answer,
            questions: p
                .questions
                .iter()
                .map(|q| api::AiQuestion {
                    question: q.question.clone(),
                    header: q.header.clone(),
                    options: q.options.iter().map(|o| api::AiOption { label: o.label.clone(), description: o.description.clone() }).collect(),
                    multi: q.multi,
                })
                .collect(),
            answers: p.answers.clone(),
        }),
        shed_ai::Entry::Notice { text, error } => api::AiEntry::Notice(api::AiNotice { text: text.clone(), error: *error }),
    }
}

fn ai_status(s: &shed_ai::Status) -> api::AiStatus {
    api::AiStatus {
        session: s.session.clone(),
        model: s.model.clone(),
        state: match s.state {
            shed_ai::State::Idle => shed_api::state::AiActivity::Idle,
            shed_ai::State::Working => shed_api::state::AiActivity::Working,
            shed_ai::State::Waiting => shed_api::state::AiActivity::Waiting,
        },
        cost: s.cost,
        context: s.context as f64,
        context_max: s.context_max as f64,
        since: s.since.map(secs),
    }
}

pub(crate) fn ai_info(commands: &[shed_ai::Command], models: &[shed_ai::Model]) -> api::AiInfo {
    api::AiInfo {
        commands: commands.iter().map(|c| api::AiCommand { name: c.name.clone(), description: c.description.clone(), hint: c.hint.clone() }).collect(),
        models: models
            .iter()
            .map(|m| api::AiModel { value: m.value.clone(), name: m.name.clone(), description: m.description.clone(), efforts: m.efforts.clone() })
            .collect(),
    }
}

// ---------------------------------------------------------------- projects

/// Git's state is read again this long after the last change settles.
const GIT_SETTLE: Duration = Duration::from_millis(300);

/// A project being followed on disk; dropping it stops.
pub(crate) struct ProjectWatch {
    _watch: shed_project::Watch,
    git: tokio::task::JoinHandle<()>,
}

impl Drop for ProjectWatch {
    fn drop(&mut self) {
        self.git.abort();
    }
}

/// Follows `root` on disk: changed files as `FilesChanged` (their paths
/// relative to the root, Git's own folder left out), and `GitChanged` once
/// changes settle.
pub(crate) fn watch_project(ctx: Arc<Ctx>, root: PathBuf, exclude: Vec<String>, sub: u32, out: Out) -> anyhow::Result<ProjectWatch> {
    let walked = shed_project::walk(&root, &exclude);
    let excluder = shed_project::Excluder::new(&root, &exclude);
    let (kick, mut kicked) = mpsc::unbounded_channel::<()>();
    let files_out = out.clone();
    let real_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let watch_root = root.clone();
    let watch = shed_project::Watch::new(&walked.dirs, excluder, move |batch| {
        ctx.forget_files(&watch_root);
        let paths: Vec<String> = batch
            .iter()
            .filter_map(|p| crate::paths::relative(&watch_root, p).or_else(|| crate::paths::relative(&real_root, p)))
            .filter(|p| p != ".git" && !p.starts_with(".git/"))
            .collect();
        if !paths.is_empty() {
            send(&files_out, sub, Event::FilesChanged(api::FilesChanged { paths }));
        }
        let _ = kick.send(());
    })?;
    let git = tokio::spawn(async move {
        while kicked.recv().await.is_some() {
            // Until things are quiet for a moment.
            while let Ok(Some(())) = tokio::time::timeout(GIT_SETTLE, kicked.recv()).await {}
            send(&out, sub, Event::GitChanged);
        }
    });
    Ok(ProjectWatch { _watch: watch, git })
}

/// Searches a project's files on a thread of its own, sending hits as files
/// are done and `SearchDone` at the end; setting the flag stops it.
pub(crate) fn search(ctx: Arc<Ctx>, root: PathBuf, exclude: Vec<String>, topic: SearchTopic, sub: u32, out: Out) -> Arc<AtomicBool> {
    const LIMIT: usize = 5000;
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    tokio::task::spawn_blocking(move || {
        let q = shed_project::search::Query { text: topic.query.clone(), regex: topic.regex, case_sensitive: topic.case_sensitive, whole_word: topic.whole_word };
        if let Err(e) = shed_project::search::check(&q) {
            send(&out, sub, Event::SearchDone(api::SearchDone { truncated: false, error: Some(e) }));
            return;
        }
        let globs = |g: &Option<String>| -> Vec<String> {
            g.as_deref().unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
        };
        let (include, leave_out) = (globs(&topic.include), globs(&topic.exclude));
        let matches = |pats: &[String], path: &Path| {
            let full = crate::calls::slash(path);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            pats.iter().any(|p| shed_model::config::glob(p, &full) || shed_model::config::glob(p, &name) || full.starts_with(&format!("{}/", p.trim_end_matches('/'))))
        };
        let files: Vec<PathBuf> = ctx
            .files(&root, &exclude)
            .iter()
            .filter(|f| (include.is_empty() || matches(&include, f)) && !matches(&leave_out, f))
            .cloned()
            .collect();
        let found = std::sync::atomic::AtomicUsize::new(0);
        let result = shed_project::search::search(&root, &files, &q, &stop, LIMIT, |hits| {
            found.fetch_add(hits.hits.len(), Ordering::Relaxed);
            let hits = api::SearchHits {
                path: crate::calls::slash(&hits.path),
                hits: hits
                    .hits
                    .into_iter()
                    .map(|h| {
                        let at = |b: usize| crate::calls::utf16_at(&h.text, b);
                        api::SearchHit { line: h.line as u32 + 1, ranges: h.ranges.iter().flat_map(|r| [at(r.start), at(r.end)]).collect(), text: h.text }
                    })
                    .collect(),
            };
            send(&out, sub, Event::SearchHits(hits));
        });
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let truncated = found.load(Ordering::Relaxed) >= LIMIT;
        send(&out, sub, Event::SearchDone(api::SearchDone { truncated, error: result.err() }));
    });
    cancel
}

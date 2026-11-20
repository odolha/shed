//! The workspace's state as connectors see it (`shed_api::state`), made
//! from the keeper's tree, its closed benches and the usage limits.
//! Private tabs keep their place and kind and nothing else.

use std::collections::HashSet;

use shed_api::state as api;
use shed_model::{BenchColor, BenchId, PaneId, PaneNode, Rgb, Shed, Tab, TabKind};
use shed_proto::PegboardInfo;

/// What a private tab is called.
pub(crate) const PRIVATE: &str = "Private";

/// Everything the state is made from.
pub(crate) struct Inputs<'a> {
    pub tree: &'a Shed,
    pub pegboard: Option<&'a PegboardInfo>,
    /// Benches a window shows.
    pub open: &'a HashSet<BenchId>,
    pub limits: Option<shed_ai::Limits>,
    /// Panes whose size a connector holds.
    pub held: &'a HashSet<PaneId>,
    pub host: &'a str,
    pub home: &'a str,
}

pub(crate) fn workspace(i: &Inputs) -> api::WorkspaceState {
    let stored = i.pegboard.map(|p| p.stored.as_slice()).unwrap_or_default();
    let order = i.pegboard.map(|p| p.order.as_slice()).unwrap_or_default();
    let listed = shed_client::pegboard::list(i.tree, |b| i.open.contains(&b), stored, order, "");
    let mut benches = Vec::new();
    let mut closed = Vec::new();
    for l in listed {
        let b = &l.bench;
        let tabs: Vec<api::Tab> = b.tabs.iter().map(|t| tab(t, i.held)).collect();
        let links = b.links.iter().map(|l| api::Link { name: l.name.clone(), url: l.url.clone() }).collect();
        match l.saved {
            Some(saved) => closed.push(api::ClosedBench {
                name: b.name.clone(),
                color: hex(b.color.accent()),
                badge: b.badge.clone(),
                cwd: b.cwd.to_string_lossy().into_owned(),
                tabs,
                saved: unix_time(&saved),
                links,
            }),
            None => benches.push(api::Bench {
                id: b.id.to_string(),
                name: b.name.clone(),
                color: hex(b.color.accent()),
                badge: b.badge.clone(),
                cwd: b.cwd.to_string_lossy().into_owned(),
                tabs,
                active_tab: b.active_tab.to_string(),
                shown: i.open.contains(&b.id),
                closing: b.closing,
                started: b.started,
                links,
            }),
        }
    }
    api::WorkspaceState { host: i.host.to_string(), benches, closed, limits: i.limits.map(limits), home: i.home.to_string() }
}

/// `#rrggbb`.
pub(crate) fn hex(c: Rgb) -> String {
    format!("#{:06x}", c.to_hex())
}

/// A bench colour from what a connector sends: a name or `#rrggbb`.
pub(crate) fn bench_color(s: &str) -> Option<BenchColor> {
    BenchColor::parse(s)
}

pub(crate) fn limits(l: shed_ai::Limits) -> api::Limits {
    let window = |w: shed_ai::Window| api::LimitWindow { used: w.used, resets: w.resets.min(u32::MAX as u64) as u32 };
    api::Limits { five_hour: l.five_hour.map(window), week: l.week.map(window), seen: l.seen.min(u32::MAX as u64) as u32 }
}

pub(crate) fn tab(t: &Tab, held: &HashSet<PaneId>) -> api::Tab {
    let divider = match t.divider {
        shed_model::TabDivider::None => api::Divider::None,
        shed_model::TabDivider::Space => api::Divider::Space,
        shed_model::TabDivider::Row => api::Divider::Row,
    };
    let (panes, layout, active_pane) = if t.has_panes() {
        let panes = t.panes.iter().map(|p| pane(p, t.private, held.contains(&p.id))).collect();
        (panes, Some(layout(&t.layout)), Some(t.active_pane.to_string()))
    } else {
        (Vec::new(), None, None)
    };
    if t.private {
        return api::Tab {
            id: t.id.to_string(),
            name: None,
            title: PRIVATE.into(),
            tags: Vec::new(),
            private: true,
            kind: match &t.kind {
                TabKind::Terminal => api::TabKind::Terminal,
                TabKind::Project(_) => api::TabKind::Project(Default::default()),
                TabKind::Ai(_) => api::TabKind::Ai(Default::default()),
                TabKind::Remote(_) => api::TabKind::Remote(Default::default()),
            },
            panes,
            layout,
            active_pane,
            divider,
        };
    }
    let kind = match &t.kind {
        TabKind::Terminal => api::TabKind::Terminal,
        TabKind::Project(p) => api::TabKind::Project(api::ProjectTab {
            root: p.root.to_string_lossy().into_owned(),
            open: p.open.iter().map(|f| f.strip_prefix(&p.root).unwrap_or(f).to_string_lossy().into_owned()).collect(),
            active: p.active.map(|a| a as u32),
            links: p.links.iter().map(|l| api::Link { name: l.name.clone(), url: l.url.clone() }).collect(),
        }),
        TabKind::Ai(a) => api::TabKind::Ai(api::AiTab {
            folder: a.folder.to_string_lossy().into_owned(),
            add_dirs: a.add_dirs.iter().map(|d| d.to_string_lossy().into_owned()).collect(),
            session: a.session.clone(),
            draft_session: a.draft_session.clone(),
            model: a.model.clone(),
            mode: a.mode.clone(),
            effort: a.effort.clone(),
            activity: activity(a.activity),
            drafts: a.drafts.iter().map(|d| api::AiDraft { session: d.session.clone(), text: d.text.clone() }).collect(),
            queued: a.queued.iter().map(|q| q.text.clone()).collect(),
            unsent: a.unsent.clone(),
            browser: a.browser,
            follow: a.follow,
        }),
        TabKind::Remote(r) => api::TabKind::Remote(api::RemoteTab { host: r.host.clone(), id: r.id.clone() }),
    };
    api::Tab {
        id: t.id.to_string(),
        name: t.name.clone(),
        title: t.display_title(),
        tags: t.all_tags().map(|g| api::Tag { label: g.label.clone(), color: hex(g.color), tint: g.tint_tab }).collect(),
        private: false,
        kind,
        panes,
        layout,
        active_pane,
        divider,
    }
}

pub(crate) fn activity(a: shed_model::AiActivity) -> api::AiActivity {
    match a {
        shed_model::AiActivity::Idle => api::AiActivity::Idle,
        shed_model::AiActivity::Working => api::AiActivity::Working,
        shed_model::AiActivity::Waiting => api::AiActivity::Waiting,
    }
}

fn pane(p: &shed_model::Pane, private: bool, size_held: bool) -> api::Pane {
    if private {
        return api::Pane { id: p.id.to_string(), cols: p.cols as u32, rows: p.rows as u32, size_held, ..Default::default() };
    }
    api::Pane {
        id: p.id.to_string(),
        cwd: p.cwd.to_string_lossy().into_owned(),
        cols: p.cols as u32,
        rows: p.rows as u32,
        title: p.title.clone(),
        tool: p.tool.as_ref().map(|t| t.display()),
        waiting: p.waiting.clone(),
        held: p.held.is_some(),
        asking: p.ai.as_ref().map(|a| match a {
            shed_model::AiState::Asking(r) => r.clone(),
            shed_model::AiState::Failed(e) => e.clone(),
        }),
        size_held,
    }
}

fn layout(node: &PaneNode) -> api::Layout {
    match node {
        PaneNode::Leaf(id) => api::Layout::Pane(id.to_string()),
        PaneNode::Split { dir, ratio, first, second } => api::Layout::Split(api::Split {
            dir: match dir {
                shed_model::SplitDir::Right => api::SplitDir::Right,
                shed_model::SplitDir::Down => api::SplitDir::Down,
            },
            ratio: *ratio,
            first: Box::new(layout(first)),
            second: Box::new(layout(second)),
        }),
    }
}

/// `2026-11-01T12:00:00Z` (as saved benches say when they were saved) in
/// Unix seconds; 0 if it does not read.
pub(crate) fn unix_time(s: &str) -> u32 {
    let parse = || -> Option<i64> {
        let (date, time) = s.trim().trim_end_matches('Z').split_once('T')?;
        let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
        let (y, m, day) = (d.next()??, d.next()??, d.next()??);
        let mut t = time.split(':').map(|x| x.split('.').next().and_then(|x| x.parse::<i64>().ok()));
        let (hh, mm, ss) = (t.next()??, t.next()??, t.next().flatten().unwrap_or(0));
        // Days from the civil date (Howard Hinnant's algorithm).
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146097 + doe - 719468;
        Some(days * 86400 + hh * 3600 + mm * 60 + ss)
    };
    parse().filter(|t| *t >= 0).map_or(0, |t| t.min(u32::MAX as i64) as u32)
}

#[cfg(test)]
mod tests {
    use shed_model::{Bench, Pane, Tag, Tool};
    use shed_proto::StoredBench;

    use super::*;

    fn bench(name: &str, tabs: Vec<Tab>) -> Bench {
        Bench {
            id: BenchId::new(),
            name: name.into(),
            color: BenchColor::Coral,
            badge: None,
            cwd: "/srv".into(),
            active_tab: tabs[0].id,
            tabs,
            aliases: Default::default(),
            closing: false,
            launcher: name.into(),
            window: None,
            started: true,
            links: Vec::new(),
        }
    }

    #[test]
    fn the_tree_as_connectors_see_it() {
        let mut shell = Pane::new(PaneId::new(), "/srv/api".into());
        shell.tool = Some(Tool { argv: vec!["/usr/bin/vim".into(), "notes.md".into()] });
        let mut term = Tab::with_pane(shell.clone());
        term.tags.push(Tag { label: "prod".into(), color: Rgb::hex(0xF0566A), tint_tab: true });
        let mut secret = Tab::with_pane(Pane::new(PaneId::new(), "/home/me/keys".into()));
        secret.private = true;
        secret.name = Some("passwords".into());
        let mut ai = Tab::ai("/srv/api".into());
        if let TabKind::Ai(a) = &mut ai.kind {
            a.activity = shed_model::AiActivity::Waiting;
        }
        let running = bench("api", vec![term.clone(), secret.clone(), ai]);
        let old = bench("old", vec![Tab::project("/srv/old".into())]);
        let tree = Shed { benches: vec![running.clone()], ..Default::default() };
        let pegboard = PegboardInfo {
            stored: vec![StoredBench { bench: old.clone(), saved: "2026-11-01T12:00:00Z".into() }],
            previews: Vec::new(),
            order: vec![old.id, running.id],
        };
        let open = HashSet::from([running.id]);
        let held = HashSet::from([shell.id]);
        let state = workspace(&Inputs { tree: &tree, pegboard: Some(&pegboard), open: &open, limits: None, held: &held, host: "pc", home: "/home/me" });

        assert_eq!(state.benches.len(), 1);
        assert_eq!(state.closed.len(), 1);
        let b = &state.benches[0];
        assert_eq!((b.name.as_str(), b.color.as_str(), b.shown), ("api", "#ff9a5a", true));
        let t = &b.tabs[0];
        assert_eq!(t.title, "vim notes.md");
        assert_eq!(t.tags[0].color, "#f0566a");
        assert_eq!(t.panes[0].tool.as_deref(), Some("vim notes.md"));
        assert!(t.panes[0].size_held);
        assert!(matches!(t.layout, Some(api::Layout::Pane(ref id)) if *id == shell.id.to_string()));
        // A private tab: its place and kind, nothing of what is in it.
        let p = &b.tabs[1];
        assert!(p.private && p.name.is_none() && p.title == PRIVATE && p.tags.is_empty());
        assert_eq!(p.panes[0].cwd, "");
        assert_eq!(p.panes[0].id, secret.panes[0].id.to_string());
        assert!(matches!(&b.tabs[2].kind, api::TabKind::Ai(a) if a.activity == api::AiActivity::Waiting && a.folder == "/srv/api"));
        assert_eq!(state.closed[0].saved, unix_time("2026-11-01T12:00:00Z"));
        assert!(matches!(&state.closed[0].tabs[0].kind, api::TabKind::Project(p) if p.root == "/srv/old"));
    }

    #[test]
    fn saved_times() {
        assert_eq!(unix_time("1970-01-01T00:00:00Z"), 0);
        assert_eq!(unix_time("2000-03-01T00:00:00Z"), 951_868_800);
        assert_eq!(unix_time("2026-11-01T12:00:00Z"), 1_793_534_400);
        assert_eq!(unix_time("garbage"), 0);
    }
}

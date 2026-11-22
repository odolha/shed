//! The pegboard's list: every bench, running or closed, in the order the
//! user arranged, with the tabs a typed filter lets through.

use shed_model::{Bench, BenchId, Shed, Tab, TabId};
use shed_proto::{BenchRef, StoredBench};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchStatus {
    /// Running, with a window.
    Open,
    /// Running, without a window.
    Hidden,
    /// Only saved.
    Closed,
}

/// A bench as listed.
#[derive(Clone, Debug, PartialEq)]
pub struct ListedBench {
    pub bench: Bench,
    pub status: BenchStatus,
    /// When a closed bench was saved.
    pub saved: Option<String>,
    /// The tabs the filter lets through.
    pub tabs: Vec<TabId>,
}

impl ListedBench {
    /// The bench as edits address it: a running one by id, a closed one by name.
    pub fn bench_ref(&self) -> BenchRef {
        match self.status {
            BenchStatus::Closed => BenchRef::Saved(self.bench.name.clone()),
            BenchStatus::Open | BenchStatus::Hidden => BenchRef::Running(self.bench.id),
        }
    }
}

/// Every bench in order, each with the tabs `query` matches (every word, in
/// any case; a bench whose name matches shows all its tabs). Running benches
/// come from `tree` (`has_window` tells open from hidden), closed ones from
/// `stored`. In the order of `order`, the rest by name, so a bench stays
/// where it is as it opens and closes.
pub fn list(tree: &Shed, has_window: impl Fn(BenchId) -> bool, stored: &[StoredBench], order: &[BenchId], query: &str) -> Vec<ListedBench> {
    let live = tree.benches.iter().map(|b| {
        let status = if has_window(b.id) { BenchStatus::Open } else { BenchStatus::Hidden };
        (b.clone(), status, None)
    });
    let stored = stored.iter().filter(|s| tree.bench(s.bench.id).is_none());
    let stored = stored.map(|s| (s.bench.clone(), BenchStatus::Closed, Some(s.saved.clone())));
    let mut all: Vec<(Bench, BenchStatus, Option<String>)> = live.chain(stored).collect();
    all.sort_by_key(|(b, _, _)| (order.iter().position(|id| *id == b.id).unwrap_or(usize::MAX), b.name.to_lowercase()));

    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let matches = |text: &str| {
        let text = text.to_lowercase();
        words.iter().all(|w| text.contains(w.as_str()))
    };
    all.into_iter()
        .filter_map(|(bench, status, saved)| {
            let tabs: Vec<TabId> = if matches(&bench.name) {
                bench.tabs.iter().map(|t| t.id).collect()
            } else {
                bench.tabs.iter().filter(|t| matches(&format!("{} {}", bench.name, haystack(t)))).map(|t| t.id).collect()
            };
            (!tabs.is_empty()).then_some(ListedBench { bench, status, saved, tabs })
        })
        .collect()
}

/// Every listed tab in order, as (index of its bench in `listed`, tab).
pub fn flat(listed: &[ListedBench]) -> Vec<(usize, TabId)> {
    listed.iter().enumerate().flat_map(|(i, e)| e.tabs.iter().map(move |t| (i, *t))).collect()
}

/// What the filter searches in a tab: its title, tags, folders and tools.
pub fn haystack(tab: &Tab) -> String {
    let mut text = tab.display_title();
    for tag in tab.all_tags() {
        text.push(' ');
        text.push_str(&tag.label);
    }
    for pane in &tab.panes {
        text.push(' ');
        text.push_str(&pane.cwd.to_string_lossy());
        if let Some(tool) = &pane.tool {
            text.push(' ');
            text.push_str(&tool.display());
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use shed_model::{BenchColor, Pane, PaneId};

    use super::*;

    fn bench(name: &str, cwds: &[&str]) -> Bench {
        let tabs: Vec<Tab> = cwds.iter().map(|c| Tab::with_pane(Pane::new(PaneId::new(), c.into()))).collect();
        Bench {
            id: BenchId::new(),
            name: name.into(),
            color: BenchColor::Blue,
            badge: None,
            cwd: "/".into(),
            active_tab: tabs[0].id,
            tabs,
            aliases: Default::default(),
            closing: false,
            launcher: name.into(),
            window: None,
            started: false,
            links: Vec::new(),
        }
    }

    #[test]
    fn benches_in_order_filtered_by_tab() {
        let (api, web, old) = (bench("api", &["/srv/api", "/srv/api/db"]), bench("Web", &["/srv/web"]), bench("old", &["/srv/old"]));
        let tree = Shed { benches: vec![web.clone(), api.clone()], ..Default::default() };
        let stored = vec![
            StoredBench { bench: old.clone(), saved: "2026-11-01T12:00:00Z".into() },
            // Running too: listed once, as running.
            StoredBench { bench: api.clone(), saved: "2026-11-01T12:00:00Z".into() },
        ];
        let names = |l: &[ListedBench]| l.iter().map(|e| e.bench.name.clone()).collect::<Vec<_>>();
        let all = list(&tree, |b| b == web.id, &stored, &[], "");
        assert_eq!(names(&all), ["api", "old", "Web"]);
        assert_eq!(all.iter().map(|e| e.status).collect::<Vec<_>>(), [BenchStatus::Hidden, BenchStatus::Closed, BenchStatus::Open]);
        assert_eq!(all[1].bench_ref(), BenchRef::Saved("old".into()));
        assert_eq!(all[0].bench_ref(), BenchRef::Running(api.id));
        assert_eq!(flat(&all).len(), 4);
        // Arranged order first, the rest by name.
        assert_eq!(names(&list(&tree, |_| true, &stored, &[web.id], "")), ["Web", "api", "old"]);
        // Words match tabs (their folders here) or the bench's name.
        let found = list(&tree, |_| true, &stored, &[], "API db");
        assert_eq!(names(&found), ["api"]);
        assert_eq!(found[0].tabs, [api.tabs[1].id]);
        assert_eq!(list(&tree, |_| true, &stored, &[], "web")[0].tabs.len(), 1);
        assert!(list(&tree, |_| true, &stored, &[], "nothing").is_empty());
    }
}

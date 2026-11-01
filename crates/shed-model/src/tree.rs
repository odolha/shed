use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{BenchColor, BenchId, PaneId, Rgb, TabId};

/// The whole of Shed: every bench the keeper holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shed {
    pub benches: Vec<Bench>,
}

impl Shed {
    pub fn bench(&self, id: BenchId) -> Option<&Bench> {
        self.benches.iter().find(|b| b.id == id)
    }

    pub fn bench_mut(&mut self, id: BenchId) -> Option<&mut Bench> {
        self.benches.iter_mut().find(|b| b.id == id)
    }

    /// Finds the bench and tab that hold `pane`.
    pub fn locate_pane(&self, pane: PaneId) -> Option<(BenchId, TabId)> {
        self.benches.iter().find_map(|b| {
            b.tabs.iter().find(|t| t.layout.contains(pane)).map(|t| (b.id, t.id))
        })
    }

    pub fn pane(&self, pane: PaneId) -> Option<&Pane> {
        self.benches.iter().flat_map(|b| &b.tabs).flat_map(|t| &t.panes).find(|p| p.id == pane)
    }

    pub fn pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.benches.iter().flat_map(|b| &b.tabs).flat_map(|t| &t.panes).map(|p| p.id)
    }

    /// Removes a pane from whichever tab holds it. Empty tabs are removed, and
    /// empty benches too. Returns the bench the pane was in.
    pub fn remove_pane(&mut self, pane: PaneId) -> Option<BenchId> {
        let (bench_id, tab_id) = self.locate_pane(pane)?;
        let bench = self.bench_mut(bench_id)?;
        let tab = bench.tabs.iter_mut().find(|t| t.id == tab_id)?;
        tab.panes.retain(|p| p.id != pane);
        let empty_tab = match tab.layout.remove(pane) {
            Some(rest) => {
                tab.layout = rest;
                if tab.active_pane == pane {
                    tab.active_pane = tab.layout.first();
                }
                false
            }
            None => true,
        };
        if empty_tab {
            bench.tabs.retain(|t| t.id != tab_id);
            bench.active_tab = bench.active_tab.min(bench.tabs.len().saturating_sub(1));
        }
        if bench.tabs.is_empty() {
            self.benches.retain(|b| b.id != bench_id);
        }
        Some(bench_id)
    }
}

/// A workspace for one job, shown in its own window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bench {
    pub id: BenchId,
    pub name: String,
    pub color: BenchColor,
    pub badge: Option<String>,
    /// Where new tabs and panes open.
    pub cwd: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl Bench {
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub tags: Vec<Tag>,
    pub private: bool,
    pub layout: PaneNode,
    pub panes: Vec<Pane>,
    pub active_pane: PaneId,
}

impl Tab {
    /// A tab with a single pane.
    pub fn with_pane(title: impl Into<String>, pane: Pane) -> Self {
        Self {
            id: TabId::new(),
            title: title.into(),
            tags: Vec::new(),
            private: false,
            layout: PaneNode::Leaf(pane.id),
            active_pane: pane.id,
            panes: vec![pane],
        }
    }
}

/// A short coloured label on a tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub label: String,
    pub color: Rgb,
    pub tint_tab: bool,
}

/// One terminal inside a tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDir {
    /// Children side by side, the second to the right.
    Right,
    /// Children stacked, the second below.
    Down,
}

/// The split tree of a tab's panes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PaneNode {
    Leaf(PaneId),
    Split {
        dir: SplitDir,
        /// Share of the space given to `first`, in `0.0..1.0`.
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            PaneNode::Leaf(id) => *id == pane,
            PaneNode::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    pub fn first(&self) -> PaneId {
        match self {
            PaneNode::Leaf(id) => *id,
            PaneNode::Split { first, .. } => first.first(),
        }
    }

    /// The tree without `pane`, or `None` if nothing is left.
    pub fn remove(&self, pane: PaneId) -> Option<PaneNode> {
        match self {
            PaneNode::Leaf(id) if *id == pane => None,
            PaneNode::Leaf(_) => Some(self.clone()),
            PaneNode::Split { dir, ratio, first, second } => {
                match (first.remove(pane), second.remove(pane)) {
                    (Some(a), Some(b)) => Some(PaneNode::Split {
                        dir: *dir,
                        ratio: *ratio,
                        first: Box::new(a),
                        second: Box::new(b),
                    }),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Pane {
        Pane { id: PaneId::new(), cwd: "/".into(), cols: 80, rows: 24 }
    }

    #[test]
    fn removing_last_pane_removes_tab_and_bench() {
        let p = pane();
        let pid = p.id;
        let mut shed = Shed {
            benches: vec![Bench {
                id: BenchId::new(),
                name: "main".into(),
                color: BenchColor::Blue,
                badge: None,
                cwd: "/".into(),
                tabs: vec![Tab::with_pane("shell", p)],
                active_tab: 0,
            }],
        };
        assert!(shed.remove_pane(pid).is_some());
        assert!(shed.benches.is_empty());
    }

    #[test]
    fn removing_from_split_collapses() {
        let (a, b) = (PaneId::new(), PaneId::new());
        let node = PaneNode::Split {
            dir: SplitDir::Right,
            ratio: 0.5,
            first: Box::new(PaneNode::Leaf(a)),
            second: Box::new(PaneNode::Leaf(b)),
        };
        assert_eq!(node.remove(a), Some(PaneNode::Leaf(b)));
        assert!(node.contains(b));
    }
}

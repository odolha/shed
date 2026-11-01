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

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.benches.iter().flat_map(|b| &b.tabs).find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.benches.iter_mut().flat_map(|b| &mut b.tabs).find(|t| t.id == id)
    }

    /// The bench holding `tab`.
    pub fn bench_of_tab(&self, tab: TabId) -> Option<BenchId> {
        self.benches.iter().find(|b| b.tabs.iter().any(|t| t.id == tab)).map(|b| b.id)
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

    pub fn pane_mut(&mut self, pane: PaneId) -> Option<&mut Pane> {
        self.benches.iter_mut().flat_map(|b| &mut b.tabs).flat_map(|t| &mut t.panes).find(|p| p.id == pane)
    }

    pub fn pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.benches.iter().flat_map(|b| &b.tabs).flat_map(|t| &t.panes).map(|p| p.id)
    }

    /// Removes a pane from whichever tab holds it. Empty tabs are removed, and
    /// empty benches too. Returns the bench the pane was in.
    pub fn remove_pane(&mut self, pane: PaneId) -> Option<BenchId> {
        let (bench_id, tab_id) = self.locate_pane(pane)?;
        let tab = self.tab_mut(tab_id)?;
        tab.panes.retain(|p| p.id != pane);
        match tab.layout.remove(pane) {
            Some(rest) => {
                tab.layout = rest;
                if tab.active_pane == pane {
                    tab.active_pane = tab.layout.first();
                }
            }
            None => {
                self.take_tab(tab_id);
            }
        }
        self.drop_empty_benches();
        Some(bench_id)
    }

    /// Removes a tab and returns it, fixing up its bench's active tab.
    pub fn take_tab(&mut self, tab: TabId) -> Option<Tab> {
        let bench = self.bench_mut(self.bench_of_tab(tab)?)?;
        let index = bench.tabs.iter().position(|t| t.id == tab)?;
        let removed = bench.tabs.remove(index);
        if bench.active_tab == tab
            && let Some(next) = bench.tabs.get(index.min(bench.tabs.len().saturating_sub(1)))
        {
            bench.active_tab = next.id;
        }
        Some(removed)
    }

    /// Moves `tab` to `bench` at `index` (clamped) and makes it active there.
    /// A bench left without tabs is removed.
    pub fn move_tab(&mut self, tab: TabId, bench: BenchId, index: usize) -> bool {
        if self.bench(bench).is_none() {
            return false;
        }
        let Some(moved) = self.take_tab(tab) else { return false };
        let target = self.bench_mut(bench).unwrap();
        let index = index.min(target.tabs.len());
        target.tabs.insert(index, moved);
        target.active_tab = tab;
        self.drop_empty_benches();
        true
    }

    /// Makes `pane` the active pane of its tab, and that tab active in its bench.
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some((bench, tab)) = self.locate_pane(pane) else { return false };
        self.tab_mut(tab).unwrap().active_pane = pane;
        self.bench_mut(bench).unwrap().active_tab = tab;
        true
    }

    fn drop_empty_benches(&mut self) {
        self.benches.retain(|b| !b.tabs.is_empty());
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
    pub active_tab: TabId,
}

impl Bench {
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active_tab).or(self.tabs.first())
    }

    pub fn tab_index(&self, tab: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == tab)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    /// A name the user gave the tab; otherwise the title comes from its active pane.
    pub name: Option<String>,
    /// Tags the user set.
    pub tags: Vec<Tag>,
    /// Tags set by tag rules, recomputed as tools and directories change.
    pub auto_tags: Vec<Tag>,
    /// Commands run here stay in this tab's local logbook only.
    pub private: bool,
    pub layout: PaneNode,
    pub panes: Vec<Pane>,
    pub active_pane: PaneId,
}

impl Tab {
    /// A tab with a single pane.
    pub fn with_pane(pane: Pane) -> Self {
        Self {
            id: TabId::new(),
            name: None,
            tags: Vec::new(),
            auto_tags: Vec::new(),
            private: false,
            layout: PaneNode::Leaf(pane.id),
            active_pane: pane.id,
            panes: vec![pane],
        }
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    pub fn active(&self) -> Option<&Pane> {
        self.pane(self.active_pane)
    }

    /// What the tab bar shows: the user's name, else the active pane's title,
    /// else its tool, else its directory.
    pub fn display_title(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let Some(pane) = self.active() else { return "shell".into() };
        if let Some(title) = pane.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return title.to_string();
        }
        if let Some(tool) = &pane.tool {
            return tool.display();
        }
        pane.cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| pane.cwd.display().to_string())
    }

    /// Every tag on the tab: the user's, then the rules'.
    pub fn all_tags(&self) -> impl Iterator<Item = &Tag> {
        self.tags.iter().chain(self.auto_tags.iter().filter(|a| !self.tags.iter().any(|t| t.label == a.label)))
    }

    /// Splits `target`, putting `pane` after it in direction `dir`.
    pub fn split(&mut self, target: PaneId, pane: Pane, dir: SplitDir) -> bool {
        if !self.layout.split(target, pane.id, dir) {
            return false;
        }
        self.active_pane = pane.id;
        self.panes.push(pane);
        true
    }
}

/// A short coloured label on a tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub label: String,
    pub color: Rgb,
    /// Tint the whole tab, as a warning (for production shells, say).
    pub tint_tab: bool,
}

/// One terminal inside a tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    /// The shell's working directory.
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    /// The title the running program set, if any.
    pub title: Option<String>,
    /// The program in the foreground, when it is not the shell itself.
    pub tool: Option<Tool>,
    /// A command typed at the prompt by a bench load, waiting for Enter.
    pub waiting: Option<String>,
}

impl Pane {
    pub fn new(id: PaneId, cwd: PathBuf) -> Self {
        Self { id, cwd, cols: 80, rows: 24, title: None, tool: None, waiting: None }
    }
}

/// A program running in the foreground of a pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub argv: Vec<String>,
}

impl Tool {
    /// The program name without its path, then its arguments.
    pub fn display(&self) -> String {
        let mut parts = self.argv.iter();
        let Some(first) = parts.next() else { return String::new() };
        let name = first.rsplit('/').next().unwrap_or(first);
        std::iter::once(name).chain(parts.map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDir {
    /// Children side by side, the second to the right.
    Right,
    /// Children stacked, the second below.
    Down,
}

/// A direction for moving between panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// A rectangle in layout units (the whole tab is `0..1` on both axes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const UNIT: Rect = Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
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

    /// Every pane, in reading order.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            PaneNode::Leaf(id) => out.push(*id),
            PaneNode::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    /// Replaces the leaf `target` with a split of `target` and `new`.
    pub fn split(&mut self, target: PaneId, new: PaneId, dir: SplitDir) -> bool {
        match self {
            PaneNode::Leaf(id) if *id == target => {
                *self = PaneNode::Split {
                    dir,
                    ratio: 0.5,
                    first: Box::new(PaneNode::Leaf(target)),
                    second: Box::new(PaneNode::Leaf(new)),
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { first, second, .. } => {
                first.split(target, new, dir) || second.split(target, new, dir)
            }
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

    /// Each pane's rectangle within `area`.
    pub fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.collect_rects(area, &mut out);
        out
    }

    fn collect_rects(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            PaneNode::Leaf(id) => out.push((*id, area)),
            PaneNode::Split { dir, ratio, first, second } => {
                let (a, b) = split_rect(area, *dir, *ratio);
                first.collect_rects(a, out);
                second.collect_rects(b, out);
            }
        }
    }

    /// The pane next to `from` in `dir`: the one sharing the most edge with it,
    /// nearest first.
    pub fn neighbor(&self, from: PaneId, dir: Direction) -> Option<PaneId> {
        let rects = self.rects(Rect::UNIT);
        let (_, r) = *rects.iter().find(|(id, _)| *id == from)?;
        const EPS: f32 = 1e-4;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
        rects
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|(id, o)| {
                let (gap, shared) = match dir {
                    Direction::Left => (r.x - (o.x + o.w), overlap(r.y, r.y + r.h, o.y, o.y + o.h)),
                    Direction::Right => (o.x - (r.x + r.w), overlap(r.y, r.y + r.h, o.y, o.y + o.h)),
                    Direction::Up => (r.y - (o.y + o.h), overlap(r.x, r.x + r.w, o.x, o.x + o.w)),
                    Direction::Down => (o.y - (r.y + r.h), overlap(r.x, r.x + r.w, o.x, o.x + o.w)),
                };
                (gap > -EPS && shared > EPS).then_some((*id, gap, shared))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(id, _, _)| id)
    }

    /// Sets the ratio of the split at `path` (`false` = first child, `true` = second).
    pub fn set_ratio(&mut self, path: &[bool], new: f32) -> bool {
        match (self, path.split_first()) {
            (PaneNode::Split { ratio, .. }, None) => {
                *ratio = new.clamp(0.05, 0.95);
                true
            }
            (PaneNode::Split { first, second, .. }, Some((&go_second, rest))) => {
                if go_second { second.set_ratio(rest, new) } else { first.set_ratio(rest, new) }
            }
            (PaneNode::Leaf(_), _) => false,
        }
    }

    /// The same layout with every ratio clamped to keep panes visible.
    pub fn clamped(&self) -> PaneNode {
        match self {
            PaneNode::Leaf(_) => self.clone(),
            PaneNode::Split { dir, ratio, first, second } => PaneNode::Split {
                dir: *dir,
                ratio: ratio.clamp(0.05, 0.95),
                first: Box::new(first.clamped()),
                second: Box::new(second.clamped()),
            },
        }
    }
}

/// Splits `area` in two along `dir`, giving `ratio` of it to the first part.
pub fn split_rect(area: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect) {
    match dir {
        SplitDir::Right => {
            let w = area.w * ratio;
            (Rect { w, ..area }, Rect { x: area.x + w, w: area.w - w, ..area })
        }
        SplitDir::Down => {
            let h = area.h * ratio;
            (Rect { h, ..area }, Rect { y: area.y + h, h: area.h - h, ..area })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Pane {
        Pane::new(PaneId::new(), "/".into())
    }

    fn bench(tabs: Vec<Tab>) -> Bench {
        Bench {
            id: BenchId::new(),
            name: "main".into(),
            color: BenchColor::Blue,
            badge: None,
            cwd: "/".into(),
            active_tab: tabs[0].id,
            tabs,
        }
    }

    #[test]
    fn removing_last_pane_removes_tab_and_bench() {
        let p = pane();
        let pid = p.id;
        let mut shed = Shed { benches: vec![bench(vec![Tab::with_pane(p)])] };
        assert!(shed.remove_pane(pid).is_some());
        assert!(shed.benches.is_empty());
    }

    #[test]
    fn closing_active_tab_activates_neighbour() {
        let tabs: Vec<Tab> = (0..3).map(|_| Tab::with_pane(pane())).collect();
        let ids: Vec<TabId> = tabs.iter().map(|t| t.id).collect();
        let mut b = bench(tabs);
        b.active_tab = ids[1];
        let mut shed = Shed { benches: vec![b] };
        shed.take_tab(ids[1]);
        assert_eq!(shed.benches[0].active_tab, ids[2]);
        shed.take_tab(ids[2]);
        assert_eq!(shed.benches[0].active_tab, ids[0]);
    }

    #[test]
    fn moving_the_only_tab_removes_its_bench() {
        let a = bench(vec![Tab::with_pane(pane())]);
        let b = bench(vec![Tab::with_pane(pane())]);
        let (tab, target) = (a.tabs[0].id, b.id);
        let mut shed = Shed { benches: vec![a, b] };
        assert!(shed.move_tab(tab, target, 0));
        assert_eq!(shed.benches.len(), 1);
        assert_eq!(shed.benches[0].tabs[0].id, tab);
        assert_eq!(shed.benches[0].active_tab, tab);
    }

    #[test]
    fn split_and_collapse() {
        let (a, b) = (PaneId::new(), PaneId::new());
        let mut node = PaneNode::Leaf(a);
        assert!(node.split(a, b, SplitDir::Right));
        assert_eq!(node.leaves(), vec![a, b]);
        assert_eq!(node.remove(a), Some(PaneNode::Leaf(b)));
    }

    #[test]
    fn set_ratio_by_path() {
        let (a, b, c) = (PaneId::new(), PaneId::new(), PaneId::new());
        let mut node = PaneNode::Leaf(a);
        node.split(a, b, SplitDir::Right);
        node.split(b, c, SplitDir::Down);
        assert!(node.set_ratio(&[true], 0.3));
        assert!(node.set_ratio(&[], 2.0));
        let PaneNode::Split { ratio, second, .. } = &node else { panic!() };
        assert_eq!(*ratio, 0.95);
        assert!(matches!(**second, PaneNode::Split { ratio, .. } if ratio == 0.3));
        assert!(!node.set_ratio(&[false], 0.5), "a leaf has no ratio");
    }

    #[test]
    fn neighbours_follow_geometry() {
        // a | b
        // --+--
        //   c (under b)
        let (a, b, c) = (PaneId::new(), PaneId::new(), PaneId::new());
        let mut node = PaneNode::Leaf(a);
        node.split(a, b, SplitDir::Right);
        node.split(b, c, SplitDir::Down);
        assert_eq!(node.neighbor(a, Direction::Right), Some(b));
        assert_eq!(node.neighbor(b, Direction::Down), Some(c));
        assert_eq!(node.neighbor(c, Direction::Up), Some(b));
        assert_eq!(node.neighbor(c, Direction::Left), Some(a));
        assert_eq!(node.neighbor(a, Direction::Left), None);
    }

    #[test]
    fn display_title_prefers_name_then_title_then_tool() {
        let mut tab = Tab::with_pane(Pane::new(PaneId::new(), "/home/me/code/api".into()));
        assert_eq!(tab.display_title(), "api");
        tab.panes[0].tool = Some(Tool { argv: vec!["/usr/bin/nvim".into(), "main.rs".into()] });
        assert_eq!(tab.display_title(), "nvim main.rs");
        tab.panes[0].title = Some("vim: main.rs".into());
        assert_eq!(tab.display_title(), "vim: main.rs");
        tab.name = Some("api".into());
        assert_eq!(tab.display_title(), "api");
    }
}

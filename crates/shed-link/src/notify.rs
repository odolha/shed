//! Notifications, from how the tree changes: an AI tab starts waiting for
//! an answer or finishes a turn, a command that ran a minute or more ends.
//! Nothing from private tabs.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use shed_api::wire::{Notification, NotificationKind};
use shed_model::{AiActivity, PaneId, Shed, TabId};

/// A command this long or longer is worth hearing about when it ends.
pub(crate) const LONG_COMMAND: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct Notifier {
    /// Nothing is said about the first tree: it is how things were.
    seen: bool,
    activity: HashMap<TabId, AiActivity>,
    /// Each pane's foreground tool, and since when it runs.
    tools: HashMap<PaneId, (String, Instant)>,
}

impl Notifier {
    /// What changed since the last tree that is worth a notification.
    pub fn changed(&mut self, tree: &Shed, now: Instant) -> Vec<Notification> {
        let mut out = Vec::new();
        let mut activity = HashMap::new();
        let mut tools = HashMap::new();
        for bench in &tree.benches {
            for tab in &bench.tabs {
                let where_ = format!("{} · {}", bench.name, tab.display_title());
                let at = |kind, title: &str| Notification {
                    kind,
                    title: title.into(),
                    body: where_.clone(),
                    bench: Some(bench.id.to_string()),
                    tab: Some(tab.id.to_string()),
                };
                if let Some(ai) = tab.ai_info() {
                    let before = self.activity.get(&tab.id).copied();
                    activity.insert(tab.id, ai.overall_activity());
                    if self.seen && !tab.private {
                        match (before, ai.overall_activity()) {
                            (Some(b), AiActivity::Waiting) if b != AiActivity::Waiting => out.push(at(NotificationKind::AiWaiting, "Claude waits for an answer")),
                            // (Its turn over, work it started may go on: no word of that.)
                            (Some(AiActivity::Working), AiActivity::Idle | AiActivity::Background(_)) => out.push(at(NotificationKind::AiDone, "Claude is done")),
                            _ => {}
                        }
                    }
                }
                for pane in &tab.panes {
                    let tool = pane.tool.as_ref().map(|t| t.display());
                    let before = self.tools.get(&pane.id);
                    match (&tool, before) {
                        (Some(t), Some((b, since))) if t == b => {
                            tools.insert(pane.id, (t.clone(), *since));
                        }
                        (Some(t), _) => {
                            tools.insert(pane.id, (t.clone(), now));
                        }
                        (None, _) => {}
                    }
                    if let Some((was, since)) = before
                        && tool.as_ref() != Some(was)
                        && self.seen
                        && !tab.private
                        && now.duration_since(*since) >= LONG_COMMAND
                    {
                        out.push(at(NotificationKind::CommandDone, &format!("{} finished", short(was))));
                    }
                }
            }
        }
        self.activity = activity;
        self.tools = tools;
        self.seen = true;
        out
    }
}

/// A command line cut to a readable length.
fn short(cmd: &str) -> String {
    let mut s: String = cmd.chars().take(60).collect();
    if cmd.chars().count() > 60 {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use shed_model::{Bench, BenchColor, BenchId, Pane, Tab, Tool};

    use super::*;

    fn tree(ai: AiActivity, tool: Option<&str>, private: bool) -> (Shed, TabId, PaneId) {
        let pane = Pane::new(PaneId::from_bytes([1; 16]), "/srv".into());
        let mut term = Tab::with_pane(pane);
        term.id = TabId::from_bytes([2; 16]);
        term.private = private;
        term.panes[0].tool = tool.map(|t| Tool { argv: t.split(' ').map(String::from).collect() });
        let mut ai_tab = Tab::ai("/srv".into());
        ai_tab.id = TabId::from_bytes([3; 16]);
        if let shed_model::TabKind::Ai(a) = &mut ai_tab.kind {
            a.activity = ai;
        }
        let bench = Bench {
            id: BenchId::from_bytes([4; 16]),
            name: "api".into(),
            color: BenchColor::Blue,
            badge: None,
            cwd: "/srv".into(),
            active_tab: term.id,
            tabs: vec![term, ai_tab],
            aliases: Default::default(),
            closing: false,
            launcher: "api".into(),
            window: None,
            started: false,
            links: Vec::new(),
            bookmarks: Vec::new(),
        };
        (Shed { benches: vec![bench], ..Default::default() }, TabId::from_bytes([3; 16]), PaneId::from_bytes([1; 16]))
    }

    #[test]
    fn what_is_worth_a_notification() {
        let mut n = Notifier::default();
        let t0 = Instant::now();
        let (idle, ai, _) = tree(AiActivity::Idle, None, false);
        assert!(n.changed(&idle, t0).is_empty());
        let (working, ..) = tree(AiActivity::Working, Some("cargo build"), false);
        assert!(n.changed(&working, t0).is_empty());
        let (waiting, ..) = tree(AiActivity::Waiting, Some("cargo build"), false);
        let said = n.changed(&waiting, t0 + Duration::from_secs(5));
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].kind, NotificationKind::AiWaiting);
        assert_eq!(said[0].body, "api · Claude · srv");
        assert_eq!(said[0].tab, Some(ai.to_string()));
        let (working, ..) = tree(AiActivity::Working, Some("cargo build"), false);
        assert!(n.changed(&working, t0 + Duration::from_secs(10)).is_empty());
        // The turn ends, and so does a command that ran over a minute.
        let (done, ..) = tree(AiActivity::Idle, None, false);
        let said = n.changed(&done, t0 + Duration::from_secs(90));
        let kinds: Vec<_> = said.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [NotificationKind::CommandDone, NotificationKind::AiDone]);
        assert_eq!(said[0].title, "cargo build finished");
        // A short command says nothing, nor does a private tab's.
        let (quick, ..) = tree(AiActivity::Idle, Some("ls"), false);
        n.changed(&quick, t0 + Duration::from_secs(100));
        assert!(n.changed(&done, t0 + Duration::from_secs(101)).is_empty());
        let (private, ..) = tree(AiActivity::Idle, Some("make"), true);
        n.changed(&private, t0 + Duration::from_secs(110));
        let (private_done, ..) = tree(AiActivity::Idle, None, true);
        assert!(n.changed(&private_done, t0 + Duration::from_secs(300)).is_empty());
    }

    /// A turn that ends with work going on in the background is done, once:
    /// that work ending says nothing (Claude, woken by it, says so again).
    #[test]
    fn background_work_is_no_news() {
        let mut n = Notifier::default();
        let t0 = Instant::now();
        let (working, ..) = tree(AiActivity::Working, None, false);
        n.changed(&working, t0);
        let (waiting_on, ..) = tree(AiActivity::Background(2), None, false);
        let said = n.changed(&waiting_on, t0);
        assert_eq!(said.iter().map(|s| s.kind).collect::<Vec<_>>(), [NotificationKind::AiDone]);
        let (fewer, ..) = tree(AiActivity::Background(1), None, false);
        assert!(n.changed(&fewer, t0).is_empty());
        let (idle, ..) = tree(AiActivity::Idle, None, false);
        assert!(n.changed(&idle, t0).is_empty());
    }

    #[test]
    fn a_private_ai_tab_says_nothing() {
        let private_ai = |activity| {
            let (mut tree, ai, _) = tree(activity, None, false);
            tree.tab_mut(ai).unwrap().private = true;
            tree
        };
        let mut n = Notifier::default();
        let t0 = Instant::now();
        n.changed(&private_ai(AiActivity::Working), t0);
        assert!(n.changed(&private_ai(AiActivity::Waiting), t0).is_empty());
        n.changed(&private_ai(AiActivity::Working), t0);
        assert!(n.changed(&private_ai(AiActivity::Idle), t0).is_empty());
    }
}

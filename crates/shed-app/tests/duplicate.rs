//! Duplicating a tab: the copy opens beside it, laid out and set up the
//! same, with nothing of what runs in the original.

mod common;

use common::{Client, start_in};
use shed_model::{BenchId, PaneNode, PaneSetup, Project, Rgb, Shed, SplitDir, TabDivider, Tag, groups};
use shed_proto::{AiChange, AliasTarget, ClientMsg, Frame, KeeperMsg, ShowTarget};

fn tree_until(c: &mut Client, what: &str, ok: impl Fn(&Shed) -> bool) -> Shed {
    if ok(&c.tree) {
        return c.tree.clone();
    }
    c.until(what, |f, _| match f {
        Frame::Keeper(KeeperMsg::TreeChanged(t)) if ok(t) => Some(t.clone()),
        _ => None,
    })
}

fn show_and_attach(c: &mut Client, target: ShowTarget) -> BenchId {
    c.msg(ClientMsg::Show(target));
    let bench = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    bench
}

fn tab_count(bench: BenchId, n: usize) -> impl Fn(&Shed) -> bool {
    move |t| t.bench(bench).is_some_and(|b| b.tabs.len() == n)
}

/// The shape of a layout: its splits' directions and ratios, without ids.
fn shape(node: &PaneNode) -> String {
    match node {
        PaneNode::Leaf(_) => "()".into(),
        PaneNode::Split { dir, ratio, first, second } => format!("({dir:?} {ratio} {} {})", shape(first), shape(second)),
    }
}

#[test]
fn a_split_terminal_tab_is_duplicated_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let (one, two) = (work.join("one"), work.join("two"));
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    let bench = show_and_attach(&mut c, ShowTarget::NewBench { name: Some("dup".into()), cwd: Some(work.clone()) });
    let tree = tree_until(&mut c, "bench", |t| t.bench(bench).is_some());
    let a = tree.bench(bench).unwrap().tabs[0].id;
    let p1 = tree.bench(bench).unwrap().tabs[0].active_pane;

    // Two panes side by side: the first moved to `two`, the second in `one`
    // with a startup and teardown of its own.
    c.msg(ClientMsg::Split { pane: p1, dir: SplitDir::Right, cwd: Some(one.clone()) });
    let tree = tree_until(&mut c, "split", |t| t.tab(a).is_some_and(|t| t.panes.len() == 2));
    let p2 = tree.tab(a).unwrap().layout.leaves()[1];
    c.prompt(p1);
    c.input(p1, "cd two\n");
    tree_until(&mut c, "cd", |t| t.pane(p1).is_some_and(|p| p.cwd == two));
    let setup = PaneSetup { startup: vec!["echo STARTED-$((6*7))".into()], teardown: vec!["echo bye".into()], ..Default::default() };
    c.msg(ClientMsg::SetPaneSetup { pane: p2, setup: setup.clone() });
    c.msg(ClientMsg::RenameTab { tab: a, name: Some("api".into()) });
    let tags = vec![Tag { label: "mine-tag".into(), color: Rgb { r: 200, g: 40, b: 40 }, tint_tab: true }];
    c.msg(ClientMsg::SetTags { tab: a, tags: tags.clone() });
    c.msg(ClientMsg::SetPrivate { tab: a, private: true });
    let aliases: shed_model::Aliases = [("hi".to_string(), "echo hi".to_string())].into();
    c.msg(ClientMsg::SetAliases { level: AliasTarget::Tab(a), aliases: aliases.clone() });
    // A second tab, in a group of its own: [a] [b].
    c.msg(ClientMsg::NewTab { bench, cwd: None });
    let tree = tree_until(&mut c, "two tabs", tab_count(bench, 2));
    let b = tree.bench(bench).unwrap().tabs[1].id;
    c.msg(ClientMsg::SetTabDivider { tab: b, divider: TabDivider::Space });
    let tree = tree_until(&mut c, "set up", |t| {
        t.tab(b).is_some_and(|t| t.divider == TabDivider::Space)
            && t.tab(a).is_some_and(|t| t.private && t.aliases == aliases && t.name.as_deref() == Some("api") && t.tags == tags)
            && t.pane(p2).is_some_and(|p| p.setup == setup)
    });
    let original = tree.tab(a).unwrap().clone();

    // To the right: in a's group, shown.
    c.msg(ClientMsg::DuplicateTab { tab: a, after: true });
    let tree = tree_until(&mut c, "a copy right of a", tab_count(bench, 3));
    let now = groups::of(tree.bench(bench).unwrap());
    let r = now[1].0;
    assert_eq!(now, [(a, TabDivider::None), (r, TabDivider::None), (b, TabDivider::Space)]);
    assert_eq!(tree.bench(bench).unwrap().active_tab, r);
    let copy = tree.tab(r).unwrap().clone();
    assert_eq!(copy.name.as_deref(), Some("api"));
    assert_eq!(copy.tags, tags);
    assert!(copy.private);
    assert_eq!(copy.aliases, aliases);
    assert_eq!(shape(&copy.layout), shape(&original.layout));
    let leaves = copy.layout.leaves();
    assert_eq!(leaves.len(), 2);
    assert!(leaves.iter().all(|p| !original.layout.contains(*p)), "fresh panes");
    let (c1, c2) = (copy.pane(leaves[0]).unwrap().clone(), copy.pane(leaves[1]).unwrap().clone());
    assert_eq!(c1.cwd, two);
    assert_eq!(c2.cwd, one);
    assert_eq!(c1.setup, PaneSetup::default());
    assert_eq!(c2.setup, setup);
    // The shells start where the tree says, and the startup does not run.
    c.prompt(leaves[1]);
    c.input(leaves[1], "pwd; echo done-$((1+1))\n");
    let out = c.output_until(leaves[1], "done-2");
    assert!(out.contains(&one.display().to_string()), "{out:?}");
    c.prompt(leaves[0]);
    c.input(leaves[0], "pwd; echo done-$((2+2))\n");
    let out = c.output_until(leaves[0], "done-4");
    assert!(out.contains(&two.display().to_string()), "{out:?}");
    assert!(!c.seen.get(&leaves[1]).is_some_and(|s| s.contains("STARTED-42")), "startup ran");

    // To the left of a group's first tab: first in that group.
    c.msg(ClientMsg::DuplicateTab { tab: b, after: false });
    let tree = tree_until(&mut c, "a copy left of b", tab_count(bench, 4));
    let now = groups::of(tree.bench(bench).unwrap());
    let l = now[2].0;
    assert_eq!(now, [(a, TabDivider::None), (r, TabDivider::None), (l, TabDivider::Space), (b, TabDivider::None)]);
    assert_eq!(tree.bench(bench).unwrap().active_tab, l);
    assert_eq!(tree.tab(l).unwrap().panes[0].cwd, work);

    // The original tab, left of itself: before it, in the same group.
    c.msg(ClientMsg::DuplicateTab { tab: a, after: false });
    let tree = tree_until(&mut c, "a copy left of a", tab_count(bench, 5));
    let first = tree.bench(bench).unwrap().tabs[0].clone();
    assert_ne!(first.id, a);
    assert_eq!(tree.bench(bench).unwrap().tabs[1].id, a);
    assert_eq!(first.name.as_deref(), Some("api"));
    assert_eq!(first.panes.len(), 2);
    assert_eq!(tree.bench(bench).unwrap().active_tab, first.id);
}

#[test]
fn ai_and_project_tabs_are_duplicated_with_their_settings() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let extra = dir.path().join("extra");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(work.join("a.txt"), "a").unwrap();
    std::fs::write(work.join("b.txt"), "b").unwrap();
    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    let bench = show_and_attach(&mut c, ShowTarget::NewBench { name: Some("dup".into()), cwd: Some(work.clone()) });
    tree_until(&mut c, "bench", |t| t.bench(bench).is_some());

    // An AI tab with its settings, a session shown, the browser and
    // something written in its prompt.
    c.msg(ClientMsg::NewAiTab { bench, folder: work.clone() });
    let tree = tree_until(&mut c, "ai tab", tab_count(bench, 2));
    let ai = tree.bench(bench).unwrap().tabs.iter().find(|t| t.ai_info().is_some()).unwrap().id;
    let settings = AiChange::Settings {
        model: Some(Some("opus".into())),
        mode: Some("plan".into()),
        effort: Some(Some("high".into())),
        follow: Some(true),
        browser: Some(true),
        add_dirs: Some(vec![extra.clone()]),
    };
    c.msg(ClientMsg::AiChange { tab: ai, change: settings });
    c.msg(ClientMsg::AiOpenSession { tab: ai, session: Some("session-1".into()) });
    c.msg(ClientMsg::AiChange { tab: ai, change: AiChange::Unsent("half a thought".into()) });
    tree_until(&mut c, "ai set up", |t| {
        t.tab(ai).and_then(|t| t.ai_info()).is_some_and(|a| a.browser && a.follow && a.session.as_deref() == Some("session-1") && a.unsent == "half a thought")
    });
    c.msg(ClientMsg::DuplicateTab { tab: ai, after: true });
    let tree = tree_until(&mut c, "ai copy", tab_count(bench, 3));
    let tabs = &tree.bench(bench).unwrap().tabs;
    let at = tabs.iter().position(|t| t.id == ai).unwrap();
    let copy = &tabs[at + 1];
    assert_eq!(tree.bench(bench).unwrap().active_tab, copy.id);
    let x = copy.ai_info().expect("an AI tab");
    assert_eq!(x.folder, work);
    assert_eq!(x.add_dirs, std::slice::from_ref(&extra));
    assert_eq!((x.model.as_deref(), x.mode.as_str(), x.effort.as_deref(), x.follow), (Some("opus"), "plan", Some("high"), true));
    assert_eq!(x.session, None);
    assert_eq!(x.fork_of.as_deref(), Some("session-1"));
    assert!(!x.browser);
    assert!(x.unsent.is_empty() && x.queued.is_empty() && x.drafts.is_empty());
    // The original keeps the browser and its session.
    let o = tree.tab(ai).unwrap().ai_info().unwrap();
    assert!(o.browser && o.session.as_deref() == Some("session-1"));

    // A project tab with two files open, the second shown.
    c.msg(ClientMsg::NewProjectTab { bench, root: work.clone() });
    let tree = tree_until(&mut c, "project tab", tab_count(bench, 4));
    let p = tree.bench(bench).unwrap().tabs.iter().find(|t| t.project_info().is_some()).unwrap().id;
    let project = Project { root: work.clone(), open: vec![work.join("a.txt"), work.join("b.txt")], active: Some(1), exclude: vec!["target".into()], ..Default::default() };
    c.msg(ClientMsg::SetProject { tab: p, project: project.clone() });
    tree_until(&mut c, "files open", |t| t.tab(p).and_then(|t| t.project_info()).is_some_and(|pr| *pr == project));
    c.msg(ClientMsg::DuplicateTab { tab: p, after: false });
    let tree = tree_until(&mut c, "project copy", tab_count(bench, 5));
    let tabs = &tree.bench(bench).unwrap().tabs;
    let at = tabs.iter().position(|t| t.id == p).unwrap();
    let copy = &tabs[at - 1];
    assert_eq!(copy.project_info(), Some(&project));
    assert!(copy.panes.is_empty());
    assert_eq!(tree.bench(bench).unwrap().active_tab, copy.id);
}

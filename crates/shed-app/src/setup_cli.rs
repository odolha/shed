//! `shed setup` and `shed alias`: a tab's startup, teardown and aliases from the shell.

use std::time::Duration;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use shed_model::config::resolve_aliases;
use shed_model::{Aliases, BenchId, PaneId, PaneSetup, Shed, TabId};
use shed_proto::{AliasTarget, ClientKind, ClientMsg, Frame, KeeperMsg};
use shed_save::{expand_home, home_relative};

use crate::keeper_conn;

/// The tab and bench this process runs in.
fn here() -> anyhow::Result<(TabId, BenchId)> {
    let tab = std::env::var("SHED_TAB").ok().and_then(|t| t.parse().ok()).map(TabId);
    let bench = std::env::var("SHED_BENCH").ok().and_then(|b| b.parse().ok()).map(BenchId);
    match (tab, bench) {
        (Some(t), Some(b)) => Ok((t, b)),
        _ => bail!("not inside a Shed tab"),
    }
}

fn tree() -> anyhow::Result<Shed> {
    crate::request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s.tree),
        _ => None,
    })
}

/// Sends messages and waits a moment for errors.
fn send_all(msgs: Vec<ClientMsg>) -> anyhow::Result<()> {
    let (conn, rx) = keeper_conn::connect(ClientKind::Cli, false)?;
    for m in msgs {
        conn.send_msg(m);
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(Frame::Keeper(KeeperMsg::Error(e))) => bail!("{e}"),
            Ok(_) => {}
            Err(async_channel::TryRecvError::Closed) => break,
            Err(async_channel::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    Ok(())
}

pub fn show_setup() -> anyhow::Result<()> {
    let (tab_id, _) = here()?;
    let tree = tree()?;
    let tab = tree.tab(tab_id).context("this tab is not in the keeper")?;
    println!("tab {:?}, {} pane(s)", tab.display_title(), tab.panes.len());
    for (i, id) in tab.layout.leaves().iter().enumerate() {
        let Some(p) = tab.pane(*id) else { continue };
        let running = p.tool.as_ref().map(|t| format!("   running: {}", t.display())).unwrap_or_default();
        println!("\npane {}   now in {}{running}", i + 1, home_relative(&p.cwd));
        let start = p.setup.start_in.as_deref().map(home_relative).unwrap_or_else(|| "where it was last".into());
        println!("  start in   {start}");
        print_list("  startup  ", &p.setup.startup, "(whatever was running when saved, by the rack)");
        if p.setup.wait && !p.setup.startup.is_empty() {
            println!("             (typed, waiting for Enter)");
        }
        print_list("  teardown ", &p.setup.teardown, "(stop the running tool)");
        if !p.setup.after.is_empty() {
            let names: Vec<String> = p.setup.after.iter().map(|d| pane_name(&tree, *d)).collect();
            println!("  after      {}", names.join(", "));
        }
        match &p.setup.ready_when {
            Some(text) => println!("  ready when its output shows {text:?}"),
            None => println!("  ready when its startup finishes"),
        }
    }
    println!("\nEdit with `shed setup --edit`.");
    Ok(())
}

/// A pane as `shed setup` names it: its tab, and which pane if there are several.
fn pane_name(tree: &Shed, pane: PaneId) -> String {
    let Some(tab) = tree.locate_pane(pane).and_then(|(_, t)| tree.tab(t)) else { return "(a pane no longer here)".into() };
    let leaves = tab.layout.leaves();
    match leaves.iter().position(|p| *p == pane) {
        Some(i) if leaves.len() > 1 => format!("{} · pane {}", tab.display_title(), i + 1),
        _ => tab.display_title(),
    }
}

fn print_list(label: &str, items: &[String], empty: &str) {
    if items.is_empty() {
        println!("{label} {empty}");
    }
    for (i, item) in items.iter().enumerate() {
        println!("{} {item}", if i == 0 { label.to_string() } else { " ".repeat(label.len()) });
    }
}

/// The editable form of a tab's setup.
#[derive(Serialize, Deserialize)]
struct EditFile {
    #[serde(default, rename = "pane")]
    panes: Vec<EditPane>,
    /// This tab's own aliases.
    #[serde(default)]
    aliases: Aliases,
}

#[derive(Serialize, Deserialize)]
struct EditPane {
    id: PaneId,
    #[serde(default)]
    start_in: String,
    #[serde(default)]
    startup: Vec<String>,
    #[serde(default)]
    wait: bool,
    #[serde(default)]
    teardown: Vec<String>,
    /// Ready (for panes that start after it) once the output shows this;
    /// empty: once its startup finishes.
    #[serde(default)]
    ready_when: String,
}

pub fn edit_setup() -> anyhow::Result<()> {
    let (tab_id, _) = here()?;
    let tree = tree()?;
    let tab = tree.tab(tab_id).context("this tab is not in the keeper")?;
    let file = EditFile {
        panes: tab
            .layout
            .leaves()
            .iter()
            .filter_map(|id| tab.pane(*id))
            .map(|p| EditPane {
                id: p.id,
                start_in: p.setup.start_in.as_deref().map(home_relative).unwrap_or_default(),
                startup: p.setup.startup.clone(),
                wait: p.setup.wait,
                teardown: p.setup.teardown.clone(),
                ready_when: p.setup.ready_when.clone().unwrap_or_default(),
            })
            .collect(),
        aliases: tab.aliases.clone(),
    };
    let header = format!(
        "# Setup of the tab {:?}. Saved into the bench when you close the editor.\n\
         #\n\
         # Per pane, in the order of the layout:\n\
         #   start_in  where it starts when the bench loads (empty: where it was last)\n\
         #   startup   commands run in order when the bench loads, instead of what\n\
         #             was running when it was saved; wait = true types them instead\n\
         #   teardown  commands run when the bench is closed, after the running\n\
         #             tool is stopped (for example \"docker compose down\")\n\
         # [aliases] are this tab's own; see `shed alias` for the bench's and global ones.\n\n",
        tab.display_title()
    );
    let path = std::env::temp_dir().join(format!("shed-setup-{}.toml", std::process::id()));
    std::fs::write(&path, header + &toml::to_string(&file)?)?;

    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "nano".into());
    let status = std::process::Command::new("sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("sh").arg(&path).status()?;
    if !status.success() {
        bail!("the editor exited with {status}; nothing changed");
    }
    let text = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);
    let edited: EditFile = toml::from_str(&text).context("the setup did not parse; nothing changed")?;

    let mut msgs = Vec::new();
    for ep in edited.panes {
        if tab.pane(ep.id).is_none() {
            continue;
        }
        let start_in = Some(ep.start_in.trim()).filter(|s| !s.is_empty()).map(expand_home);
        let clean = |v: Vec<String>| v.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
        let after = tab.pane(ep.id).map(|p| p.setup.after.clone()).unwrap_or_default();
        let ready_when = Some(ep.ready_when.trim().to_string()).filter(|s| !s.is_empty());
        let setup = PaneSetup { start_in, startup: clean(ep.startup), wait: ep.wait, teardown: clean(ep.teardown), after, ready_when };
        msgs.push(ClientMsg::SetPaneSetup { pane: ep.id, setup });
    }
    msgs.push(ClientMsg::SetAliases { level: AliasTarget::Tab(tab_id), aliases: edited.aliases });
    send_all(msgs)?;
    println!("Setup saved.");
    Ok(())
}

/// Which level `shed alias add/rm` works on.
#[derive(Clone, Copy, Debug)]
pub enum Level {
    Global,
    Bench,
    Tab,
    /// The shell's own, in the file its startup files source.
    Shell,
}

pub fn list_aliases() -> anyhow::Result<()> {
    let tree = tree()?;
    let (shell, bench, tab) = match here() {
        Ok((tab_id, bench_id)) => {
            let tab = tree.tab(tab_id);
            let shell = tab.and_then(|t| t.active()).map(|p| p.shell_aliases.clone()).unwrap_or_default();
            let bench = tree.bench(bench_id).map(|b| b.aliases.clone()).unwrap_or_default();
            (shell, bench, tab.map(|t| t.aliases.clone()).unwrap_or_default())
        }
        Err(_) => (Aliases::new(), Aliases::new(), Aliases::new()),
    };
    let rows = resolve_aliases(&shell, &tree.aliases, &bench, &tab);
    if rows.is_empty() {
        println!("No aliases. Add one with `shed alias add NAME COMMAND`.");
    }
    let width = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    for (name, value, level, hidden) in rows {
        let over = if hidden.is_empty() {
            String::new()
        } else {
            format!("   (over {})", hidden.iter().map(|l| l.name()).collect::<Vec<_>>().join(", "))
        };
        println!("{:<6}  {name:<width$}  {value}{over}", level.name());
    }
    Ok(())
}

pub fn change_alias(name: &str, value: Option<&str>, level: Option<Level>) -> anyhow::Result<()> {
    if let Some(Level::Shell) = level {
        let msg = ClientMsg::SetShellAlias { name: name.to_string(), value: value.map(str::to_string) };
        let done = crate::request(msg, |m| match m {
            KeeperMsg::Done(text) => Some(text),
            _ => None,
        })?;
        println!("{done}");
        return Ok(());
    }
    let tree = tree()?;
    let inside = here().ok();
    // Inside Shed the narrowest level is the default, so nothing else changes by surprise.
    let level = level.unwrap_or(if inside.is_some() { Level::Tab } else { Level::Global });
    let (target, mut aliases): (AliasTarget, Aliases) = match (level, inside) {
        (Level::Global, _) => (AliasTarget::Global, tree.aliases.clone()),
        (Level::Bench, Some((_, b))) => (AliasTarget::Bench(b), tree.bench(b).map(|b| b.aliases.clone()).unwrap_or_default()),
        (Level::Tab, Some((t, _))) => (AliasTarget::Tab(t), tree.tab(t).map(|t| t.aliases.clone()).unwrap_or_default()),
        (_, None) => bail!("--bench and --tab only work inside Shed"),
        (Level::Shell, _) => unreachable!("handled above"),
    };
    match value {
        Some(v) => {
            if name.is_empty() || name.contains([' ', '=', '\t', '/']) {
                bail!("{name:?} is not a valid alias name");
            }
            aliases.insert(name.to_string(), v.to_string());
        }
        None => {
            if aliases.remove(name).is_none() {
                bail!("no {} alias named {name:?}", level_name(level));
            }
        }
    }
    send_all(vec![ClientMsg::SetAliases { level: target, aliases }])?;
    let what = if value.is_some() { "set" } else { "removed" };
    println!("{name}: {what} for {}; shells pick it up at their next prompt.", level_name(level));
    Ok(())
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Global => "everything (global)",
        Level::Bench => "this bench",
        Level::Tab => "this tab",
        Level::Shell => "the shell",
    }
}

//! `shed`: the bench windows, plus the CLI.

#[cfg(feature = "gui")]
mod actions;
#[cfg(feature = "gui")]
mod ai_text;
#[cfg(feature = "gui")]
mod ai_view;
#[cfg(feature = "gui")]
mod assets;
mod browser;
#[cfg(feature = "gui")]
mod gui;
#[cfg(feature = "gui")]
mod folder_browser;
#[cfg(feature = "gui")]
mod picture;
#[cfg(feature = "gui")]
mod bench_window;
#[cfg(feature = "gui")]
mod changes_ui;
#[cfg(feature = "gui")]
mod emoji;
#[cfg(feature = "gui")]
mod line_editor;
#[cfg(feature = "gui")]
mod links;
mod logbook;
#[cfg(feature = "gui")]
mod logbook_view;
#[cfg(feature = "gui")]
mod md_draw;
#[cfg(feature = "gui")]
mod md_view;
#[cfg(all(feature = "gui", target_os = "macos"))]
mod macos;
#[cfg(feature = "gui")]
mod menu;
#[cfg(feature = "gui")]
mod notify;
#[cfg(feature = "gui")]
mod pegboard;
#[cfg(feature = "gui")]
mod project_view;
#[cfg(feature = "gui")]
mod prompt;
#[cfg(feature = "gui")]
mod pulse;
#[cfg(feature = "gui")]
mod workspace_api;
#[cfg(feature = "gui")]
mod scrollbar;
#[cfg(all(feature = "gui", target_os = "linux"))]
mod selection;
#[cfg(feature = "gui")]
mod settings;
mod setup_cli;
#[cfg(feature = "gui")]
mod settings_panel;
#[cfg(feature = "gui")]
mod setup_panel;
#[cfg(feature = "gui")]
mod shed_window;
#[cfg(feature = "gui")]
mod state;
#[cfg(feature = "gui")]
mod terminal_view;
#[cfg(feature = "gui")]
mod text_view;
#[cfg(feature = "gui")]
mod theme;
#[cfg(feature = "gui")]
mod window_place;

use std::time::Duration;

use anyhow::bail;
use clap::{Parser, Subcommand};
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, ShowTarget};
use tracing_subscriber::EnvFilter;

use shed_client::cli::request;
use shed_client::{cli, keeper_conn, remote};


#[derive(Parser)]
#[command(name = "shed", version, about = "A terminal whose shells outlive its windows")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum TagAction {
    /// Add a tag, or change its colour.
    Add {
        label: String,
        /// A name (red, orange, yellow, green, blue, violet, teal, grey) or #rrggbb.
        #[arg(long)]
        color: Option<String>,
        /// Tint the whole tab.
        #[arg(long)]
        tint: bool,
    },
    Remove {
        label: String,
    },
    List,
}

#[derive(Subcommand)]
enum RemoteAction {
    /// Opens a remote tab in this bench (run inside Shed): a shell on HOST
    /// (as ssh takes it) that keeps running when the bench closes.
    New {
        host: String,
        #[arg(long)]
        bench: Option<String>,
        /// Joins a shell HOST keeps already (an ID from `shed remote list`),
        /// as another PC opened it, instead of starting one.
        #[arg(long)]
        join: Option<String>,
    },
    /// Lists the shells HOST keeps, which `new --join` can join.
    List { host: String },
    /// What a remote tab runs: joins the shell kept for ID on HOST.
    #[command(hide = true)]
    Connect { host: String, id: String },
}

#[derive(Subcommand)]
enum BackupAction {
    /// Lists the backups, oldest first.
    List,
    /// Puts a backup's files back (what is there now is backed up first).
    /// Shed must be stopped (`shed stop`).
    Restore { name: String },
}

#[derive(Subcommand)]
enum BrowserAction {
    /// Writes Shed's browser extension and registers its native host with
    /// the browsers found (Chrome, Chromium, Brave, Edge, Vivaldi).
    Setup {
        /// Register with these browser profile folders instead.
        #[arg(long)]
        profile: Vec<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Open a new bench in its own window.
    New {
        /// The bench's name (default: the next free one).
        #[arg(long)]
        name: Option<String>,
        /// The bench's directory (default: home).
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
    },
    /// Open a bench by name: raise its window, reattach it, or load it from
    /// its saved file.
    Load {
        name: String,
        /// Loading from its file: shells only, in their folders with their
        /// scrollback; no startup commands, no programs started again (the
        /// default, unless `start_on_open` is set).
        #[arg(long, conflicts_with = "start")]
        quiet: bool,
        /// Loading from its file: run its startup, and start again what ran.
        #[arg(long)]
        start: bool,
        /// Another workspace's bench (its id or name at the nexus this
        /// desktop is signed in to): Shed's window switches to it first.
        #[arg(long, conflicts_with_all = ["quiet", "start"])]
        workspace: Option<String>,
    },
    /// Hide a running bench: its window closes, its tools keep running.
    Hide { name: String },
    /// Save a running bench now (it also saves itself every minute).
    Save {
        name: String,
        /// Also save a copy under this name, as a template.
        #[arg(long = "as")]
        as_name: Option<String>,
    },
    /// Delete a saved bench, stopping it if it runs.
    Delete {
        name: String,
        /// Delete without asking.
        #[arg(long)]
        yes: bool,
    },
    /// List benches: open, hidden and closed.
    List,
    /// Open a folder as a project tab in this bench (run inside Shed). The
    /// folder defaults to the Git repository you are in, else this folder.
    Project {
        dir: Option<std::path::PathBuf>,
        /// Into this bench instead (by name).
        #[arg(long)]
        bench: Option<String>,
    },
    /// Open an AI tab (Claude Code) in this bench, working in DIR (by
    /// default the Git repository you are in, else this folder).
    Ai {
        dir: Option<std::path::PathBuf>,
        /// Into this bench instead (by name).
        #[arg(long)]
        bench: Option<String>,
    },
    /// Shells on servers that outlive the bench.
    Remote {
        #[command(subcommand)]
        action: RemoteAction,
    },
    /// Lets Shed bring forward a browser tab already showing a project's link.
    Browser {
        #[command(subcommand)]
        action: BrowserAction,
    },
    /// The browser extension's native host, started by the browser.
    #[command(hide = true)]
    BrowserHost {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Tag the current tab (run inside Shed).
    Tag {
        #[command(subcommand)]
        action: TagAction,
    },
    /// Print the shell integration, for shells Shed did not start:
    /// eval "$(shed init bash)"
    Init { shell: String },
    /// Show commands from the logbook, most recent first.
    Log {
        /// local, bench or global (local and bench need to run inside Shed).
        #[arg(long, default_value = "global")]
        scope: String,
        /// How many to show.
        #[arg(short = 'n', long, default_value_t = 20)]
        count: usize,
        /// Only commands tagged with this.
        #[arg(long)]
        tag: Option<String>,
        /// Only commands containing this.
        query: Option<String>,
    },
    /// Show this tab's startup and teardown (run inside Shed).
    Setup {
        /// Edit it in $EDITOR instead.
        #[arg(long)]
        edit: bool,
    },
    /// List Shed's aliases and the shell's, or add and remove them.
    Alias {
        #[command(subcommand)]
        action: Option<AliasAction>,
    },
    /// Show whether the keeper is running, and what it holds.
    Status,
    /// Back up your setup now (shed.toml, aliases, themes, saved benches);
    /// Shed also does once a day. `list` and `restore NAME` too.
    Backup {
        #[command(subcommand)]
        action: Option<BackupAction>,
    },
    /// Stop the keeper and every tool it holds.
    Stop,
    /// Link this workspace to a nexus, to reach it from the web and Android apps.
    Nexus {
        #[command(subcommand)]
        action: NexusAction,
    },
    /// The link to a nexus, started by the keeper.
    #[command(hide = true)]
    Link,
    /// Types text into the first bench's active pane (for testing; `\r` is Enter).
    #[command(hide = true)]
    DebugType { text: String },
    /// Sends a prompt to the first AI tab (for testing), optionally choosing its model first.
    #[command(hide = true)]
    DebugAi {
        #[arg(default_value = "")]
        text: String,
        #[arg(long)]
        model: Option<String>,
        /// Instead of a prompt, answer every waiting request: allow or deny.
        #[arg(long)]
        answer: Option<String>,
    },
    /// Shed in this terminal (over ssh too: ssh -t host shed tui): benches,
    /// terminals, AI and project tabs. Ctrl+B d leaves; everything keeps running.
    #[cfg(feature = "tui")]
    Tui {
        /// Open this bench (loading it if it is closed).
        bench: Option<String>,
    },
    /// Called by the shell hook.
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
}

#[derive(Subcommand)]
enum NexusAction {
    /// Pair this workspace with the nexus at URL: shows a code to approve in
    /// the web app, then links.
    Join {
        url: String,
        /// The name it goes by there (default: the machine's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// The nexus set up here, the pairing, and whether the link is connected.
    Status,
    /// Forget the pairing and the nexus here; the link stops.
    Leave,
    /// Sign this desktop in to a nexus (asks for user, password and code),
    /// to show other workspaces in Shed's window. Apart from the pairing.
    Signin {
        /// The nexus (default: the one paired with, or signed in to before).
        url: Option<String>,
    },
    /// Forget this desktop's sign-in to the nexus.
    Signout,
}

#[derive(Subcommand)]
enum AliasAction {
    /// Set an alias. Inside Shed it applies to this tab unless told otherwise.
    Add {
        name: String,
        command: String,
        #[command(flatten)]
        level: AliasLevelArgs,
    },
    /// Remove an alias from a level.
    Rm {
        name: String,
        #[command(flatten)]
        level: AliasLevelArgs,
    },
}

#[derive(clap::Args)]
#[group(multiple = false)]
struct AliasLevelArgs {
    /// Everywhere.
    #[arg(long)]
    global: bool,
    /// Every tab of this bench.
    #[arg(long)]
    bench: bool,
    /// This tab (the default inside Shed).
    #[arg(long)]
    tab: bool,
    /// The shell's own: kept in ~/.config/shed/shell-aliases.sh, which
    /// ~/.bashrc and ~/.zshrc source, so it works in every terminal.
    #[arg(long)]
    shell: bool,
}

impl AliasLevelArgs {
    fn level(&self) -> Option<setup_cli::Level> {
        if self.global {
            Some(setup_cli::Level::Global)
        } else if self.bench {
            Some(setup_cli::Level::Bench)
        } else if self.tab {
            Some(setup_cli::Level::Tab)
        } else if self.shell {
            Some(setup_cli::Level::Shell)
        } else {
            None
        }
    }
}

#[derive(Subcommand)]
enum HookAction {
    /// Prints step `pos` of walking `scope`'s history from the current tab:
    /// "<pos> <total> <source tab, base64>", then the command.
    Walk {
        #[arg(long)]
        scope: String,
        #[arg(long)]
        pos: usize,
        #[arg(long, default_value = "")]
        prefix: String,
    },
}

fn main() -> anyhow::Result<()> {
    let command = Cli::parse().command;
    // The link runs in the background; its log says what it did.
    let level = if matches!(command, Some(Command::Link)) { "info" } else { "warn" };
    let filter = EnvFilter::try_from_env("SHED_LOG").unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
    log_panics();

    match command {
        None => run_app(ShowTarget::Shed),
        Some(Command::New { name, cwd }) => {
            let cwd = cwd.map(std::path::absolute).transpose()?;
            run_app(ShowTarget::NewBench { name, cwd })
        }
        Some(Command::Load { name, workspace: Some(workspace), .. }) => run_app(ShowTarget::InWorkspace { workspace, bench: name }),
        Some(Command::Load { name, quiet: true, .. }) => run_app(ShowTarget::LoadQuiet { name }),
        Some(Command::Load { name, start: true, .. }) => run_app(ShowTarget::LoadAndStart { name }),
        Some(Command::Load { name, .. }) => run_app(ShowTarget::Load { name }),
        Some(Command::Hide { name }) => cli::hide(&name),
        Some(Command::Save { name, as_name }) => cli::save(&name, as_name),
        Some(Command::Delete { name, yes }) => {
            if !yes {
                eprint!("Delete the bench {name:?}, stopping its tools and removing its saved file? [y/N] ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !answer.trim().eq_ignore_ascii_case("y") {
                    return Ok(());
                }
            }
            let (conn, rx) = match keeper_conn::connect(ClientKind::Cli, false) {
                Ok(c) => c,
                Err(_) => {
                    // No keeper: only the files.
                    let store = shed_save::Store::default();
                    let launcher = store.load(&name).map(|b| b.launcher()).unwrap_or_else(|_| shed_model::slug(&name));
                    store.delete(&name)?;
                    shed_desktop::Desktop::new(shed_model::paths::data_home()).remove(&launcher);
                    println!("deleted {name}");
                    return Ok(());
                }
            };
            conn.send_msg(ClientMsg::DeleteBench { name });
            loop {
                match rx.recv_blocking() {
                    Ok(Frame::Keeper(KeeperMsg::Done(text))) => {
                        println!("{text}");
                        return Ok(());
                    }
                    Ok(Frame::Keeper(KeeperMsg::Error(e))) => bail!("{e}"),
                    Ok(_) => {}
                    Err(_) => bail!("the keeper closed the connection"),
                }
            }
        }
        Some(Command::List) => cli::list(),
        Some(Command::Setup { edit: false }) => setup_cli::show_setup(),
        Some(Command::Setup { edit: true }) => setup_cli::edit_setup(),
        Some(Command::Alias { action: None }) => setup_cli::list_aliases(),
        Some(Command::Alias { action: Some(AliasAction::Add { name, command, level }) }) => {
            setup_cli::change_alias(&name, Some(&command), level.level())
        }
        Some(Command::Alias { action: Some(AliasAction::Rm { name, level }) }) => setup_cli::change_alias(&name, None, level.level()),
        Some(Command::Status) => cli::status(),
        Some(Command::Tag { action }) => tag(action),
        Some(Command::BrowserHost { .. }) => Ok(browser::host()?),
        Some(Command::Remote { action: RemoteAction::Connect { host, id } }) => remote::connect(&host, &id),
        Some(Command::Remote { action: RemoteAction::New { host, bench, join } }) => cli::remote_new(host, bench, join),
        Some(Command::Remote { action: RemoteAction::List { host } }) => {
            for shell in remote::list_shells(&host)? {
                println!("{}  {}", shell.id, remote::describe(&shell));
            }
            Ok(())
        }
        Some(Command::Browser { action: BrowserAction::Setup { profile } }) => {
            let browsers = browser::install(&profile)?;
            if browsers.is_empty() {
                println!("No Chrome, Chromium, Brave, Edge or Vivaldi found; links open the usual way.");
            } else {
                for b in &browsers {
                    println!("registered with {}", b.display());
                }
                println!(
                    "\nNow load the extension once: open chrome://extensions (or brave://extensions),\n\
                     turn on Developer mode, choose \"Load unpacked\", and pick\n  {}\n\
                     Links in project tabs then bring their tab forward instead of opening it again.",
                    browser::extension_dir().display()
                );
            }
            Ok(())
        }
        Some(Command::Project { dir, bench }) => project(dir, bench, false),
        Some(Command::Ai { dir, bench }) => project(dir, bench, true),
        Some(Command::Stop) => cli::stop(),
        Some(Command::Link) => std::process::exit(shed_link::main()),
        Some(Command::Nexus { action: NexusAction::Join { url, name } }) => shed_link::pairing::join(&url, name),
        Some(Command::Nexus { action: NexusAction::Status }) => shed_link::pairing::status(),
        Some(Command::Nexus { action: NexusAction::Leave }) => shed_link::pairing::leave(),
        Some(Command::Nexus { action: NexusAction::Signin { url } }) => shed_link::pairing::signin(url),
        Some(Command::Nexus { action: NexusAction::Signout }) => shed_link::pairing::signout(),
        Some(Command::Backup { action }) => backup(action),
        Some(Command::DebugType { text }) => cli::debug_type(&text),
        Some(Command::DebugAi { text, model, answer }) => debug_ai(text, model, answer),
        Some(Command::Init { shell }) => match shell.as_str() {
            "bash" => {
                print!("{}", shed_logbook::BASH_HOOK);
                Ok(())
            }
            "zsh" => {
                print!("{}", shed_logbook::ZSH_HOOK);
                Ok(())
            }
            other => bail!("no integration for {other:?} yet; bash and zsh have one"),
        },
        Some(Command::Log { scope, count, tag, query }) => logbook::print_log(&scope, count, tag.as_deref(), query.as_deref()),
        #[cfg(feature = "tui")]
        Some(Command::Tui { bench }) => shed_tui::run(bench.map_or(ShowTarget::Shed, |name| ShowTarget::Load { name })),
        Some(Command::Hook { action: HookAction::Walk { scope, pos, prefix } }) => {
            // The hook treats any failure as "nothing to show".
            match logbook::hook_walk(&scope, pos, &prefix) {
                Ok(true) => Ok(()),
                _ => std::process::exit(1),
            }
        }
    }
}

/// Shows `target`. If a window process is already running, it shows it and
/// this process exits; otherwise this process becomes the window process.
/// A crash leaves its cause in `~/.local/state/shed/shed.log` (the window
/// process has no terminal to print it to), then goes on as it would.
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let path = shed_model::paths::state_dir().join("shed.log");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let thread = std::thread::current().name().unwrap_or("?").to_string();
            let _ = writeln!(f, "--- shed {} panicked at {when} (unix time), thread {thread}: {info}", env!("CARGO_PKG_VERSION"));
            let _ = writeln!(f, "{}", std::backtrace::Backtrace::force_capture());
        }
        previous(info);
    }));
}

/// Shows `target`: in a window (see `gui::run_app`), or, built without
/// windows, in this terminal.
fn run_app(target: ShowTarget) -> anyhow::Result<()> {
    #[cfg(feature = "gui")]
    return gui::run_app(target);
    #[cfg(all(not(feature = "gui"), feature = "tui"))]
    return shed_tui::run(target);
    #[cfg(not(any(feature = "gui", feature = "tui")))]
    {
        let _ = target;
        bail!("this shed was built without windows or a terminal interface (features gui, tui)")
    }
}
fn project(dir: Option<std::path::PathBuf>, bench: Option<String>, ai: bool) -> anyhow::Result<()> {
    let here = std::env::current_dir()?;
    let root = match dir {
        Some(d) => std::fs::canonicalize(here.join(d))?,
        None => shed_project::project_root(&here),
    };
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    let tree = request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s.tree),
        _ => None,
    })?;
    let bench = cli::bench_named_or_here(&tree, bench)?;
    let msg = if ai { ClientMsg::NewAiTab { bench, folder: root } } else { ClientMsg::NewProjectTab { bench, root } };
    let done = request(msg, |m| match m {
        KeeperMsg::Done(text) => Some(text),
        _ => None,
    })?;
    println!("{done}");
    Ok(())
}

fn debug_ai(text: String, model: Option<String>, answer: Option<String>) -> anyhow::Result<()> {
    let tree = request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s.tree),
        _ => None,
    })?;
    let Some(tab) = tree.benches.iter().flat_map(|b| &b.tabs).find_map(|t| t.ai_info().map(|_| t.id)) else {
        bail!("no AI tab");
    };
    let (conn, _rx) = keeper_conn::connect(ClientKind::Cli, false)?;
    if let Some(answer) = answer {
        conn.send_msg(ClientMsg::AiAnswer { tab, request: "*".into(), allow: answer == "allow", always: false });
        std::thread::sleep(Duration::from_millis(200));
        return Ok(());
    }
    if let Some(model) = model {
        let change = shed_proto::AiChange::Settings { model: Some(Some(model)), mode: None, effort: None, follow: None, browser: None, add_dirs: None };
        conn.send_msg(ClientMsg::AiChange { tab, change });
    }
    conn.send_msg(ClientMsg::AiSend { tab, text, images: Vec::new() });
    std::thread::sleep(Duration::from_millis(200));
    Ok(())
}

fn tag(action: TagAction) -> anyhow::Result<()> {
    use shed_model::{Tag, TabId, default_tag_color, tag_color};
    let Some(tab) = std::env::var("SHED_TAB").ok().and_then(|t| t.parse().ok()).map(TabId) else {
        bail!("not inside a Shed tab ($SHED_TAB is not set)");
    };
    let tree = request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s.tree),
        _ => None,
    })?;
    let Some(current) = tree.tab(tab) else { bail!("this tab is no longer in the keeper") };
    let mut tags = current.tags.clone();
    match action {
        TagAction::List => {
            for t in current.all_tags() {
                let auto = if current.tags.iter().any(|m| m.label == t.label) { "" } else { " (rule)" };
                println!("{} #{:06X}{}{auto}", t.label, t.color.to_hex(), if t.tint_tab { " tint" } else { "" });
            }
            return Ok(());
        }
        TagAction::Add { label, color, tint } => {
            let color = match color {
                Some(c) => tag_color(&c).ok_or_else(|| anyhow::anyhow!("unknown colour {c:?}"))?,
                None => default_tag_color(&label),
            };
            tags.retain(|t| t.label != label);
            tags.push(Tag { label, color, tint_tab: tint });
        }
        TagAction::Remove { label } => {
            let before = tags.len();
            tags.retain(|t| t.label != label);
            if tags.len() == before {
                bail!("this tab has no tag {label:?}");
            }
        }
    }
    let (conn, _rx) = keeper_conn::connect(ClientKind::Cli, false)?;
    conn.send_msg(ClientMsg::SetTags { tab, tags });
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

fn backup(action: Option<BackupAction>) -> anyhow::Result<()> {
    use shed_save::backup;
    let places = backup::Places::default();
    match action {
        None => {
            let made = backup::now_backup(&places)?;
            println!("Backed up to {}", made.display());
        }
        Some(BackupAction::List) => {
            let names = backup::list(&places);
            if names.is_empty() {
                println!("No backups yet (Shed makes one a day; `shed backup` makes one now).");
            }
            for n in names {
                println!("{n}");
            }
            println!("in {}", places.backups.display());
        }
        Some(BackupAction::Restore { name }) => {
            if std::os::unix::net::UnixStream::connect(shed_model::paths::socket_path()).is_ok() {
                bail!("Shed is running: stop it first with `shed stop` (your benches are saved), then restore");
            }
            let before = backup::restore(&places, &name)?;
            println!("Restored {name}. What was there before is in {}", before.display());
        }
    }
    Ok(())
}

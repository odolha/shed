//! `shed remote connect HOST ID`: what a remote tab's pane runs. It makes
//! sure the server has this Shed's `shedd` (copying it to `~/.local/bin`
//! over ssh when missing or different), then runs `ssh -t HOST shedd attach
//! ID`, which joins the shell the server keeps. A dropped connection is
//! joined again until the shell ends or Ctrl+C; the window's Reconnect
//! (SIGUSR1, from the keeper) drops it and joins again at once.
//!
//! ssh runs in the pane, so passwords and 2FA are asked right there; one
//! connection is shared by the steps (`ControlMaster`), so they are asked once.
//! When a join ends badly that shared connection is closed too, as a dead one
//! would hang the next join.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Where `shedd` goes on the server, and the file saying which build it is.
const REMOTE_SHEDD: &str = ".local/bin/shedd";
const REMOTE_ID: &str = ".local/bin/.shedd-id";

/// The longest a step that types nothing (asking about the shared
/// connection, closing it) may take: a hung connection must not hang the pane.
const QUICK: Duration = Duration::from_secs(3);

/// A join shorter than this that ends badly failed; a longer one was a
/// connection, lost.
const SETTLED: Duration = Duration::from_secs(10);

/// The program standing in for ssh (tests use a stand-in).
fn ssh_program() -> String {
    std::env::var("SHED_SSH").unwrap_or_else(|_| "ssh".into())
}

fn ssh(host: &str) -> Command {
    ssh_with(host, &[])
}

/// ssh to `host` sharing one connection, with `options` before the host.
/// A dead connection is noticed within 15 s (`ServerAlive*`).
fn ssh_with(host: &str, options: &[&str]) -> Command {
    let mut c = Command::new(ssh_program());
    let control = std::env::temp_dir().join("shed-ssh-%C");
    c.args(["-o", "ControlMaster=auto", "-o", &format!("ControlPath={}", control.display()), "-o", "ControlPersist=120"]);
    c.args(["-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=3", "-o", "ConnectTimeout=10"]);
    c.args(options);
    c.arg(host);
    c
}

/// The `shedd` to put on a Linux server of architecture `arch`: a static
/// build for it if there is one beside `shed` (it runs on any Linux), else,
/// on a Linux machine of the same architecture, the one Shed itself uses.
fn local_shedd(arch: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SHED_REMOTE_SHEDD") {
        return Some(p.into());
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let static_build = dir.join(format!("shedd-{arch}-static"));
    if static_build.is_file() {
        return Some(static_build);
    }
    let same = cfg!(target_os = "linux") && arch == std::env::consts::ARCH;
    Some(dir.join("shedd")).filter(|p| same && p.is_file())
}

/// Which build a `shedd` is: its version and size, enough to tell builds apart.
fn build_id(path: &std::path::Path) -> String {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    format!("{} {size}", env!("CARGO_PKG_VERSION"))
}

fn say(text: &str) {
    // Dimmed, so it reads as Shed's, not the server's.
    print!("\x1b[2m{text}\x1b[0m\r\n");
    let _ = std::io::stdout().flush();
}

/// Set by SIGUSR1: the window's Reconnect.
static RECONNECT: AtomicBool = AtomicBool::new(false);

extern "C" fn on_reconnect(_: libc::c_int) {
    RECONNECT.store(true, Ordering::SeqCst);
}

/// Hears the window's Reconnect: the keeper sends this process SIGUSR1.
fn hear_reconnect() {
    // SAFETY: the handler only stores to an atomic, which is
    // async-signal-safe; `action` is fully set before use.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_reconnect as extern "C" fn(libc::c_int) as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut());
    }
}

fn reconnect_asked() -> bool {
    RECONNECT.swap(false, Ordering::SeqCst)
}

/// Reconnect was asked during a step.
#[derive(Debug)]
struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("reconnecting")
    }
}

impl std::error::Error for Interrupted {}

enum Ended {
    Exited(ExitStatus),
    Reconnect,
}

/// Waits for an ssh, stopping it at once when Reconnect is asked.
fn wait(child: &mut Child) -> std::io::Result<Ended> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Ended::Exited(status));
        }
        if reconnect_asked() {
            stop(child);
            return Ok(Ended::Reconnect);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Ends an ssh: asked first (it puts the terminal back), killed if it hangs.
fn stop(child: &mut Child) {
    // SAFETY: signalling our own child, which has not been waited for.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits `how_long`; true when Reconnect cut it short.
fn pause(how_long: Duration) -> bool {
    let deadline = Instant::now() + how_long;
    while Instant::now() < deadline {
        if reconnect_asked() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    reconnect_asked()
}

/// How long to wait before trying again after `failures` failed joins in a
/// row: 3 s, doubling, at most 30 s.
fn backoff(failures: u32) -> Duration {
    Duration::from_secs((3u64 << failures.saturating_sub(1).min(4)).min(30))
}

/// The pane's terminal settings, put back after an ssh was stopped in raw mode.
struct Tty(Option<libc::termios>);

impl Tty {
    fn save() -> Self {
        // SAFETY: an all-zero `termios` is valid; `tcgetattr` fills it.
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `t` is writable.
        Tty((unsafe { libc::tcgetattr(0, &mut t) } == 0).then_some(t))
    }

    fn restore(&self) {
        if let Some(t) = &self.0 {
            // SAFETY: `t` came from `tcgetattr`.
            unsafe { libc::tcsetattr(0, libc::TCSANOW, t) };
        }
    }
}

/// Runs a step that types nothing, killed after [`QUICK`]: `None` then.
fn run_quick(mut c: Command) -> Option<Output> {
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let deadline = Instant::now() + QUICK;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// The pid in ssh's "Master running (pid=N)".
fn master_pid(text: &str) -> Option<u32> {
    let at = text.find("(pid=")? + "(pid=".len();
    text[at..].split(')').next()?.trim().parse().ok()
}

/// The pid of the ssh holding the connection to `host` that ssh steps
/// share, if there is one. One that does not answer is let go of (its
/// socket removed), so the next step opens a fresh connection.
fn shared_connection(host: &str) -> Option<u32> {
    let Some(out) = run_quick(ssh_with(host, &["-O", "check"])) else {
        forget_shared(host);
        return None;
    };
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    master_pid(&text).filter(|_| out.status.success())
}

/// Removes the socket of a shared connection that does not answer.
fn forget_shared(host: &str) {
    use std::os::unix::fs::FileTypeExt;
    let Some(out) = run_quick(ssh_with(host, &["-G"])) else { return };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(path) = text.lines().find_map(|l| l.strip_prefix("controlpath ")).map(|p| PathBuf::from(p.trim())) else { return };
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_socket()) {
        let _ = std::fs::remove_file(&path);
    }
}

/// Closes the shared connection `through` (the one a join went through),
/// so the next join opens a fresh one. A newer one, another tab's, is left.
fn close_shared(host: &str, through: Option<u32>) {
    let Some(pid) = through.filter(|p| shared_connection(host) == Some(*p)) else { return };
    if !run_quick(ssh_with(host, &["-O", "exit"])).is_some_and(|o| o.status.success()) {
        // SAFETY: plain kill(2) of the ssh that answered for the connection just now.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
}

/// Puts this Shed's `shedd` on the server unless it is there already.
fn ensure_shedd(host: &str) -> anyhow::Result<()> {
    let mut child = ssh(host)
        .arg(format!("uname -s; uname -m; cat {REMOTE_ID} 2>/dev/null; true"))
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let status = match wait(&mut child)? {
        Ended::Reconnect => return Err(Interrupted.into()),
        Ended::Exited(status) => status,
    };
    if !status.success() {
        anyhow::bail!("could not reach {host} ({})", how_ssh_ended(status));
    }
    let mut text = String::new();
    child.stdout.take().unwrap().read_to_string(&mut text)?;
    let mut lines = text.lines().map(str::trim);
    let (os, arch, theirs) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""), lines.next().unwrap_or(""));
    if os != "Linux" {
        anyhow::bail!("{host} runs {os}; remote tabs need a Linux server");
    }
    let Some(local) = local_shedd(arch) else {
        anyhow::bail!("no shedd for Linux {arch} here: put a static build beside shed as shedd-{arch}-static");
    };
    let id = build_id(&local);
    if theirs == id {
        return Ok(());
    }
    say(&format!("Putting shedd on {host} (~/{REMOTE_SHEDD})…"));
    let script = upload_script(&id);
    let mut child = ssh(host).arg(script).stdin(Stdio::piped()).spawn()?;
    std::io::copy(&mut std::fs::File::open(&local)?, child.stdin.as_mut().unwrap())?;
    drop(child.stdin.take());
    if !child.wait()?.success() {
        anyhow::bail!("could not copy shedd to {host}");
    }
    Ok(())
}

/// The shell script a server runs to take shedd from its stdin.
fn upload_script(id: &str) -> String {
    // Several remote tabs to this server reconnect at once after an update,
    // each sending shedd: each writes a file of its own (the remote shell's
    // pid in its name) and renames it into place, so none starts a shedd
    // another is still writing ("Text file busy") or renames one away
    // mid-write; the id goes last, the same way. Leftovers of a cut-off copy
    // older than an hour are cleared.
    format!(
        "mkdir -p .local/bin && find .local/bin -maxdepth 1 -name 'shedd.new.*' -mmin +60 -delete 2>/dev/null; \
         t={REMOTE_SHEDD}.new.$$; i={REMOTE_ID}.new.$$; \
         cat > \"$t\" && chmod 755 \"$t\" && mv -f \"$t\" {REMOTE_SHEDD} && echo '{id}' > \"$i\" && mv -f \"$i\" {REMOTE_ID}; \
         s=$?; rm -f \"$t\" \"$i\"; exit $s"
    )
}

fn how_ssh_ended(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("ssh ended with code {code}"),
        None => "ssh was stopped".into(),
    }
}

/// How a join ended.
enum Join {
    /// The shell ended, or this end let go: nothing more to do.
    Done,
    /// The window asked to connect again.
    Reconnect,
    /// The connection ended after working a while.
    Lost,
    /// Joining did not work, for this reason.
    Failed(String),
}

/// One join of the shell: a shared connection (opened first, asking for a
/// password if one is needed), `shedd` on the server, then the attach.
fn join(host: &str, id: &str, installed: &mut bool, tty: &Tty) -> Join {
    let mut through = shared_connection(host);
    let step = if !*installed {
        ensure_shedd(host).map(|()| *installed = true)
    } else if through.is_none() {
        // Opened on its own, so the attach goes through a connection known
        // to be this join's (and only that one is closed if it ends badly).
        ssh(host).arg("true").stdin(Stdio::inherit()).spawn().map_err(anyhow::Error::from).and_then(|mut child| match wait(&mut child)? {
            Ended::Reconnect => Err(Interrupted.into()),
            Ended::Exited(s) if s.success() => Ok(()),
            Ended::Exited(s) => Err(anyhow::anyhow!("could not reach {host} ({})", how_ssh_ended(s))),
        })
    } else {
        Ok(())
    };
    match step {
        Ok(()) => {}
        Err(e) if e.is::<Interrupted>() => {
            tty.restore();
            return Join::Reconnect;
        }
        Err(e) => return Join::Failed(e.to_string()),
    }
    if through.is_none() {
        through = shared_connection(host);
    }
    let started = Instant::now();
    let attach = ssh_with(host, &["-o", "TCPKeepAlive=yes"]).arg("-t").arg(format!("exec {REMOTE_SHEDD} attach {id}")).spawn();
    let ended = match attach.and_then(|mut child| wait(&mut child)) {
        Ok(ended) => ended,
        Err(e) => return Join::Failed(format!("cannot run ssh: {e}")),
    };
    match ended {
        Ended::Reconnect => {
            tty.restore();
            close_shared(host, through);
            Join::Reconnect
        }
        // 0: the shell ended, or the tab was closed; 2: this end let go.
        Ended::Exited(status) if matches!(status.code(), Some(0 | 2)) => Join::Done,
        // 3: the shell was lost with the server's keeper (a reboot); that
        // brings its saved shell back. Otherwise the connection ended.
        Ended::Exited(status) => {
            tty.restore();
            close_shared(host, through);
            if started.elapsed() >= SETTLED {
                Join::Lost
            } else {
                Join::Failed(format!("could not join the shell on {host} ({})", how_ssh_ended(status)))
            }
        }
    }
}

pub fn connect(host: &str, id: &str) -> anyhow::Result<()> {
    hear_reconnect();
    note_used(host);
    let tty = Tty::save();
    let mut installed = false;
    let mut failures = 0;
    say(&format!("Connecting to {host}…"));
    loop {
        match join(host, id, &mut installed, &tty) {
            Join::Done => return Ok(()),
            Join::Reconnect => {
                failures = 0;
                say(&format!("Reconnecting to {host}…"));
            }
            Join::Lost => {
                failures = 0;
                say(&format!("Connection to {host} lost; joining again…"));
            }
            Join::Failed(why) => {
                failures += 1;
                let wait = backoff(failures);
                say(&format!("{why}; trying again in {} s (Ctrl+C stops)", wait.as_secs()));
                if pause(wait) {
                    failures = 0;
                    say(&format!("Reconnecting to {host}…"));
                } else {
                    say(&format!("Connecting to {host}…"));
                }
            }
        }
    }
}

/// Services reached over ssh only for Git: never a shell to offer.
const GIT_SERVICES: &[&str] = &["github.com", "gitlab.com", "bitbucket.org", "ssh.github.com", "codeberg.org", "git.sr.ht", "ssh.dev.azure.com", "vs-ssh.visualstudio.com"];

/// `host` (maybe `user@host`) is a Git service's.
fn is_git_service(host: &str) -> bool {
    let h = host.rsplit('@').next().unwrap_or(host).to_ascii_lowercase();
    GIT_SERVICES.contains(&h.as_str()) || h.starts_with("altssh.")
}

/// How deep `Include`s are followed (they may include each other).
const MAX_INCLUDE_DEPTH: usize = 4;

/// The hosts an ssh config names on its `Host` lines (patterns left out),
/// in order, each with whether it is for Git only: its block says `User
/// git`, or its `HostName` (or its name) is a Git service's. `Match` blocks
/// name no host. `Include`d files are read through `include` (given the
/// path as written; it gives the text of each file it names).
pub fn config_hosts(text: &str, include: &mut dyn FnMut(&str) -> Vec<String>) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    read_config(text, include, 0, &mut out);
    out
}

fn read_config(text: &str, include: &mut dyn FnMut(&str) -> Vec<String>, depth: usize, out: &mut Vec<(String, bool)>) {
    // The hosts of the block being read, by their place in `out`.
    let mut block: Vec<usize> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `Key value` or `Key=value`.
        let at = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
        let (key, value) = (line[..at].to_ascii_lowercase(), line[at..].trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim());
        let value = value.trim_matches('"');
        match key.as_str() {
            "host" => {
                block.clear();
                for name in value.split_whitespace().filter(|h| !h.contains(['*', '?', '!'])) {
                    let at = out.iter().position(|(h, _)| h == name).unwrap_or_else(|| {
                        out.push((name.to_string(), false));
                        out.len() - 1
                    });
                    if is_git_service(name) {
                        out[at].1 = true;
                    }
                    block.push(at);
                }
            }
            "match" => block.clear(),
            "user" if value == "git" => block.iter().for_each(|&i| out[i].1 = true),
            "hostname" if is_git_service(value) => block.iter().for_each(|&i| out[i].1 = true),
            "include" if depth < MAX_INCLUDE_DEPTH => {
                for path in value.split_whitespace() {
                    for text in include(path) {
                        read_config(&text, include, depth + 1, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// `name` matches `pattern` (`*` any run, `?` one character).
fn wildcard(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|i| go(rest, &n[i..])),
            Some(('?', rest)) => !n.is_empty() && go(rest, &n[1..]),
            Some((c, rest)) => n.first() == Some(c) && go(rest, &n[1..]),
        }
    }
    go(&pattern.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
}

/// The files an `Include` names, as ssh finds them (`~/…`, or relative to
/// `~/.ssh`; a wildcard in the file's name), their text each.
fn read_include(ssh_dir: &std::path::Path, home: &std::path::Path, path: &str) -> Vec<String> {
    let full = match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path.starts_with('/') => PathBuf::from(path),
        None => ssh_dir.join(path),
    };
    let (Some(dir), Some(name)) = (full.parent(), full.file_name().map(|n| n.to_string_lossy().into_owned())) else { return Vec::new() };
    let mut files: Vec<PathBuf> = if name.contains(['*', '?']) {
        std::fs::read_dir(dir).map(|d| d.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| wildcard(&name, &n.to_string_lossy()))).collect()).unwrap_or_default()
    } else {
        vec![full]
    };
    files.sort();
    // Small text files only: an Include of something else reads nothing.
    files.iter().filter(|p| p.metadata().is_ok_and(|m| m.is_file() && m.len() < 1 << 20)).filter_map(|p| std::fs::read_to_string(p).ok()).collect()
}

/// Hosts to offer for a new remote tab: `recent` ones (those remote tabs
/// used, most recent first), then those named in `~/.ssh/config`. Hosts
/// only for Git (GitHub and the like) are left out, unless a remote tab
/// used one.
pub fn known_hosts(recent: &[String]) -> Vec<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config = home.as_ref().map_or(Vec::new(), |home| {
        let ssh = home.join(".ssh");
        let text = std::fs::read_to_string(ssh.join("config")).unwrap_or_default();
        config_hosts(&text, &mut |path| read_include(&ssh, home, path))
    });
    offered_hosts(recent, &config)
}

/// `recent` first, then the config's hosts but those for Git only; once each.
fn offered_hosts(recent: &[String], config: &[(String, bool)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for host in recent.iter().filter(|h| !h.trim().is_empty()) {
        if !out.contains(host) {
            out.push(host.clone());
        }
    }
    for (host, git) in config {
        if !git && !is_git_service(host) && !out.contains(host) {
            out.push(host.clone());
        }
    }
    out
}

/// Where the hosts remote tabs connected to are kept, latest first.
fn used_file() -> PathBuf {
    shed_model::paths::state_dir().join("remote-hosts")
}

/// How many hosts are remembered as used.
const USED_KEPT: usize = 30;

/// Remembers that a remote tab connected to `host` now.
pub fn note_used(host: &str) {
    let file = used_file();
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    let hosts: Vec<&str> = std::iter::once(host).chain(old.lines().map(str::trim).filter(|h| !h.is_empty() && *h != host)).take(USED_KEPT).collect();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = shed_model::paths::write_whole(&file, hosts.join("\n") + "\n");
}

/// The hosts remote tabs use, most recently used first: those they
/// connected to lately, then those of running benches, then those of saved
/// benches, the latest saved first.
pub fn recent_hosts(tree: &shed_model::Shed) -> Vec<String> {
    let used: Vec<String> = std::fs::read_to_string(used_file()).unwrap_or_default().lines().map(str::trim).filter(|h| !h.is_empty()).map(String::from).collect();
    recent_from(used, tree, &shed_save::Store::default())
}

fn recent_from(used: Vec<String>, tree: &shed_model::Shed, store: &shed_save::Store) -> Vec<String> {
    let running = tree.benches.iter().flat_map(|b| &b.tabs).filter_map(|t| t.remote_info()).map(|r| r.host.clone());
    let mut saved: Vec<(String, Vec<String>)> = std::fs::read_dir(&store.dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .filter_map(|p| std::fs::read_to_string(p).ok())
                .filter(|text| text.contains("[tabs.remote]"))
                .filter_map(|text| shed_save::SavedBench::from_toml(&text).ok())
                .map(|b| (b.saved.clone(), b.tabs.iter().filter_map(|t| t.remote.as_ref()).map(|r| r.host.clone()).collect()))
                .collect()
        })
        .unwrap_or_default();
    saved.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<String> = Vec::new();
    for host in used.into_iter().chain(running).chain(saved.into_iter().flat_map(|(_, hosts)| hosts)) {
        if !host.trim().is_empty() && !out.contains(&host) {
            out.push(host);
        }
    }
    out
}

/// A shell the server keeps, as `shedd list` describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteShell {
    pub id: String,
    /// Running now; else saved (the server restarted), started when joined.
    pub running: bool,
    /// How many PCs show it now.
    pub attached: u32,
    /// Its bench and tab on the server: the logbook knows it by them.
    pub bench: String,
    pub tab: String,
    pub cwd: String,
    /// The program in front, if not the shell.
    pub tool: String,
}

fn parse_list(text: &str) -> Vec<RemoteShell> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            Some(RemoteShell {
                id: f.next()?.to_string(),
                running: f.next()? == "running",
                attached: f.next()?.parse().ok()?,
                bench: f.next()?.to_string(),
                tab: f.next()?.to_string(),
                cwd: f.next()?.to_string(),
                tool: f.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// ssh for a question asked from the window, not the pane: nothing can be
/// typed, so it never waits for a password (an open remote tab's shared
/// connection, or a key, lets it in).
fn ssh_quiet(host: &str, command: &str) -> anyhow::Result<std::process::Output> {
    let mut c = Command::new(ssh_program());
    let control = std::env::temp_dir().join("shed-ssh-%C");
    c.args(["-o", "ControlMaster=auto", "-o", &format!("ControlPath={}", control.display()), "-o", "ControlPersist=120"]);
    c.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8"]);
    c.arg(host).arg(command);
    let out = c.stdin(Stdio::null()).stderr(Stdio::piped()).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("{}", err.lines().last().unwrap_or("ssh failed").trim());
    }
    Ok(out)
}

/// The shells `host` keeps (none when Shed was never there).
pub fn list_shells(host: &str) -> anyhow::Result<Vec<RemoteShell>> {
    let out = ssh_quiet(host, &format!("test -x {REMOTE_SHEDD} || exit 0; exec {REMOTE_SHEDD} list"))?;
    Ok(parse_list(&String::from_utf8_lossy(&out.stdout)))
}

/// Fetches `host`'s command history into `dest`; false when it has none.
pub fn fetch_logbook(host: &str, dest: &std::path::Path) -> anyhow::Result<bool> {
    let out = ssh_quiet(host, &format!("test -x {REMOTE_SHEDD} || exit 0; exec {REMOTE_SHEDD} logbook"))?;
    if out.stdout.is_empty() {
        return Ok(false);
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &out.stdout)?;
    std::fs::rename(&tmp, dest)?;
    Ok(true)
}

/// How a shell reads in a list: its folder, and what runs in it.
pub fn describe(shell: &RemoteShell) -> String {
    let mut text = shell.cwd.clone();
    if !shell.tool.is_empty() {
        text.push_str(&format!(" · {}", shell.tool));
    }
    match (shell.running, shell.attached) {
        (false, _) => text.push_str(" (saved)"),
        (true, 0) => {}
        (true, 1) => text.push_str(" (open on 1 PC)"),
        (true, n) => text.push_str(&format!(" (open on {n} PCs)")),
    }
    text
}

#[cfg(test)]
mod tests {

    #[test]
    fn shedd_copies_at_once_never_mix() {
        use std::io::Write;
        let home = tempfile::tempdir().unwrap();
        let script = upload_script("id-2");
        // Four tabs sending shedd at once, each a different (large) file.
        let children: Vec<_> = (0..4u8)
            .map(|n| {
                let mut c = std::process::Command::new("bash")
                    .arg("-c")
                    .arg(&script)
                    .current_dir(home.path())
                    .stdin(Stdio::piped())
                    .spawn()
                    .unwrap();
                let body = vec![b'a' + n; 4 << 20];
                let mut stdin = c.stdin.take().unwrap();
                std::thread::spawn(move || {
                    stdin.write_all(&body).unwrap();
                });
                c
            })
            .collect();
        for mut c in children {
            assert!(c.wait().unwrap().success());
        }
        // One whole copy is in place, the id beside it, nothing left over.
        let shedd = std::fs::read(home.path().join(REMOTE_SHEDD)).unwrap();
        assert_eq!(shedd.len(), 4 << 20);
        assert!(shedd.iter().all(|&b| b == shedd[0]));
        assert_eq!(std::fs::read_to_string(home.path().join(REMOTE_ID)).unwrap().trim(), "id-2");
        let left: Vec<_> = std::fs::read_dir(home.path().join(".local/bin")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
    }

    use super::*;

    /// A config as people write them: patterns, several names on a line,
    /// `Match` blocks, `Include`s, Git hosts by `User git` or `HostName`.
    #[test]
    fn ssh_config_hosts_leave_git_and_patterns_out() {
        let config = "# mine\nHost *\n  ServerAliveInterval 30\n\nHost web db.internal  !nope *.corp\n  User deploy\n\nHost github.com\n  IdentityFile ~/.ssh/gh\n\nHost gh-work\n  HostName ssh.github.com\n  Port 443\n\nHost forge\n  HostName git.example.org\n  User git\n\nMatch host web exec \"true\"\n  User git\n\nInclude config.d/*\nHost=box\n  HostName 10.0.0.5\n";
        let mut asked = Vec::new();
        let mut include = |path: &str| {
            asked.push(path.to_string());
            vec!["Host lab\n  User me\nHost gitlab.com\nInclude loop\n".to_string()]
        };
        let hosts = config_hosts(config, &mut include);
        let names: Vec<(&str, bool)> = hosts.iter().map(|(h, g)| (h.as_str(), *g)).collect();
        assert_eq!(names[..5], [("web", false), ("db.internal", false), ("github.com", true), ("gh-work", true), ("forge", true)]);
        assert!(names.contains(&("lab", false)) && names.contains(&("gitlab.com", true)) && names.contains(&("box", false)), "{names:?}");
        // An Include that includes again stops after a few levels.
        assert_eq!(asked.len(), MAX_INCLUDE_DEPTH, "{asked:?}");
        let offered = offered_hosts(&["prod".into(), "gh-work".into()], &hosts);
        assert_eq!(offered, ["prod", "gh-work", "web", "db.internal", "lab", "box"]);
        assert!(is_git_service("git@altssh.gitlab.com") && is_git_service("vs-ssh.visualstudio.com") && !is_git_service("gitea.home"));
    }

    #[test]
    fn includes_are_found_as_ssh_finds_them() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(ssh.join("config.d/a.conf"), "Host a").unwrap();
        std::fs::write(ssh.join("config.d/b.conf"), "Host b").unwrap();
        std::fs::write(ssh.join("config.d/notes.txt"), "Host c").unwrap();
        assert_eq!(read_include(&ssh, home.path(), "config.d/*.conf"), ["Host a", "Host b"]);
        assert_eq!(read_include(&ssh, home.path(), "~/.ssh/config.d/b.conf"), ["Host b"]);
        assert!(read_include(&ssh, home.path(), "missing").is_empty());
        assert!(read_include(&ssh, home.path(), "config.d").is_empty(), "a folder is no file");
        assert!(wildcard("*.conf", "x.conf") && wildcard("a?c", "abc") && !wildcard("*.conf", "x.txt"));
    }

    /// Recent hosts: those connected to lately, then running benches', then
    /// saved benches' (the latest saved first), once each.
    #[test]
    fn recent_hosts_come_latest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = shed_save::Store::new(dir.path().to_path_buf());
        let saved = |name: &str, when: &str, hosts: &[&str]| {
            let tabs: String = hosts.iter().map(|h| format!("\n[[tabs]]\nid = \"{}\"\n\n[tabs.remote]\nhost = \"{h}\"\nid = \"x\"\n", shed_model::TabId::new())).collect();
            let text = format!("name = \"{name}\"\nid = \"{}\"\ncolor = \"blue\"\ncwd = \"/\"\nsaved = \"{when}\"\n{tabs}", shed_model::BenchId::new());
            assert!(shed_save::SavedBench::from_toml(&text).is_ok(), "{text}");
            std::fs::write(dir.path().join(format!("{name}.toml")), text).unwrap();
        };
        saved("old", "2026-01-01T00:00:00Z", &["old-host", "shared"]);
        saved("new", "2026-06-01T00:00:00Z", &["new-host"]);
        std::fs::write(dir.path().join("plain.toml"), "name = \"plain\"").unwrap();
        let mut tree = shed_model::Shed::default();
        let mut tab = shed_model::Tab::with_pane(shed_model::Pane::new(shed_model::PaneId::new(), "/".into()));
        tab.kind = shed_model::TabKind::Remote(shed_model::RemoteTab { host: "running-host".into(), id: "r".into() });
        tree.benches.push(shed_model::Bench {
            id: shed_model::BenchId::new(),
            name: "run".into(),
            color: shed_model::BenchColor::Blue,
            badge: None,
            cwd: "/".into(),
            active_tab: tab.id,
            tabs: vec![tab],
            aliases: Default::default(),
            closing: false,
            launcher: "run".into(),
            window: None,
            started: false,
            links: Vec::new(),
        });
        let hosts = recent_from(vec!["shared".into()], &tree, &store);
        assert_eq!(hosts, ["shared", "running-host", "new-host", "old-host"]);
    }

    #[test]
    fn lists_parse() {
        let shells = parse_list("a-1\trunning\t2\tB\tT\t~/x\tvim a.rs\nb-2\tsaved\t0\tB2\tT2\t~\t\nbroken\n");
        assert_eq!(shells.len(), 2);
        assert!(shells[0].running && shells[0].attached == 2 && shells[0].tool == "vim a.rs");
        assert_eq!(describe(&shells[0]), "~/x · vim a.rs (open on 2 PCs)");
        assert_eq!(describe(&shells[1]), "~ (saved)");
    }

    #[test]
    fn joins_back_off() {
        let secs: Vec<u64> = (1..=7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [3, 6, 12, 24, 30, 30, 30]);
        assert_eq!(backoff(u32::MAX).as_secs(), 30);
    }

    #[test]
    fn shared_connection_pids() {
        assert_eq!(master_pid("Master running (pid=4242)\r\n"), Some(4242));
        assert_eq!(master_pid("Control socket connect(/tmp/x): No such file or directory"), None);
    }
}

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
    let script = format!(
        "mkdir -p .local/bin && cat > {REMOTE_SHEDD}.new && chmod 755 {REMOTE_SHEDD}.new && mv -f {REMOTE_SHEDD}.new {REMOTE_SHEDD} && echo '{id}' > {REMOTE_ID}"
    );
    let mut child = ssh(host).arg(script).stdin(Stdio::piped()).spawn()?;
    std::io::copy(&mut std::fs::File::open(&local)?, child.stdin.as_mut().unwrap())?;
    drop(child.stdin.take());
    if !child.wait()?.success() {
        anyhow::bail!("could not copy shedd to {host}");
    }
    Ok(())
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

/// Hosts named in `~/.ssh/config`, to offer for a new remote tab.
pub fn known_hosts() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(PathBuf::from(home).join(".ssh/config")) else { return Vec::new() };
    let mut hosts: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let (key, rest) = l.split_once(char::is_whitespace)?;
            key.eq_ignore_ascii_case("host").then_some(rest)
        })
        .flat_map(|rest| rest.split_whitespace())
        .filter(|h| !h.contains(['*', '?', '!']))
        .map(String::from)
        .collect();
    hosts.dedup();
    hosts
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
    use super::*;

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
